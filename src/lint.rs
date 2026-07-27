//! Opinionated bundle health checks, beyond §11 conformance.
//!
//! [`validate_bundle`](crate::validate_bundle) enforces only the spec's hard
//! requirements and reports soft guidance as warnings. [`lint_bundle`] goes
//! further: it flags the hygiene issues a continuously-authored corpus drifts
//! into, such as orphan concepts no link or index points at, an `index.md`
//! that has fallen behind its directory, or a verification that predates the
//! last regeneration.
//!
//! Every finding is tagged with a stable rule code so CI can pin or
//! silence individual checks. None of them is a conformance failure: a bundle
//! with lint findings is still conformant if [`validate_bundle`](crate::validate_bundle) says so, which
//! is why `okf lint` is a separate command rather than a stricter
//! `okf validate`.
//!
//! | Code | Severity | Finding                                                   |
//! |------|----------|-----------------------------------------------------------|
//! | L1   | warning  | missing `title`                                           |
//! | L2   | warning  | missing `description`                                     |
//! | L3   | warning  | missing `generated` (and no legacy `timestamp`)           |
//! | L4   | info     | no `verified` events, trust tier is `unverified`          |
//! | L5   | warning  | legacy v0.1 `timestamp` present                           |
//! | L6   | warning  | legacy v0.1 body `# Citations` list present               |
//! | L7   | warning  | body is empty                                             |
//! | L8   | warning  | body has no top-level `#` heading                         |
//! | L9   | warning  | latest `verified.at` predates `generated.at`              |
//! | L10  | warning  | links to a `status: deprecated` concept                   |
//! | L11  | warning  | past `stale_after` (with `--today`)                       |
//! | L12  | info     | `status: draft`                                           |
//! | L13  | info     | self-link                                                 |
//! | L14  | warning  | `title` shared with another concept                       |
//! | L15  | warning  | orphan: no inbound links and not listed in any `index.md` |
//! | L16  | warning  | an existing `index.md` is out of sync with its directory  |

use crate::bundle::Bundle;
use crate::concept_id::ConceptId;
use crate::date::Date;
use crate::document::Document;
use crate::frontmatter::Frontmatter;
use crate::trust::Status;
use crate::validate::{Diagnostic, Report, Severity};
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

/// Lints a loaded bundle, returning all findings.
///
/// Deterministic: staleness is checked for *syntax* but not against the clock.
/// Use [`lint_bundle_at`] to also flag concepts past their `stale_after`.
#[must_use]
pub fn lint_bundle(bundle: &Bundle) -> Report {
    lint_bundle_at(bundle, None)
}

/// Lints a bundle, additionally flagging concepts that are stale on `today`.
#[must_use]
pub fn lint_bundle_at(bundle: &Bundle, today: Option<Date>) -> Report {
    let mut report = Report::default();

    let indexed = indexed_concepts(bundle);
    let title_counts = count_titles(bundle);

    for concept in bundle.concepts() {
        let mut cx = Cx {
            report: &mut report,
            path: concept.path.clone(),
            id: concept.id.clone(),
        };
        let doc = &concept.document;
        let fm = &doc.frontmatter;

        check_missing_title(&mut cx, fm);
        check_missing_description(&mut cx, fm);
        check_missing_generated(&mut cx, fm);
        check_unverified(&mut cx, fm);
        check_legacy(&mut cx, doc);
        check_empty_body(&mut cx, doc);
        check_top_heading(&mut cx, doc);
        check_verified_before_generated(&mut cx, fm);
        check_links_to_deprecated(&mut cx, bundle);
        check_staleness(&mut cx, fm, today);
        check_draft_status(&mut cx, fm);
        check_self_link(&mut cx, bundle);
        check_duplicate_title(&mut cx, fm, &title_counts);
    }

    check_orphans(bundle, &indexed, &mut report);
    check_stale_indexes(bundle, &mut report);

    report
}

