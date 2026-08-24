//! Opinionated bundle health checks, beyond conformance and validation.
//!
//! [`validate_bundle`](crate::validate_bundle) enforces the spec's hard
//! conformance requirements and reports material deviations, data integrity
//! problems, broken links, temporal inconsistencies, and contract issues as
//! validation warnings. [`lint_bundle`] goes further into opinionated
//! authoring hygiene, markdown prose layout, code-block syntax tagging,
//! and whitespace formatting.
//!
//! Every finding is tagged with a stable rule code (`L1`..`L12`) so CI can pin or
//! silence individual checks. None of them is a conformance failure: a bundle
//! with lint findings is still conformant if [`validate_bundle`](crate::validate_bundle) says so, which
//! is why `okf lint` is a separate command rather than a stricter
//! `okf validate`.
//!
//! | Code | Severity | Finding                                                            |
//! |------|----------|--------------------------------------------------------------------|
//! | L1   | warning  | body has no top-level `#` heading                                  |
//! | L2   | info     | frontmatter keys not in canonical preferred order                  |
//! | L3   | warning  | heading hierarchy drift (heading levels skipped or multiple `#`)   |
//! | L4   | warning  | empty / stub section heading with no content                       |
//! | L5   | info     | source declared in frontmatter but never cited with footnote       |
//! | L6   | info     | non-standard actor identity in generated, verified, or author      |
//! | L7   | warning  | `# Computation` code block missing language tag                    |
//! | L8   | info     | trailing whitespace or excess blank lines in markdown body         |
//! | L9   | warning  | orphan: no inbound links and not listed in any `index.md`          |
//! | L10  | info     | self-link (concept links to itself)                                |
//! | L11  | info     | no `verified` events, trust tier is `unverified`                   |
//! | L12  | info     | `status: draft`                                                    |

use crate::validate::{Diagnostic, Report, Severity, index_listed_targets, is_concept_link};
use okf_core::bundle::Bundle;
use okf_core::concept_id::ConceptId;
use okf_core::date::Date;
use okf_core::document::Document;
use okf_core::frontmatter::Frontmatter;
use okf_core::trust::Status;
use std::collections::BTreeSet;
use std::path::PathBuf;

/// Lints a loaded bundle, returning all findings.
///
/// Deterministic: staleness and temporal checks are handled during validation.
/// Use [`lint_bundle_at`] for consistency with the validator API.
#[must_use]
pub fn lint_bundle(bundle: &Bundle) -> Report {
    lint_bundle_at(bundle, None)
}

/// Lints a bundle, returning all opinionated formatting and style findings.
#[must_use]
pub fn lint_bundle_at(bundle: &Bundle, _today: Option<Date>) -> Report {
    let mut report = Report::default();

    let indexed = indexed_concepts(bundle);

    for concept in bundle.concepts() {
        let mut cx = Cx {
            report: &mut report,
            path: concept.path.clone(),
            id: concept.id.clone(),
        };
        let doc = &concept.document;
        let fm = &doc.frontmatter;

        check_top_heading(&mut cx, doc);
        check_key_order(&mut cx, fm);
        check_heading_hierarchy(&mut cx, doc);
        check_empty_headings(&mut cx, doc);
        check_unused_sources(&mut cx, doc);
        check_non_standard_actor(&mut cx, fm);
        check_computation_block_formatting(&mut cx, doc);
        check_whitespace(&mut cx, doc);
        check_self_link(&mut cx, bundle);
        check_unverified(&mut cx, fm);
        check_draft_status(&mut cx, fm);
    }

    check_orphans(bundle, &indexed, &mut report);

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
        let fixable = is_fixable_lint(code);
        self.report.diagnostics.push(Diagnostic {
            severity,
            path: Some(self.path.clone()),
            concept: Some(self.id.clone()),
            message: format!("[{code}] {}", message.into()),
            fixable,
        });
    }
}

/// Whether a lint finding can be automatically remediated by `okf fix`.
const fn is_fixable_lint(code: &str) -> bool {
    matches!(code.as_bytes(), b"L1" | b"L2" | b"L7" | b"L8")
}

fn check_unverified(cx: &mut Cx, fm: &Frontmatter) {
    if fm.get("verified").is_none() {
        cx.info("L11", "no `verified` events; trust tier is `unverified`");
    }
}

fn check_top_heading(cx: &mut Cx, doc: &Document) {
    if doc.body.trim().is_empty() {
        return;
    }
    let has_top_heading = doc.body.lines().any(|l| l.trim_start().starts_with("# "));
    if !has_top_heading {
        cx.warn(
            "L1",
            "body has no top-level `#` heading; OKF docs conventionally open with one",
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
                "L10",
                "self-link; a concept that links to itself usually signals a stray reference",
            );
            return;
        }
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
                message: "[L9] orphan concept: no other concept links to it and no \
                          `index.md` lists it"
                    .to_string(),
                fixable: false,
            });
        }
    }
}

fn check_key_order(cx: &mut Cx, fm: &Frontmatter) {
    let keys: Vec<&str> = fm.as_mapping().keys().collect();
    if keys.len() < 2 {
        return;
    }
    let mut last_rank = None;
    for key in keys {
        if let Some(rank) = okf_core::frontmatter::PREFERRED_KEY_ORDER
            .iter()
            .position(|&k| k == key)
        {
            if let Some(prev) = last_rank
                && rank < prev
            {
                cx.info(
                    "L2",
                    "frontmatter keys are not in canonical order (run `okf fmt` to normalize)",
                );
                return;
            }
            last_rank = Some(rank);
        }
    }
}

