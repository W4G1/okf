//! Opinionated bundle health checks, beyond conformance.
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
//! | Code | Severity | Finding                                                            |
//! |------|----------|--------------------------------------------------------------------|
//! | L1   | warning  | missing `title`                                                    |
//! | L2   | warning  | missing `description`                                              |
//! | L3   | warning  | missing `generated` (and no legacy `timestamp`)                    |
//! | L4   | info     | no `verified` events, trust tier is `unverified`                   |
//! | L5   | warning  | legacy v0.1 `timestamp` present                                    |
//! | L6   | warning  | legacy v0.1 body `# Citations` list present                        |
//! | L7   | warning  | body is empty                                                      |
//! | L8   | warning  | body has no top-level `#` heading                                  |
//! | L9   | warning  | latest `verified.at` predates `generated.at`                       |
//! | L10  | warning  | links to a `status: deprecated` concept                            |
//! | L11  | warning  | past `stale_after` (with `--today`)                                |
//! | L12  | info     | `status: draft`                                                    |
//! | L13  | info     | self-link                                                          |
//! | L14  | warning  | `title` shared with another concept                                |
//! | L15  | warning  | orphan: no inbound links and not listed in any `index.md`          |
//! | L16  | warning  | an existing `index.md` is out of sync with its directory           |
//! | L17  | warning  | broken link to a concept that does not exist                       |
//! | L18  | info     | frontmatter keys not in canonical preferred order                  |
//! | L19  | warning  | heading hierarchy drift (heading levels skipped or multiple `#`)   |
//! | L20  | warning  | empty / stub section heading with no content                       |
//! | L21  | info     | source declared in frontmatter but never cited with footnote       |
//! | L22  | warning  | circular concept derivation in sources graph                       |
//! | L23  | info     | non-standard actor identity in generated, verified, or author      |
//! | L24  | warning  | timestamp in generated, verified, or sources is in the future      |
//! | L25  | warning  | `executor`, `attester`, or `computation` resource missing on disk  |
//! | L26  | warning  | `# Computation` code block missing language tag                    |
//! | L27  | warning  | duplicate date heading in `log.md`                                 |
//! | L28  | info     | trailing whitespace or excess blank lines in markdown body         |

use crate::validate::{Diagnostic, Report, Severity};
use okf_core::bundle::Bundle;
use okf_core::concept_id::ConceptId;
use okf_core::date::Date;
use okf_core::document::Document;
use okf_core::frontmatter::Frontmatter;
use okf_core::trust::Status;
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
        check_broken_links(&mut cx, bundle);
        check_duplicate_title(&mut cx, fm, &title_counts);
        check_key_order(&mut cx, fm);
        check_heading_hierarchy(&mut cx, doc);
        check_empty_headings(&mut cx, doc);
        check_unused_sources(&mut cx, doc);
        check_non_standard_actor(&mut cx, fm);
        check_future_timestamps(&mut cx, fm, today);
        check_attestation_resources(&mut cx, bundle, doc);
        check_computation_block_formatting(&mut cx, doc);
        check_whitespace(&mut cx, doc);
    }

    check_orphans(bundle, &indexed, &mut report);
    check_stale_indexes(bundle, &mut report);
    check_circular_derivation(bundle, &mut report);
    check_duplicate_log_dates(bundle, &mut report);

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
    if okf_core::links::LinkKind::External == okf_core::links::Link::classify(t) {
        return false;
    }
    let before_anchor = t.split('#').next().unwrap_or(t);
    let basename = before_anchor.rsplit('/').next().unwrap_or(before_anchor);
    if okf_core::bundle::RESERVED_FILENAMES.contains(&basename) {
        return false;
    }
    // OKF reserves the lowercase `index.md` and `log.md` filenames, so a
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
    if let Some(last) = segments.last_mut()
        && let Some(stripped) = last.strip_suffix(".md")
    {
        *last = stripped.to_string();
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
    matches!(
        code.as_bytes(),
        b"L1" | b"L3" | b"L5" | b"L6" | b"L8" | b"L16" | b"L18" | b"L26" | b"L27" | b"L28"
    )
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
    let Some(latest) = okf_core::trust::latest_verification(&verified) else {
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
        if let Some(target) = bundle.get(&link.target)
            && target.status().is_deprecated()
        {
            cx.warn(
                "L10",
                format!("links to deprecated concept `{}`", link.target),
            );
        }
    }
}