/// The concepts an existing `index.md` lists, resolved across every index in
/// the bundle. Used by the orphan rule.
fn indexed_concepts(bundle: &Bundle) -> BTreeSet<ConceptId> {
    let mut out = BTreeSet::new();
    for index_path in bundle.index_files() {
        for (raw, target) in index_listed_targets(bundle, index_path) {
            if is_concept_link(&raw) && bundle.contains(&target) {
                out.insert(target);
            }
        }
    }
    out
}

/// Every link target an `index.md` lists, paired with the raw target as
/// written, resolved to a concept id whether or not that concept exists in the
/// bundle.
///
/// The raw target is returned alongside so callers can tell concept links from
/// resource links: `[sql_equality.py](sql_equality.py)` resolves to an id but
/// names a non-markdown resource, not a concept, so the stale-index rule skips
/// it.
fn index_listed_targets(bundle: &Bundle, index_path: &Path) -> Vec<(String, ConceptId)> {
    let mut out = Vec::new();
    let Some(source) = index_source_id(bundle.root(), index_path) else {
        return out;
    };
    let Ok(text) = fs::read_to_string(index_path) else {
        return out;
    };
    let Ok(doc) = Document::parse(&text) else {
        return out;
    };
    for link in doc.links() {
        for target in link.resolve_all(&source) {
            out.push((link.target.clone(), target));
        }
    }
    out
}

/// `true` when a raw link target names a concept (a `.md` file or a bare id)
/// rather than a non-markdown resource such as `attester.py`.
fn is_concept_link(raw: &str) -> bool {
    let t = raw.trim();
    if t.starts_with('#') || t.is_empty() {
        return false;
    }
    if crate::links::LinkKind::External == crate::links::Link::classify(t) {
        return false;
    }
    let before_anchor = t.split('#').next().unwrap_or(t);
    let basename = before_anchor.rsplit('/').next().unwrap_or(before_anchor);
    // OKF reserves the lowercase `index.md` and `log.md` filenames (§3.1), so a
    // case-sensitive comparison is correct here, not a missing-extension bug.
    #[allow(clippy::case_sensitive_file_extension_comparisons)]
    {
        basename.ends_with(".md") || !basename.contains('.')
    }
}

/// Synthesizes the concept id an `index.md` would have if it were itself a
/// concept, so [`Link::resolve_all`] can resolve its relative links against the
/// index's own directory.
///
/// For `<root>/index.md` this returns the one-segment id `index`, whose
/// [`ConceptId::parent`] is `None`, so relative links resolve from the bundle
/// root. For `<root>/computations/index.md` it returns `computations/index`,
/// whose parent is `computations`.
fn index_source_id(bundle_root: &Path, index_path: &Path) -> Option<ConceptId> {
    let rel = index_path.strip_prefix(bundle_root).ok()?;
    let mut segments: Vec<String> = rel
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(s) => Some(s.to_string_lossy().to_string()),
            _ => None,
        })
        .collect();
    if let Some(last) = segments.last_mut() {
        if let Some(stripped) = last.strip_suffix(".md") {
            *last = stripped.to_string();
        }
    }
    ConceptId::new(segments).ok()
}

/// Maps each `title` (as written) to the number of concepts that share it.
fn count_titles(bundle: &Bundle) -> HashMap<String, usize> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for c in bundle.concepts() {
        if let Some(title) = c.document.frontmatter.title() {
            *counts.entry(title.into_owned()).or_default() += 1;
        }
    }
    counts
}

/// The per-concept lint context, mirroring [`validate`](crate::validate)'s
/// `Context`: each rule can emit a diagnostic without repeating the path and
/// id, and every message is tagged with its rule code.
struct Cx<'a> {
    report: &'a mut Report,
    path: PathBuf,
    id: ConceptId,
}

impl Cx<'_> {
    fn warn(&mut self, code: &'static str, message: impl Into<String>) {
        self.push(Severity::Warning, code, message);
    }

    fn info(&mut self, code: &'static str, message: impl Into<String>) {
        self.push(Severity::Info, code, message);
    }

    fn push(&mut self, severity: Severity, code: &'static str, message: impl Into<String>) {
        self.report.diagnostics.push(Diagnostic {
            severity,
            path: Some(self.path.clone()),
            concept: Some(self.id.clone()),
            message: format!("[{code}] {}", message.into()),
        });
    }
}

