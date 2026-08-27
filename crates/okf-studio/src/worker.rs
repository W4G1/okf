//! The background worker: every disk touch — loads, refactors, fixes, and
//! structured frontmatter edits — happens here, never on the UI thread.
//!
//! The worker keeps the latest [`Snapshot`] it built and runs refactor
//! previews and applies against that bundle. After every write it rebuilds,
//! so the UI converges on the on-disk truth without the watcher's help.

use crate::app::{Command, Msg, PreviewReport, RefactorOp};
use crate::snapshot::Snapshot;
use okf_core::log::append_log_entry;
use okf_core::scaffold::current_iso_timestamp;
use okf_core::{
    ConceptOptions, Date, Document, FixOptions, MergeOptions, MoveOptions, RefactorError,
    RemoveOptions, RenameSectionOptions, SplitOptions, Value, create_concept, merge_concepts,
    move_concept, remediate_bundle, remove_concept, rename_section, split_concept, yaml::Mapping,
};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

/// The worker's fixed configuration.
pub struct WorkerConfig {
    /// Bundle root directory.
    pub root: PathBuf,
    /// Pinned evaluation date, when given.
    pub today: Option<Date>,
    /// Author identity for verification stamps and log entries.
    pub author: String,
}

/// Spawns the worker thread and returns its command channel.
#[must_use]
pub fn spawn(config: WorkerConfig, msg_tx: Sender<Msg>) -> Sender<Command> {
    let (cmd_tx, cmd_rx) = channel::<Command>();
    std::thread::spawn(move || run_worker(&config, &cmd_rx, &msg_tx));
    cmd_tx
}

#[allow(clippy::too_many_lines)]
fn run_worker(config: &WorkerConfig, cmd_rx: &Receiver<Command>, msg_tx: &Sender<Msg>) {
    let mut generation: u64 = 0;
    let mut today = config.today;
    let mut snapshot: Option<Arc<Snapshot>> = None;

    let reload =
        |generation: &mut u64, today: Option<Date>, snapshot: &mut Option<Arc<Snapshot>>| -> bool {
            *generation += 1;
            match Snapshot::build(&config.root, today, *generation) {
                Ok(snap) => {
                    let snap = Arc::new(snap);
                    *snapshot = Some(Arc::clone(&snap));
                    let _ = msg_tx.send(Msg::SnapshotReady(snap));
                    true
                }
                Err(e) => {
                    let _ = msg_tx.send(Msg::SnapshotFailed(e.to_string()));
                    false
                }
            }
        };

    while let Ok(command) = cmd_rx.recv() {
        match command {
            Command::Shutdown => break,
            Command::Reload => {
                reload(&mut generation, today, &mut snapshot);
            }
            Command::SetToday(date) => {
                today = date;
                reload(&mut generation, today, &mut snapshot);
            }
            Command::Preview { request, op } => {
                let result = snapshot.as_ref().map_or_else(
                    || Err(RefactorError::Io("no snapshot loaded yet".into())),
                    |snap| run_refactor(snap, &op, true, &config.author),
                );
                let _ = msg_tx.send(Msg::PreviewReady(request, result));
            }
            Command::Apply(op) => {
                // Re-run against the freshest bundle: if files changed since
                // the preview, the apply still operates on disk truth.
                reload(&mut generation, today, &mut snapshot);
                let result = snapshot.as_ref().map_or_else(
                    || Err(RefactorError::Io("no snapshot loaded yet".into())),
                    |snap| run_refactor(snap, &op, false, &config.author),
                );
                let _ = msg_tx.send(Msg::Applied(
                    result
                        .map(|report| toast_for(&report))
                        .map_err(|e| e.to_string()),
                ));
                reload(&mut generation, today, &mut snapshot);
            }
            Command::StampVerification(id) => {
                let result = stamp_verification(config, &id);
                let _ = msg_tx.send(Msg::Applied(result));
                reload(&mut generation, today, &mut snapshot);
            }
            Command::SetStaleAfter(id, date) => {
                let result = set_stale_after(config, &id, date);
                let _ = msg_tx.send(Msg::Applied(result));
                reload(&mut generation, today, &mut snapshot);
            }
            Command::CreateConcept {
                rel_path,
                type_,
                title,
            } => {
                let options = ConceptOptions {
                    type_,
                    title,
                    author: Some(config.author.clone()),
                    ..ConceptOptions::default()
                };
                let result = create_concept(config.root.join(&rel_path), &options)
                    .map(|path| format!("✔ created {}", path.display()))
                    .map_err(|e| e.to_string());
                let _ = msg_tx.send(Msg::Applied(result));
                reload(&mut generation, today, &mut snapshot);
            }
            Command::PreviewFix => match remediate_bundle(&config.root, &FixOptions::default()) {
                Ok(report) => {
                    let _ = msg_tx.send(Msg::FixReportReady(Box::new(report)));
                }
                Err(e) => {
                    let _ = msg_tx.send(Msg::Error(e.to_string()));
                }
            },
            Command::ApplyFixFile(path) => {
                let result = okf_core::remediate_file(&path, &FixOptions::default())
                    .and_then(|report| {
                        if report.changed {
                            std::fs::write(&report.path, &report.remediated_content)?;
                        }
                        Ok(format!(
                            "✔ fixed {} issue(s) in {}",
                            report.remediations.len(),
                            report.path.display()
                        ))
                    })
                    .map_err(|e| e.to_string());
                let _ = msg_tx.send(Msg::Applied(result));
                reload(&mut generation, today, &mut snapshot);
            }
            Command::ApplyFix => {
                // Re-run so the applied fix reflects the current disk state,
                // then apply in one step.
                let result = remediate_bundle(&config.root, &FixOptions::default())
                    .and_then(|report| {
                        let total = report.total_remediations();
                        let (files, _) = report.apply()?;
                        Ok(format!("✔ fixed {total} issue(s) in {files} file(s)"))
                    })
                    .map_err(|e| e.to_string());
                let _ = msg_tx.send(Msg::Applied(result));
                reload(&mut generation, today, &mut snapshot);
            }
        }
    }
}