fn check_staleness(cx: &mut Cx, fm: &Frontmatter, today: Option<Date>) {
    let Some(today) = today else {
        return;
    };
    let Some(stale_after) = fm.stale_after() else {
        return;
    };
    if fm.is_stale_on(today) {
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

fn check_broken_links(cx: &mut Cx, bundle: &Bundle) {
    for link in bundle.links_from(&cx.id) {
        if !link.exists {
            cx.warn(
                "L17",
                format!(
                    "broken link to `{}` (target concept does not exist)",
                    link.raw
                ),
            );
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
                fixable: false,
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
            fixable: true,
        });
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
                    "L18",
                    "frontmatter keys are not in canonical order (run `okf fmt` to normalize)",
                );
                return;
            }
            last_rank = Some(rank);
        }
    }
}

struct MarkdownHeading<'a> {
    level: usize,
    text: &'a str,
    line_num: usize,
}

fn parse_heading_line(line: &str) -> Option<(usize, &str)> {
    let t = line.trim_start();
    if !t.starts_with('#') {
        return None;
    }
    let count = t.chars().take_while(|&c| c == '#').count();
    if (1..=6).contains(&count) && t[count..].starts_with(' ') {
        Some((count, t[count..].trim()))
    } else {
        None
    }
}

fn parse_markdown_headings(body: &str) -> Vec<MarkdownHeading<'_>> {
    let mut headings = Vec::new();
    let mut in_code_block = false;

    for (i, line) in body.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_code_block = !in_code_block;
            continue;
        }
        if in_code_block {
            continue;
        }
        if let Some((level, text)) = parse_heading_line(trimmed) {
            headings.push(MarkdownHeading {
                level,
                text,
                line_num: i + 1,
            });
        }
    }
    headings
}

fn check_heading_hierarchy(cx: &mut Cx, doc: &Document) {
    let headings = parse_markdown_headings(&doc.body);
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
                    "L19",
                    format!(
                        "multiple top-level `#` headings found (heading `{}` at line {})",
                        h.text, h.line_num
                    ),
                );
            }
        }

        if prev_level > 0 && h.level > prev_level + 1 {
            cx.warn(
                "L19",
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
    let mut in_code_block = false;

    let mut headings_with_line: Vec<(usize, usize, &str)> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_code_block = !in_code_block;
            continue;
        }
        if in_code_block {
            continue;
        }
        if let Some((level, text)) = parse_heading_line(trimmed) {
            headings_with_line.push((i, level, text));
        }
    }

    for (k, &(line_idx, level, text)) in headings_with_line.iter().enumerate() {
        let next_heading = headings_with_line
            .get(k + 1)
            .map(|&(next_idx, next_level, _)| (next_idx, next_level));

        let content_end = match next_heading {
            Some((next_idx, next_level)) => {
                if next_level > level {
                    continue;
                }
                next_idx
            }
            None => lines.len(),
        };

        let has_content = (line_idx + 1..content_end).any(|idx| {
            let l = lines[idx].trim();
            !l.is_empty() && !l.starts_with("<!--")
        });

        if !has_content {
            cx.warn("L20", format!("heading `{text}` has no content"));
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
                    "L21",
                    format!(
                        "source `{id}` is declared in frontmatter but never cited with footnote `[^{id}]`",
                    ),
                );
            }
        }
    }
}

fn check_circular_derivation(bundle: &Bundle, report: &mut Report) {
    let mut warned: BTreeSet<ConceptId> = BTreeSet::new();

    for concept in bundle.concepts() {
        if warned.contains(&concept.id) {
            continue;
        }
        let mut path = Vec::new();
        let mut visited = BTreeSet::new();

        if find_derivation_cycle(bundle, &concept.id, &mut path, &mut visited) {
            for id in &path {
                warned.insert(id.clone());
            }
            let cycle_str: Vec<String> = path.iter().map(ToString::to_string).collect();
            report.diagnostics.push(Diagnostic {
                severity: Severity::Warning,
                path: Some(concept.path.clone()),
                concept: Some(concept.id.clone()),
                message: format!(
                    "[L22] circular concept derivation: {}",
                    cycle_str.join(" ~> ")
                ),
                fixable: false,
            });
        }
    }
}

fn find_derivation_cycle(
    bundle: &Bundle,
    current: &ConceptId,
    path: &mut Vec<ConceptId>,
    visited: &mut BTreeSet<ConceptId>,
) -> bool {
    path.push(current.clone());
    visited.insert(current.clone());

    for next in bundle.derived_from(current) {
        if path.first() == Some(next) || path.contains(next) {
            path.push((*next).clone());
            return true;
        }
        if !visited.contains(next) && find_derivation_cycle(bundle, next, path, visited) {
            return true;
        }
    }

    path.pop();
    false
}

