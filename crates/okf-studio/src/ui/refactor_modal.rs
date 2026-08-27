//! The refactor modal: intent → decision (only on a genuine conflict) →
//! dry-run preview. One overlay for all five verbs.

use crate::app::{App, PreviewReport, RefactorState, RemoveChoice, VerbKind};
use crate::markdown::truncate_to_width;
use crate::theme::GLYPH_BROKEN;
use crate::ui::widgets::{centered, input_line};
use okf_core::RefactorError;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

/// Draws the refactor modal.
#[allow(clippy::too_many_lines)]
pub fn draw(frame: &mut Frame, app: &App, body: Rect, state: &RefactorState) {
    let theme = &app.theme;
    let title = match state.verb {
        VerbKind::Move => format!(" Move {} → … ", state.subject),
        VerbKind::Remove => format!(" Remove {} ", state.subject),
        VerbKind::Merge => format!(" Merge {} into … ", state.subject),
        VerbKind::Split => format!(
            " Split '{}' out of {} ",
            state.section.clone().unwrap_or_default(),
            state.subject
        ),
        VerbKind::RenameSection => format!(
            " Rename section '{}' in {} ",
            state.section.clone().unwrap_or_default(),
            state.subject
        ),
    };
    let area = centered(body, 66, 16);
    let ready = matches!(state.preview, Some(Ok(_)));
    let bottom = if ready {
        " ⏎ Apply   Esc Cancel "
    } else {
        " Esc Cancel "
    };
    let block = Block::new()
        .borders(Borders::ALL)
        .title(truncate_to_width(
            &title,
            usize::from(area.width).saturating_sub(2),
        ))
        .title_bottom(bottom)
        .border_style(theme.accent());
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);

    let width = usize::from(inner.width).saturating_sub(2);
    let mut lines: Vec<Line<'static>> = Vec::new();

    // Intent inputs.
    match state.verb {
        VerbKind::Move => {
            lines.push(input_line(
                "target id",
                &state.input,
                state.field == 0,
                theme,
                width,
            ));
            if okf_core::ConceptId::parse(state.input.trim()).is_err() {
                lines.push(Line::from(Span::styled(
                    format!("  {GLYPH_BROKEN} not a valid concept id"),
                    theme.error(),
                )));
            }
        }
        VerbKind::Merge => {
            lines.push(input_line(
                "merge into",
                &state.input,
                state.field == 0,
                theme,
                width,
            ));
            suggest(app, &state.input, &mut lines);
        }
        VerbKind::Split => {
            lines.push(input_line(
                "new id  ",
                &state.input,
                state.field == 0,
                theme,
                width,
            ));
            lines.push(input_line(
                "title   ",
                &state.extra,
                state.field == 1,
                theme,
                width,
            ));
        }
        VerbKind::RenameSection => {
            lines.push(input_line(
                "new heading",
                &state.input,
                state.field == 0,
                theme,
                width,
            ));
        }
        VerbKind::Remove => {}
    }

    // Decision stage: only when the engine raised a genuine question.
    match &state.preview {
        Some(Err(RefactorError::HasInboundLinks {
            inbound_count,
            inbound_concepts,
            ..
        })) if state.verb == VerbKind::Remove => {
            lines.push(Line::from(Span::styled(
                format!("✋ {inbound_count} concept(s) still link here:"),
                theme.warn().add_modifier(Modifier::BOLD),
            )));
            let joined = inbound_concepts
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(" · ");
            lines.push(Line::from(Span::styled(
                format!("   {}", truncate_to_width(&joined, width.saturating_sub(3))),
                theme.dim(),
            )));
            lines.push(Line::default());
            let choice_row = |key: &str, label: &str, active: bool| {
                let style = if active {
                    theme.accent().add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                Line::from(vec![
                    Span::styled(format!(" {} ", if active { "▸" } else { " " }), style),
                    Span::styled(format!("({key}) "), theme.accent()),
                    Span::styled(label.to_string(), style),
                ])
            };
            lines.push(choice_row(
                "r",
                "Redirect links to…",
                state.choice == RemoveChoice::Redirect,
            ));
            if state.choice == RemoveChoice::Redirect {
                lines.push(input_line(
                    "    target",
                    &state.extra,
                    state.field == 1,
                    theme,
                    width,
                ));
            }
            lines.push(choice_row(
                "u",
                "Unlink into plain text",
                state.choice == RemoveChoice::Unlink,
            ));
            lines.push(choice_row(
                "f",
                format!("Force — leave {inbound_count} broken link(s)").as_str(),
                state.choice == RemoveChoice::Force,
            ));
        }
        Some(Err(RefactorError::ConceptAlreadyExists(id)))
            if matches!(state.verb, VerbKind::Move | VerbKind::Split) =>
        {
            lines.push(Line::from(Span::styled(
                format!("✋ {id} already exists — press o to overwrite"),
                theme.warn(),
            )));
        }
        Some(Err(error)) => {
            lines.push(Line::from(Span::styled(
                truncate_to_width(&format!("{GLYPH_BROKEN} {error}"), width),
                theme.error(),
            )));
        }
        _ => {}
    }

    // Preview stage.
    lines.push(Line::default());
    match &state.preview {
        Some(Ok(report)) => {
            lines.push(Line::from(Span::styled(
                "PREVIEW (dry-run)",
                theme.accent().add_modifier(Modifier::BOLD),
            )));
            for line in preview_lines(report) {
                lines.push(Line::from(Span::raw(format!(
                    "  {}",
                    truncate_to_width(&line, width.saturating_sub(2))
                ))));
            }
        }
        None if state.needs_preview => {
            lines.push(Line::from(Span::styled("… previewing", theme.dim())));
        }
        _ => {}
    }

    let visible: Vec<Line<'static>> = lines.into_iter().take(usize::from(inner.height)).collect();
    frame.render_widget(Paragraph::new(visible), inner);
}