fn check_missing_title(cx: &mut Cx, fm: &Frontmatter) {
    if fm.title().is_none() {
        cx.warn(
            "L1",
            "missing `title`; consumers fall back to the filename, but a human-readable \
             title is recommended",
        );
    }
}

fn check_missing_description(cx: &mut Cx, fm: &Frontmatter) {
    if fm.description().is_none() {
        cx.warn(
            "L2",
            "missing `description`; a one-line summary is recommended and \
             what `index.md` listings display",
        );
    }
}

fn check_missing_generated(cx: &mut Cx, fm: &Frontmatter) {
    let has_generated_key = fm.get("generated").is_some();
    let has_legacy_timestamp = fm.timestamp().is_some();
    if !has_generated_key && !has_legacy_timestamp {
        cx.warn(
            "L3",
            "missing `generated`; a continuously-authored corpus should record who \
             produced the content and when",
        );
    }
}

fn check_unverified(cx: &mut Cx, fm: &Frontmatter) {
    if fm.get("verified").is_none() {
        cx.info("L4", "no `verified` events; trust tier is `unverified`");
    }
}

fn check_legacy(cx: &mut Cx, doc: &Document) {
    if doc.frontmatter.timestamp().is_some() {
        cx.warn(
            "L5",
            "`timestamp` is a v0.1 key superseded by `generated: { by, at }`",
        );
    }
    if doc.has_legacy_citations() {
        cx.warn(
            "L6",
            "body `# Citations` list is superseded by `sources` + footnote attribution",
        );
    }
}

fn check_empty_body(cx: &mut Cx, doc: &Document) {
    if doc.body.trim().is_empty() {
        cx.warn(
            "L7",
            "body is empty; a concept should carry at least one line of prose or code",
        );
    }
}

fn check_top_heading(cx: &mut Cx, doc: &Document) {
    if doc.body.trim().is_empty() {
        return; // L7 already covers this
    }
    let has_top_heading = doc.body.lines().any(|l| l.trim_start().starts_with("# "));
    if !has_top_heading {
        cx.warn(
            "L8",
            "body has no top-level `#` heading; OKF docs conventionally open with one",
        );
    }
}

fn check_verified_before_generated(cx: &mut Cx, fm: &Frontmatter) {
    let Some(generated) = fm.generated() else {
        return;
    };
    let Some(generated_at) = generated.at.as_ref().and_then(|a| a.datetime) else {
        return;
    };
    let verified = fm.verified();
    let Some(latest) = crate::trust::latest_verification(&verified) else {
        return;
    };
    let Some(latest_at) = latest.at.as_ref().and_then(|a| a.datetime) else {
        return;
    };
    if latest_at < generated_at {
        cx.warn(
            "L9",
            format!(
                "latest verification ({latest_at}) predates `generated.at` ({generated_at}); \
                 the current content was never re-verified"
            ),
        );
    }
}

fn check_links_to_deprecated(cx: &mut Cx, bundle: &Bundle) {
    let mut warned: BTreeSet<ConceptId> = BTreeSet::new();
    for link in bundle.links_from(&cx.id) {
        if !link.exists || !warned.insert(link.target.clone()) {
            continue;
        }
        if let Some(target) = bundle.get(&link.target) {
            if target.status().is_deprecated() {
                cx.warn(
                    "L10",
                    format!("links to deprecated concept `{}`", link.target),
                );
            }
        }
    }
}

fn check_staleness(cx: &mut Cx, fm: &Frontmatter, today: Option<Date>) {
    let Some(today) = today else {
        return;
    };
    let Some(stale_after) = fm.stale_after().and_then(|d| d.effective_date()) else {
        return;
    };
    if today >= stale_after {
        cx.warn(
            "L11",
            format!("stale since {stale_after} (`stale_after` passed)"),
        );
    }
}