fn check_non_standard_actor(cx: &mut Cx, fm: &Frontmatter) {
    if let Some(generated) = fm.generated()
        && let Some(by) = &generated.by
        && matches!(by.kind(), okf_core::ActorKind::Other)
    {
        cx.info(
            "L23",
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
                "L23",
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
                "L23",
                format!(
                    "author `{author}` in `sources.author` does not follow the standard `human:<id>`, `process:<id>`, or `<producer>/<version>` convention"
                ),
            );
        }
    }
}

fn check_future_timestamps(cx: &mut Cx, fm: &Frontmatter, today: Option<Date>) {
    let check_date = today.or_else(Date::today_utc);
    let Some(check_date) = check_date else {
        return;
    };
    let threshold_seconds = (check_date.days_since_epoch() + 1) * 86_400;

    if let Some(generated) = fm.generated()
        && let Some(dt) = generated.at.as_ref().and_then(|a| a.datetime)
        && dt.to_utc_seconds() > threshold_seconds
    {
        cx.warn(
            "L24",
            format!("`generated.at` timestamp `{dt}` is in the future"),
        );
    }
    for verification in fm.verified() {
        if let Some(dt) = verification.at.as_ref().and_then(|a| a.datetime)
            && dt.to_utc_seconds() > threshold_seconds
        {
            cx.warn(
                "L24",
                format!("`verified.at` timestamp `{dt}` is in the future"),
            );
        }
    }
    for source in fm.sources() {
        if let Some(dt) = source.last_modified.as_ref().and_then(|a| a.datetime)
            && dt.to_utc_seconds() > threshold_seconds
        {
            cx.warn(
                "L24",
                format!("`sources.last_modified` timestamp `{dt}` is in the future"),
            );
        }
    }
}

fn check_attestation_resources(cx: &mut Cx, bundle: &Bundle, doc: &Document) {
    let Some(contract) = doc.attested_computation() else {
        return;
    };
    if let Some(executor) = &contract.executor
        && let Some(res) = &executor.resource
        && !res.starts_with("http://")
        && !res.starts_with("https://")
        && bundle.resolve_path_field(&cx.id, res).is_none()
    {
        cx.warn(
            "L25",
            format!("`executor.resource` points to `{res}` which does not exist on disk"),
        );
    }
    if let Some(attester) = &contract.attester
        && let Some(res) = &attester.resource
        && !res.starts_with("http://")
        && !res.starts_with("https://")
        && bundle.resolve_path_field(&cx.id, res).is_none()
    {
        cx.warn(
            "L25",
            format!("`attester.resource` points to `{res}` which does not exist on disk"),
        );
    }
    if let okf_core::computation::ComputationSource::File(path) = &contract.computation
        && !path.starts_with("http://")
        && !path.starts_with("https://")
        && bundle.resolve_path_field(&cx.id, path).is_none()
    {
        cx.warn(
            "L25",
            format!("`computation` file `{path}` does not exist on disk"),
        );
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
            "L26",
            "`# Computation` code block is missing a syntax language tag (e.g. ` ```python ` or ` ```sql `)",
        );
    }
}

fn check_duplicate_log_dates(bundle: &Bundle, report: &mut Report) {
    for log_path in bundle.log_files() {
        let Ok(text) = fs::read_to_string(log_path) else {
            continue;
        };
        let log = okf_core::log::Log::parse(&text);
        let mut seen = HashMap::new();
        for day in &log.days {
            *seen.entry(day.date.clone()).or_insert(0) += 1;
        }
        for (date, count) in seen {
            if count > 1 {
                report.diagnostics.push(Diagnostic {
                    severity: Severity::Warning,
                    path: Some(log_path.clone()),
                    concept: None,
                    message: format!(
                        "[L27] log.md contains duplicate date heading `## {date}` (entries should be grouped under a single heading)"
                    ),
                    fixable: true,
                });
            }
        }
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
            "L28",
            format!(
                "trailing whitespace found on {trailing_count} line(s) in markdown body (first at line {first_trailing_line})"
            ),
        );
    } else if excess_blank || has_trailing_blank_lines {
        cx.info(
            "L28",
            "excess consecutive blank lines found in markdown body",
        );
    }
}