/// Fuzzy suggestions under the merge-target input.
fn suggest(app: &App, query: &str, lines: &mut Vec<Line<'static>>) {
    let Some(snapshot) = &app.snapshot else {
        return;
    };
    if query.trim().is_empty() {
        return;
    }
    let hits = snapshot.search.search(query.trim(), 3);
    for hit in hits {
        if hit.heading.is_none() {
            lines.push(Line::from(Span::styled(
                format!("    ≈ {}", hit.id),
                app.theme.dim(),
            )));
        }
    }
}

/// The dry-run report, as human lines (straight off the `*Report` types).
fn preview_lines(report: &PreviewReport) -> Vec<String> {
    match report {
        PreviewReport::Move(r) => vec![
            format!(
                "✎ {} incoming link(s) rewritten · {} outgoing rebased",
                r.rewritten_incoming_links, r.rebased_outgoing_links
            ),
            format!(
                "✎ {} frontmatter path(s) rebased",
                r.rebased_frontmatter_paths
            ),
            format!(
                "R {} → {}",
                r.source_path.display(),
                r.target_path.display()
            ),
            format!("{} file(s) affected", r.affected_files.len()),
        ],
        PreviewReport::Remove(r) => {
            let mut out = vec![format!("would remove {}", r.removed_path.display())];
            if let Some(redirect) = &r.redirected_to {
                out.push(format!(
                    "✎ {} link(s) redirected to {redirect}",
                    r.redirected_count
                ));
            }
            if r.unlinked_count > 0 {
                out.push(format!("✎ {} link(s) unlinked", r.unlinked_count));
            }
            out.push(format!(
                "{} file(s) affected · index + log updated",
                r.affected_files.len()
            ));
            out
        }
        PreviewReport::Merge(r) => vec![
            format!("✎ {} incoming link(s) rewritten", r.rewritten_links_count),
            format!("✎ {} source(s) merged", r.merged_sources_count),
            format!("would remove {}", r.removed_path.display()),
            format!("would update {}", r.updated_path.display()),
        ],
        PreviewReport::Split(r) => vec![
            format!("would create {}", r.target_path.display()),
            format!(
                "✂ {} line(s) extracted · {} source/footnote(s) moved",
                r.extracted_lines_count, r.moved_sources_count
            ),
            format!("{} file(s) affected", r.affected_files.len()),
        ],
        PreviewReport::RenameSection(r) => vec![
            format!("'{}' → '{}'", r.old_section, r.new_section),
            format!("#{} → #{}", r.old_slug, r.new_slug),
            format!(
                "✎ {} internal · {} external link(s) updated",
                r.internal_links_updated, r.external_links_updated
            ),
        ],
    }
}