fn run_refactor(
    snapshot: &Snapshot,
    op: &RefactorOp,
    dry_run: bool,
    author: &str,
) -> Result<PreviewReport, RefactorError> {
    let bundle = &snapshot.bundle;
    let author = Some(author.to_string());
    match op {
        RefactorOp::Move {
            source,
            target,
            force,
        } => move_concept(
            bundle,
            source,
            target,
            &MoveOptions {
                dry_run,
                force: *force,
                author,
                ..MoveOptions::default()
            },
        )
        .map(PreviewReport::Move),
        RefactorOp::Remove {
            target,
            redirect_to,
            unlink,
            force,
        } => remove_concept(
            bundle,
            target,
            &RemoveOptions {
                dry_run,
                force: *force,
                redirect_to: redirect_to.clone(),
                unlink: *unlink,
                author,
                ..RemoveOptions::default()
            },
        )
        .map(PreviewReport::Remove),
        RefactorOp::Merge { source, target } => merge_concepts(
            bundle,
            source,
            target,
            &MergeOptions {
                dry_run,
                author,
                ..MergeOptions::default()
            },
        )
        .map(PreviewReport::Merge),
        RefactorOp::Split {
            source,
            target,
            section,
            title,
            force,
        } => split_concept(
            bundle,
            source,
            target,
            &SplitOptions {
                section: section.clone(),
                title: title.clone(),
                force: *force,
                dry_run,
                author,
                ..SplitOptions::default()
            },
        )
        .map(PreviewReport::Split),
        RefactorOp::RenameSection { concept, old, new } => rename_section(
            bundle,
            concept,
            old,
            new,
            &RenameSectionOptions {
                dry_run,
                update_log: true,
                author,
            },
        )
        .map(PreviewReport::RenameSection),
    }
}

fn toast_for(report: &PreviewReport) -> String {
    match report {
        PreviewReport::Move(r) => format!(
            "✔ renamed {} → {} ({} files)",
            r.source,
            r.target,
            r.affected_files.len()
        ),
        PreviewReport::Remove(r) => {
            format!("✔ removed {} ({} files)", r.target, r.affected_files.len())
        }
        PreviewReport::Merge(r) => format!(
            "✔ merged {} → {} ({} links)",
            r.source, r.target, r.rewritten_links_count
        ),
        PreviewReport::Split(r) => {
            format!("✔ split '{}' out of {} → {}", r.section, r.source, r.target)
        }
        PreviewReport::RenameSection(r) => format!(
            "✔ renamed section '{}' → '{}' in {}",
            r.old_section, r.new_section, r.concept
        ),
    }
}

/// Appends a `{ by, at }` verification event to a concept's `verified` list
/// via an order-preserving frontmatter round-trip, plus a log entry.
fn stamp_verification(config: &WorkerConfig, id: &okf_core::ConceptId) -> Result<String, String> {
    let path = id.to_path(&config.root);
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut doc = Document::parse(&text).map_err(|e| e.to_string())?;

    let mut event = Mapping::new();
    event.insert("by", Value::String(config.author.clone()));
    event.insert("at", Value::String(current_iso_timestamp()));
    let event = Value::Mapping(event);

    let new_value = match doc.frontmatter.get("verified").cloned() {
        Some(Value::Sequence(mut items)) => {
            items.push(event);
            Value::Sequence(items)
        }
        Some(existing @ Value::Mapping(_)) => Value::Sequence(vec![existing, event]),
        _ => Value::Sequence(vec![event]),
    };
    doc.frontmatter.set("verified", new_value);
    std::fs::write(&path, doc.serialize()).map_err(|e| e.to_string())?;

    let today = config.today.or_else(Date::today_utc).unwrap_or(Date {
        year: 2026,
        month: 1,
        day: 1,
    });
    let _ = append_log_entry(
        &config.root,
        today,
        "Update",
        &format!("Verified concept `{id}` (by {}).", config.author),
    );
    Ok(format!("✔ verified {id} (by {})", config.author))
}

/// Writes a new `stale_after` (normalized to an explicit-UTC datetime), plus
/// a log entry.
fn set_stale_after(
    config: &WorkerConfig,
    id: &okf_core::ConceptId,
    date: Date,
) -> Result<String, String> {
    let path = id.to_path(&config.root);
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let mut doc = Document::parse(&text).map_err(|e| e.to_string())?;
    doc.frontmatter
        .set("stale_after", Value::String(format!("{date}T00:00:00Z")));
    std::fs::write(&path, doc.serialize()).map_err(|e| e.to_string())?;

    let today = config.today.or_else(Date::today_utc).unwrap_or(Date {
        year: 2026,
        month: 1,
        day: 1,
    });
    let _ = append_log_entry(
        &config.root,
        today,
        "Update",
        &format!(
            "Extended `stale_after` of `{id}` to {date} (by {}).",
            config.author
        ),
    );
    Ok(format!("✔ {id} fresh until {date}"))
}