fn check_draft_status(cx: &mut Cx, fm: &Frontmatter) {
    if matches!(fm.status(), Status::Draft) {
        cx.info(
            "L12",
            "`status: draft`; a draft concept is not ready for production consumption",
        );
    }
}

fn check_self_link(cx: &mut Cx, bundle: &Bundle) {
    for link in bundle.links_from(&cx.id) {
        if link.exists && link.target == cx.id {
            cx.info(
                "L13",
                "self-link; a concept that links to itself usually signals a stray reference",
            );
            return;
        }
    }
}

fn check_duplicate_title(cx: &mut Cx, fm: &Frontmatter, counts: &HashMap<String, usize>) {
    let Some(title) = fm.title() else {
        return;
    };
    if counts.get(title.as_ref()).copied().unwrap_or(0) > 1 {
        cx.warn(
            "L14",
            format!("`title` {title:?} is shared with another concept; titles should disambiguate"),
        );
    }
}

fn check_orphans(bundle: &Bundle, indexed: &BTreeSet<ConceptId>, report: &mut Report) {
    for c in bundle.concepts() {
        let has_backlinks = !bundle.backlinks(&c.id).is_empty();
        let is_indexed = indexed.contains(&c.id);
        if !has_backlinks && !is_indexed {
            report.diagnostics.push(Diagnostic {
                severity: Severity::Warning,
                path: Some(c.path.clone()),
                concept: Some(c.id.clone()),
                message: "[L15] orphan concept: no other concept links to it and no \
                          `index.md` lists it"
                    .to_string(),
            });
        }
    }
}

fn check_stale_indexes(bundle: &Bundle, report: &mut Report) {
    for index_path in bundle.index_files() {
        let Some(dir) = index_path.parent() else {
            continue;
        };
        let Some(index_id) = index_source_id(bundle.root(), index_path) else {
            continue;
        };
        // `None` for the root index (its directory is the bundle root, which
        // has no parent in concept-id space); `Some(dir)` for a sub-index.
        let index_dir = index_id.parent();

        let actual: BTreeSet<ConceptId> = bundle
            .concepts()
            .iter()
            .filter(|c| c.path.parent() == Some(dir))
            .map(|c| c.id.clone())
            .collect();

        // Only links that resolve into this index's own directory count: an
        // absolute link from the root index to `/computations/revenue.md` is
        // navigation elsewhere, not a row this directory's index is supposed
        // to list, so it would be noise to flag against the root's concepts.
        // Resource links (e.g. `attester.py`) are also skipped: an index may
        // legitimately list non-markdown files alongside its concepts.
        let listed: BTreeSet<ConceptId> = index_listed_targets(bundle, index_path)
            .into_iter()
            .filter(|(raw, _)| is_concept_link(raw))
            .map(|(_, target)| target)
            .filter(|t| t.parent() == index_dir)
            .collect();

        let missing_from_index: Vec<String> = actual
            .iter()
            .filter(|c| !listed.contains(*c))
            .map(ConceptId::to_string)
            .collect();
        let listed_but_not_on_disk: Vec<String> = listed
            .iter()
            .filter(|c| !actual.contains(*c))
            .map(ConceptId::to_string)
            .collect();

        if missing_from_index.is_empty() && listed_but_not_on_disk.is_empty() {
            continue;
        }

        let mut parts = Vec::new();
        if !missing_from_index.is_empty() {
            parts.push(format!(
                "missing from index: {}",
                missing_from_index.join(", ")
            ));
        }
        if !listed_but_not_on_disk.is_empty() {
            parts.push(format!(
                "listed but not on disk: {}",
                listed_but_not_on_disk.join(", ")
            ));
        }

        report.diagnostics.push(Diagnostic {
            severity: Severity::Warning,
            path: Some(index_path.clone()),
            concept: None,
            message: format!(
                "[L16] index.md is out of sync with its directory ({})",
                parts.join("; ")
            ),
        });
    }
}