fn check_heading_hierarchy(cx: &mut Cx, doc: &Document) {
    let headings = okf_core::extract_headings(&doc.body);
    if headings.is_empty() {
        return;
    }

    let is_attested = doc.frontmatter.is_attested_computation();
    let mut h1_count = 0;
    let mut prev_level = 0;

    for h in &headings {
        if h.level == 1 {
            h1_count += 1;
            if h1_count > 1 && !(is_attested && h1_count == 2 && h.text == "Computation") {
                cx.warn(
                    "L3",
                    format!(
                        "multiple top-level `#` headings found (heading `{}` at line {})",
                        h.text, h.line_num
                    ),
                );
            }
        }

        if prev_level > 0 && h.level > prev_level + 1 {
            cx.warn(
                "L3",
                format!(
                    "heading level skipped: `{}` jumps from h{prev_level} to h{}",
                    h.text, h.level
                ),
            );
        }
        prev_level = h.level;
    }
}

fn check_empty_headings(cx: &mut Cx, doc: &Document) {
    let lines: Vec<&str> = doc.body.lines().collect();
    let headings = okf_core::extract_headings(&doc.body);

    for (k, h) in headings.iter().enumerate() {
        let content_end = match headings.get(k + 1) {
            Some(next_h) => {
                if next_h.level > h.level {
                    continue;
                }
                next_h.line_index
            }
            None => lines.len(),
        };

        let has_content = (h.line_index + 1..content_end).any(|idx| {
            let l = lines[idx].trim();
            !l.is_empty() && !l.starts_with("<!--")
        });

        if !has_content {
            cx.warn("L4", format!("heading `{}` has no content", h.text));
        }
    }
}

fn check_unused_sources(cx: &mut Cx, doc: &Document) {
    let sources = doc.frontmatter.sources();
    if sources.is_empty() {
        return;
    }
    let attributions = doc.attributions();
    for source in sources {
        if let Some(id) = &source.id {
            let is_cited = attributions.iter().any(|a| a.label == *id)
                || doc.body.contains(&format!("[^{id}]"));
            if !is_cited {
                cx.info(
                    "L5",
                    format!(
                        "source `{id}` is declared in frontmatter but never cited with footnote `[^{id}]`",
                    ),
                );
            }
        }
    }
}

fn check_non_standard_actor(cx: &mut Cx, fm: &Frontmatter) {
    if let Some(generated) = fm.generated()
        && let Some(by) = &generated.by
        && matches!(by.kind(), okf_core::ActorKind::Other)
    {
        cx.info(
            "L6",
            format!(
                "actor `{by}` in `generated.by` does not follow the standard `human:<id>`, `process:<id>`, or `<producer>/<version>` convention"
            ),
        );
    }
    for verification in fm.verified() {
        if let Some(by) = &verification.by
            && matches!(by.kind(), okf_core::ActorKind::Other)
        {
            cx.info(
                "L6",
                format!(
                    "actor `{by}` in `verified.by` does not follow the standard `human:<id>`, `process:<id>`, or `<producer>/<version>` convention"
                ),
            );
        }
    }
    for source in fm.sources() {
        if let Some(author) = &source.author
            && matches!(author.kind(), okf_core::ActorKind::Other)
        {
            cx.info(
                "L6",
                format!(
                    "author `{author}` in `sources.author` does not follow the standard `human:<id>`, `process:<id>`, or `<producer>/<version>` convention"
                ),
            );
        }
    }
}

fn check_computation_block_formatting(cx: &mut Cx, doc: &Document) {
    let Some(contract) = doc.attested_computation() else {
        return;
    };
    if let okf_core::computation::ComputationSource::Inline(inline) = &contract.computation
        && inline.language.is_none()
    {
        cx.warn(
            "L7",
            "`# Computation` code block is missing a syntax language tag (e.g. ` ```python ` or ` ```sql `)",
        );
    }
}

fn check_whitespace(cx: &mut Cx, doc: &Document) {
    let mut trailing_count = 0;
    let mut first_trailing_line = 0;
    let mut excess_blank = false;
    let mut consecutive_blank = 0;

    for (i, line) in doc.body.lines().enumerate() {
        if line.ends_with(char::is_whitespace) {
            trailing_count += 1;
            if first_trailing_line == 0 {
                first_trailing_line = i + 1;
            }
        }
        if line.trim().is_empty() {
            consecutive_blank += 1;
            if consecutive_blank > 2 {
                excess_blank = true;
            }
        } else {
            consecutive_blank = 0;
        }
    }

    let body_trimmed = doc.body.trim_end_matches(['\n', '\r']);
    let has_trailing_blank_lines = doc.body.len() > body_trimmed.len() + 1
        && doc.body[body_trimmed.len()..]
            .chars()
            .filter(|&c| c == '\n')
            .count()
            > 2;

    if trailing_count > 0 {
        cx.info(
            "L8",
            format!(
                "trailing whitespace found on {trailing_count} line(s) in markdown body (first at line {first_trailing_line})"
            ),
        );
    } else if excess_blank || has_trailing_blank_lines {
        cx.info(
            "L8",
            "excess consecutive blank lines found in markdown body",
        );
    }
}
