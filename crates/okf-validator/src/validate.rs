//! Conformance checking and structural validation against OKF v0.2.
//!
//! A bundle is **conformant** if (1) every non-reserved `.md` file has a
//! parseable frontmatter block, (2) every frontmatter has a non-empty `type`,
//! and (3) reserved files follow their structure when present. Everything else
//! is soft guidance: consumers MUST NOT reject a bundle for missing optional
//! fields, unknown types or keys, broken links, or missing `index.md` files.
//!
//! Accordingly, [`validate_bundle`] reports only true conformance violations as
//! [`Severity::Error`]. Material soft-guidance deviations, data integrity issues,
//! temporal inconsistencies, contract discrepancies, and syntax errors are
//! reported as [`Severity::Warning`] (producer mistakes worth fixing)
//! or [`Severity::Info`] (permitted states worth noting).
//!
//! Staleness is the one check that depends on the wall clock, so it is opt-in:
//! [`validate_bundle`] is deterministic and [`validate_bundle_at`] takes the
//! date to compare against.
//!
//! | Code | Severity | Finding                                                                            |
//! |------|----------|------------------------------------------------------------------------------------|
//! | V1   | error    | unparseable concept document (frontmatter parse error)                             |
//! | V2   | error    | missing or non-scalar required `type` field                                        |
//! | V3   | warning  | missing recommended frontmatter field (`title`, `description`, `generated`)        |
//! | V4   | warning  | concept body is empty                                                              |
//! | V5   | warning  | `tags` is not a YAML list of strings                                               |
//! | V6   | warning  | `generated` is malformed, missing `by`, or `at` is not a valid ISO datetime        |
//! | V7   | warning  | `verified` is malformed, missing `by`, or `at` is not a valid ISO datetime         |
//! | V8   | warning  | latest `verified.at` predates `generated.at` (content modified after verification) |
//! | V9   | warning  | timestamp in `generated`, `verified`, or `sources` is in the future                |
//! | V10  | warning  | unknown `status` value (not `draft`, `stable`, `deprecated`)                       |
//! | V11  | warning  | `stale_after` is not a valid ISO datetime                                          |
//! | V12  | info     | stale concept (past `stale_after` with `--today`)                                  |
//! | V13  | warning  | `sources` malformed, missing `resource`, or duplicate `id`                         |
//! | V14  | warning  | `sources.last_modified` or `usage_window` not valid ISO datetime                   |
//! | V15  | warning  | `sources.usage_count` without `usage_window` or not an integer                     |
//! | V16  | warning  | footnote attribution matches no `sources[].id`                                     |
//! | V17  | warning  | footnote is cited in body but never defined                                        |
//! | V18  | warning  | circular concept derivation in sources graph                                       |
//! | V19  | warning  | legacy v0.1 `timestamp` present (superseded by `generated`)                        |
//! | V20  | warning  | legacy v0.1 body `# Citations` list present (superseded by `sources`)              |
//! | V21  | warning  | missing `runtime` or `computation` source on `Attested Computation`                |
//! | V22  | warning  | contract `parameters`, `executor`, or `attester` missing required fields           |
//! | V23  | warning  | `executor`, `attester`, or `computation` resource missing on disk                  |
//! | V24  | warning  | code block syntax error in concept body                                            |
//! | V25  | warning  | computation, executor, or attester script syntax error                             |
//! | V26  | info     | computation fields on non-computation concept type                                 |
//! | V27  | info     | explicit path field does not resolve to a file in the bundle                       |
//! | V28  | info     | broken link (target does not resolve to a concept in the bundle)                   |
//! | V29  | warning  | links to a `status: deprecated` concept                                            |
//! | V30  | warning  | `title` shared with another concept                                                |
//! | V31  | warning  | concept-id segment outside portable ASCII set                                      |
//! | V32  | error    | reserved `index.md` or `log.md` unreadable, unparseable, or bad frontmatter        |
//! | V33  | error    | reserved `log.md` structural errors or invalid ISO date format                     |
//! | V34  | warning  | `log.md` contains duplicate date heading `## YYYY-MM-DD`                           |
//! | V35  | warning  | existing `index.md` is out of sync with its directory                              |
//! | V36  | info     | bundle declares unrecognized or non-target `okf_version`                           |

use okf_core::bundle::Bundle;
use okf_core::computation::{ATTESTED_COMPUTATION_TYPE, ComputationSource};
use okf_core::concept_id::ConceptId;
use okf_core::date::{Date, DateTime};
use okf_core::document::Document;
use okf_core::frontmatter::Frontmatter;
use okf_core::log::Log;
use okf_core::provenance::{ResourceKind, Source};
use okf_core::trust::{STATUS_VALUES, Verification};
use okf_core::yaml::Value;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

/// Severity of a diagnostic.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    /// A conformance violation.
    Error,
    /// A soft-guidance deviation (the bundle is still conformant).
    Warning,
    /// Informational note, for example a permitted but noteworthy cross-link or draft state.
    Info,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Info => "info",
        })
    }
}

/// A single finding about a bundle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    /// How serious the finding is.
    pub severity: Severity,
    /// The file the finding relates to, if any.
    pub path: Option<PathBuf>,
    /// The concept the finding relates to, if any.
    pub concept: Option<ConceptId>,
    /// A human-readable message.
    pub message: String,
    /// Whether this finding can be automatically remediated with `okf fix`.
    pub fixable: bool,
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] ", self.severity)?;
        if let Some(p) = &self.path {
            write!(f, "{}: ", p.display())?;
        } else if let Some(c) = &self.concept {
            write!(f, "{c}: ")?;
        }
        f.write_str(&self.message)
    }
}

/// The result of validating a bundle.
#[derive(Clone, Debug, Default)]
pub struct Report {
    /// All findings, errors first by construction order.
    pub diagnostics: Vec<Diagnostic>,
}

impl Report {
    /// `true` if there are no [`Severity::Error`] diagnostics, i.e. the bundle
    /// is conformant.
    #[must_use]
    pub fn is_conformant(&self) -> bool {
        !self
            .diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    }

    /// Iterates over diagnostics of a given severity.
    pub fn of(&self, severity: Severity) -> impl Iterator<Item = &Diagnostic> {
        self.diagnostics
            .iter()
            .filter(move |d| d.severity == severity)
    }

    /// Count of error-level diagnostics.
    #[must_use]
    pub fn error_count(&self) -> usize {
        self.of(Severity::Error).count()
    }

    /// Count of warning-level diagnostics.
    #[must_use]
    pub fn warning_count(&self) -> usize {
        self.of(Severity::Warning).count()
    }

    /// Count of findings that can be automatically fixed with `okf fix`.
    #[must_use]
    pub fn fixable_count(&self) -> usize {
        self.diagnostics.iter().filter(|d| d.fixable).count()
    }
}

/// Validates a loaded bundle, returning all findings.
///
/// Deterministic: `stale_after` dates are checked for *syntax* but not against
/// the clock. Use [`validate_bundle_at`] to also flag stale concepts.
#[must_use]
pub fn validate_bundle(bundle: &Bundle) -> Report {
    validate_bundle_at(bundle, None)
}

/// Validates a bundle, additionally reporting concepts that are stale on
/// `today`.
#[must_use]
pub fn validate_bundle_at(bundle: &Bundle, today: Option<Date>) -> Report {
    let mut report = Report::default();

    // (1) Files whose frontmatter could not be parsed are conformance errors.
    for (path, error) in bundle.parse_errors() {
        report.error(
            Some(path.clone()),
            None,
            format!("unparseable concept document: {error}"),
        );
    }

    // (2) Every concept must carry a non-empty `type`. Everything else the
    // families add is soft guidance.
    for concept in bundle.concepts() {
        let mut cx = Context {
            report: &mut report,
            path: concept.path.clone(),
            id: concept.id.clone(),
        };
        let fm = &concept.document.frontmatter;

        if concept.document.validate().is_err() {
            if fm
                .get("type")
                .is_some_and(|value| value.as_display_str().is_none())
            {
                cx.error("`type` must be a non-empty scalar");
            } else {
                cx.error("missing required frontmatter field `type`");
            }
        }
        check_recommended(&mut cx, &concept.document);
        check_empty_body(&mut cx, &concept.document);
        check_tags(&mut cx, fm);
        check_trust(&mut cx, fm, today);
        check_lifecycle(&mut cx, fm, today);
        check_provenance(&mut cx, fm, today);
        check_attribution(&mut cx, &concept.document);
        check_legacy(&mut cx, &concept.document);
        check_computation(&mut cx, bundle, &concept.document);
        check_code_block_syntax(&mut cx, &concept.document);
        check_computation_script_syntax(&mut cx, bundle, &concept.document);
        check_path_fields(&mut cx, bundle, fm);
        check_links_to_deprecated(&mut cx, bundle);
    }

    // (3) Reserved files must follow their structure when present.
    check_segment_portability(bundle, &mut report);
    validate_reserved(bundle, &mut report);
    check_declared_version(bundle, &mut report);

    // Cross-bundle checks:
    check_duplicate_titles(bundle, &mut report);
    check_circular_derivation(bundle, &mut report);
    check_stale_indexes(bundle, &mut report);

    // Broken cross-links are permitted by the spec; report them as info.
    for (source, raw) in bundle.broken_links() {
        report.info(
            None,
            Some(source),
            format!("link target does not resolve to a concept in the bundle: {raw}"),
        );
    }

    report
}

/// The concept currently being checked, so each rule can emit diagnostics
/// without repeating the path and id.
struct Context<'a> {
    report: &'a mut Report,
    path: PathBuf,
    id: ConceptId,
}

impl Context<'_> {
    fn push_fixable(&mut self, severity: Severity, message: impl Into<String>, fixable: bool) {
        self.report.diagnostics.push(Diagnostic {
            severity,
            path: Some(self.path.clone()),
            concept: Some(self.id.clone()),
            message: message.into(),
            fixable,
        });
    }

    fn push(&mut self, severity: Severity, message: impl Into<String>) {
        self.push_fixable(severity, message, false);
    }

    fn error(&mut self, message: impl Into<String>) {
        self.push(Severity::Error, message);
    }

    fn warn(&mut self, message: impl Into<String>) {
        self.push(Severity::Warning, message);
    }

    fn warn_fixable(&mut self, message: impl Into<String>) {
        self.push_fixable(Severity::Warning, message, true);
    }

    fn info(&mut self, message: impl Into<String>) {
        self.push(Severity::Info, message);
    }
}

impl Report {
    fn add_fixable(
        &mut self,
        severity: Severity,
        path: Option<PathBuf>,
        concept: Option<ConceptId>,
        message: String,
        fixable: bool,
    ) {
        self.diagnostics.push(Diagnostic {
            severity,
            path,
            concept,
            message,
            fixable,
        });
    }

    fn add(
        &mut self,
        severity: Severity,
        path: Option<PathBuf>,
        concept: Option<ConceptId>,
        message: String,
    ) {
        self.add_fixable(severity, path, concept, message, false);
    }

    fn error(&mut self, path: Option<PathBuf>, concept: Option<ConceptId>, message: String) {
        self.add(Severity::Error, path, concept, message);
    }

    fn warn(&mut self, path: Option<PathBuf>, concept: Option<ConceptId>, message: String) {
        self.add(Severity::Warning, path, concept, message);
    }

    fn info(&mut self, path: Option<PathBuf>, concept: Option<ConceptId>, message: String) {
        self.add(Severity::Info, path, concept, message);
    }
}

/// Recommended fields (`title`, `description`, `status`, `generated`).
///
/// Always a warning: conformance forbids rejecting a concept for a missing optional
/// field, however much a producer wants it filled in.
fn check_recommended(cx: &mut Context, doc: &Document) {
    for field in doc.missing_recommended() {
        // `runtime` also comes back from `missing_recommended`, but
        // `check_computation` reports it with the reason it is required.
        if field == "runtime" {
            continue;
        }
        let fixable = field == "title" || field == "generated";
        cx.push_fixable(
            Severity::Warning,
            format!("missing recommended frontmatter field `{field}`"),
            fixable,
        );
    }
}

/// Checks that the document body is not empty.
fn check_empty_body(cx: &mut Context, doc: &Document) {
    if doc.body.trim().is_empty() {
        cx.warn("body is empty; a concept should carry at least one line of prose or code");
    }
}

/// The shape of `tags`.
///
/// Worth its own check because the failure is silent: the spec asks for "a YAML list
/// of short strings", and a producer that writes `tags: a, b, c` gets one plain
/// scalar, so [`Frontmatter::tags`] reads no tags at all and the concept
/// disappears from every tag view.
fn check_tags(cx: &mut Context, fm: &Frontmatter) {
    let Some(value) = fm.get("tags").filter(|v| !v.is_empty_value()) else {
        return;
    };
    if !matches!(value, Value::Sequence(_)) {
        cx.warn(format!(
            "`tags` should be a list of short strings, found {}; no tags are read from it",
            type_name(value)
        ));
    }
}

/// `generated` and `verified`.
fn check_trust(cx: &mut Context, fm: &Frontmatter, today: Option<Date>) {
    let check_date = today.or_else(Date::today_utc);
    let threshold_seconds = check_date.map(|d| (d.days_since_epoch() + 1) * 86_400);

    if let Some(value) = fm.get("generated").filter(|v| !v.is_empty_value()) {
        match fm.generated() {
            None => cx.warn(format!(
                "`generated` should be a `{{ by, at }}` mapping, found {}",
                type_name(value)
            )),
            Some(generated) => {
                if generated.by.is_none() {
                    cx.warn("`generated.by` is required within `generated`");
                }
                match generated.at {
                    None => {}
                    Some(at) if !at.is_valid() => {
                        cx.warn(format!(
                            "`generated.at` is not an ISO-8601 datetime with an explicit offset: {:?}",
                            at.raw
                        ));
                    }
                    Some(at) => {
                        if let Some(threshold) = threshold_seconds
                            && let Some(dt) = at.datetime
                            && dt.to_utc_seconds() > threshold
                        {
                            cx.warn(format!("`generated.at` timestamp `{dt}` is in the future"));
                        }
                    }
                }
            }
        }
    }

    let Some(value) = fm.get("verified").filter(|v| !v.is_empty_value()) else {
        return;
    };
    if !matches!(value, Value::Sequence(_) | Value::Mapping(_)) {
        cx.warn(format!(
            "`verified` should be a list of `{{ by, at }}` events (a bare mapping is read as \
             a one-element list), found {}",
            type_name(value)
        ));
        return;
    }
    let events = fm.verified();
    if events.is_empty() {
        cx.warn("`verified` contains no `{ by, at }` events");
    }
    match value {
        Value::Sequence(items) => {
            for (i, item) in items.iter().enumerate() {
                let Some(event) = Verification::from_value(item) else {
                    cx.warn(format!(
                        "`verified[{i}]` should be a mapping with `by` and `at`, found {}",
                        type_name(item)
                    ));
                    continue;
                };
                check_verification_event(cx, i, &event, threshold_seconds);
            }
        }
        Value::Mapping(_) => {
            if let Some(event) = Verification::from_value(value) {
                check_verification_event(cx, 0, &event, threshold_seconds);
            }
        }
        _ => unreachable!("verified shape checked above"),
    }

    // Check if latest verified.at predates generated.at
    if let Some(generated) = fm.generated()
        && let Some(generated_at) = generated.at.as_ref().and_then(|a| a.datetime)
        && let Some(latest) = okf_core::trust::latest_verification(&events)
        && let Some(latest_at) = latest.at.as_ref().and_then(|a| a.datetime)
        && latest_at < generated_at
    {
        cx.warn(format!(
            "latest verification ({latest_at}) predates `generated.at` ({generated_at}); \
             the current content was never re-verified"
        ));
    }
}

fn check_verification_event(
    cx: &mut Context,
    i: usize,
    event: &Verification,
    threshold_seconds: Option<i64>,
) {
    if event
        .by
        .as_ref()
        .is_none_or(|by| by.as_str().trim().is_empty())
    {
        cx.warn(format!("`verified[{i}].by` is missing"));
    }
    match &event.at {
        None => cx.warn(format!("`verified[{i}].at` is missing")),
        Some(at) if !at.is_valid() => cx.warn(format!(
            "`verified[{i}].at` is not an ISO-8601 datetime with an explicit offset: {:?}",
            at.raw
        )),
        Some(at) => {
            if let Some(threshold) = threshold_seconds
                && let Some(dt) = at.datetime
                && dt.to_utc_seconds() > threshold
            {
                cx.warn(format!(
                    "`verified[{i}].at` timestamp `{dt}` is in the future"
                ));
            }
        }
    }
}

/// Validates syntax of fenced code blocks inside concept bodies.
fn check_code_block_syntax(cx: &mut Context, doc: &Document) {
    let blocks = crate::syntax::extract_fenced_code_blocks(&doc.body);
    for block in blocks {
        if let Some(lang_tag) = &block.language
            && crate::syntax::Language::from_tag(lang_tag) != crate::syntax::Language::Unknown
            && let Err(err) = crate::syntax::check_syntax(lang_tag, &block.code)
        {
            cx.warn(format!(
                "code block syntax check failed ({}): {err}",
                err.language
            ));
        }
    }
}

/// Validates syntax of external script files referenced by Attested Computation contracts.
fn check_computation_script_syntax(cx: &mut Context, bundle: &Bundle, doc: &Document) {
    let Some(contract) = doc.attested_computation() else {
        return;
    };

    if let okf_core::computation::ComputationSource::File(path) = &contract.computation
        && let Some(resolved_path) = bundle.resolve_path_field(&cx.id, path)
        && let Ok(content) = fs::read_to_string(&resolved_path)
    {
        let ext = std::path::Path::new(path)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        let lang_tag = contract.runtime.as_deref().unwrap_or(ext);
        if let Err(err) = crate::syntax::check_syntax(lang_tag, &content) {
            cx.warn(format!(
                "computation script `{path}` syntax check failed ({}): {err}",
                err.language
            ));
        }
    }

    if let Some(executor) = &contract.executor
        && let Some(res) = &executor.resource
        && let Some(resolved_path) = bundle.resolve_path_field(&cx.id, res)
        && let Ok(content) = fs::read_to_string(&resolved_path)
    {
        let ext = std::path::Path::new(res)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        if let Err(err) = crate::syntax::check_syntax(ext, &content) {
            cx.warn(format!(
                "executor script `{res}` syntax check failed ({}): {err}",
                err.language
            ));
        }
    }

    if let Some(attester) = &contract.attester
        && let Some(res) = &attester.resource
        && let Some(resolved_path) = bundle.resolve_path_field(&cx.id, res)
        && let Ok(content) = fs::read_to_string(&resolved_path)
    {
        let ext = std::path::Path::new(res)
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        if let Err(err) = crate::syntax::check_syntax(ext, &content) {
            cx.warn(format!(
                "attester script `{res}` syntax check failed ({}): {err}",
                err.language
            ));
        }
    }
}

/// `status` and `stale_after`.
fn check_lifecycle(cx: &mut Context, fm: &Frontmatter, today: Option<Date>) {
    let status = fm.status();
    if !status.is_known() {
        cx.warn(format!(
            "unknown `status` value {:?}; the spec defines {} (consumers must still accept it)",
            status.to_string(),
            STATUS_VALUES.join(", ")
        ));
    }

    let Some(stale_after) = fm.stale_after() else {
        return;
    };
    match &stale_after.datetime {
        Some(dt) if stale_after.is_valid() => {
            if let Some(today) = today
                && today.to_utc_datetime() >= *dt
            {
                cx.info(format!("stale since {stale_after}"));
            }
        }
        _ => {
            cx.warn(format!(
                "`stale_after` is not an ISO-8601 datetime with an explicit offset: {:?}",
                stale_after.raw
            ));
        }
    }
}

/// `sources` and `usage_window`.
fn check_provenance(cx: &mut Context, fm: &Frontmatter, today: Option<Date>) {
    let check_date = today.or_else(Date::today_utc);
    let threshold_seconds = check_date.map(|d| (d.days_since_epoch() + 1) * 86_400);

    let Some(value) = fm.get("sources").filter(|v| !v.is_empty_value()) else {
        if fm.get("usage_window").is_some() {
            cx.warn("`usage_window` is present without `sources` to frame");
        }
        return;
    };
    if !matches!(value, Value::Sequence(_) | Value::Mapping(_)) {
        cx.warn(format!(
            "`sources` should be a list of entries, found {}",
            type_name(value)
        ));
        return;
    }

    let shared_window = fm.usage_window();
    if let Some(window) = &shared_window {
        for (field, date) in [("from", &window.from), ("to", &window.to)] {
            if let Some(d) = date.as_ref().filter(|d| !d.is_valid()) {
                cx.warn(format!(
                    "`usage_window.{field}` is not an ISO-8601 datetime with an explicit offset: {:?}",
                    d.raw
                ));
            }
        }
    }

    let mut seen_ids: HashSet<String> = HashSet::new();
    let entries: Vec<(usize, Source)> = match value {
        Value::Sequence(items) => items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| {
                if item.as_mapping().is_none() {
                    cx.warn(format!(
                        "`sources[{i}]` should be a mapping entry, found {}",
                        type_name(item)
                    ));
                    None
                } else {
                    Source::from_value(item).map(|source| (i, source))
                }
            })
            .collect(),
        Value::Mapping(_) => Source::from_value(value)
            .into_iter()
            .map(|source| (0, source))
            .collect(),
        _ => unreachable!("sources shape checked above"),
    };
    for (i, source) in &entries {
        if source.resource_kind() == ResourceKind::Missing {
            cx.warn(format!(
                "`sources[{i}].resource` is required within an entry"
            ));
        }
        if let Some(id) = &source.id
            && !seen_ids.insert(id.clone())
        {
            cx.warn(format!(
                "duplicate `sources[].id` {id:?}; ids are the join key for attribution"
            ));
        }
        if let Some(last_modified) = source.last_modified.as_ref().filter(|d| !d.is_valid()) {
            cx.warn(format!(
                "`sources[{i}].last_modified` is not an ISO-8601 datetime with an explicit offset: {:?}",
                last_modified.raw
            ));
        } else if let Some(threshold) = threshold_seconds
            && let Some(last_modified) = source.last_modified.as_ref().and_then(|d| d.datetime)
            && last_modified.to_utc_seconds() > threshold
        {
            cx.warn(format!(
                "`sources[{i}].last_modified` timestamp `{last_modified}` is in the future"
            ));
        }
        if source.usage_count.is_some()
            && source
                .effective_usage_window(shared_window.as_ref())
                .is_none()
        {
            cx.warn(format!(
                "`sources[{i}].usage_count` has no `usage_window` to frame it"
            ));
        }
    }

    // A non-integer `usage_count` is dropped by the typed reader, so check the
    // raw values too.
    if let Value::Sequence(items) = value {
        for (i, item) in items.iter().enumerate() {
            let raw = item.as_mapping().and_then(|m| m.get("usage_count"));
            if let Some(raw) = raw.filter(|v| v.as_int().is_none()) {
                cx.warn(format!(
                    "`sources[{i}].usage_count` should be an integer, found {}",
                    type_name(raw)
                ));
            }
        }
    }
}

/// Footnote attribution keyed to `sources[].id`.
fn check_attribution(cx: &mut Context, doc: &Document) {
    let has_sources = !doc.frontmatter.sources().is_empty();
    for attribution in doc.attributions() {
        if !attribution.is_resolved() && has_sources {
            cx.warn(format!(
                "footnote [^{}] matches no `sources[].id`; the label is the join key for \
                 attribution",
                attribution.label
            ));
        }
        if attribution.references > 0 && attribution.definitions == 0 {
            cx.warn(format!(
                "footnote [^{}] is cited but never defined",
                attribution.label
            ));
        }
    }
}

/// v0.1 constructs that v0.2 supersedes.
fn check_legacy(cx: &mut Context, doc: &Document) {
    let fm = &doc.frontmatter;
    if !is_blank(fm, "timestamp") {
        if is_blank(fm, "generated") {
            cx.warn_fixable("`timestamp` is superseded by `generated: { by, at }`");
        } else {
            cx.warn_fixable("`timestamp` is redundant alongside `generated` and should be removed");
        }
    }
    if doc.has_legacy_citations() {
        cx.warn_fixable(
            "the body `# Citations` list is superseded by the `sources` frontmatter field",
        );
    }
}

/// The Attested Computation contract.
fn check_computation(cx: &mut Context, bundle: &Bundle, doc: &Document) {
    let fm = &doc.frontmatter;
    let computation_keys = [
        "runtime",
        "parameters",
        "computation",
        "executor",
        "attester",
    ];

    if !fm.is_attested_computation() {
        let present: Vec<&str> = computation_keys
            .iter()
            .copied()
            .filter(|k| !is_blank(fm, k))
            .collect();
        if !present.is_empty() {
            cx.info(format!(
                "carries computation field(s) `{}` but `type` is not `{ATTESTED_COMPUTATION_TYPE}`; \
                 a sanctioned computation is its own concept",
                present.join("`, `")
            ));
        }
        return;
    }

    let Some(contract) = doc.attested_computation() else {
        return;
    };

    if contract.runtime.is_none() {
        cx.warn(
            "`runtime` is required on an `Attested Computation`; it defines what `parameters` mean",
        );
    }
    match &contract.computation {
        ComputationSource::Missing => cx.warn(
            "no computation: set `computation` to a path or add a `# Computation` block to the body",
        ),
        ComputationSource::File(_) if contract.has_redundant_inline => cx.warn(
            "`computation` names a file and the body also has a `# Computation` block; the spec asks for one or the other",
        ),
        _ => {}
    }

    for (i, parameter) in contract.parameters.iter().enumerate() {
        if parameter.name.is_none() {
            cx.warn(format!("`parameters[{i}].name` is missing"));
        }
        if parameter.type_.is_none() {
            cx.warn(format!("`parameters[{i}].type` is missing"));
        }
    }

    match &contract.executor {
        None => cx.warn("missing `executor`: nothing says how to run the computation"),
        Some(executor) => {
            if executor.resource.is_none() {
                cx.warn("`executor.resource` is missing; it names the run instructions or code");
            }
            if executor.receipt.is_empty() {
                cx.warn(
                    "`executor.receipt` is empty; it declares the evidence the attester inspects",
                );
            }
        }
    }

    match &contract.attester {
        None => cx.warn("missing `attester`: nothing can check a run's receipt"),
        Some(attester) if attester.resource.is_none() => {
            cx.warn("`attester.resource` is missing; it names the deterministic check");
        }
        Some(_) => {}
    }

    check_attestation_resources(cx, bundle, doc);
}

fn check_attestation_resources(cx: &mut Context, bundle: &Bundle, doc: &Document) {
    let Some(contract) = doc.attested_computation() else {
        return;
    };
    if let Some(executor) = &contract.executor
        && let Some(res) = &executor.resource
        && !res.starts_with("http://")
        && !res.starts_with("https://")
        && bundle.resolve_path_field(&cx.id, res).is_none()
    {
        cx.warn(format!(
            "`executor.resource` points to `{res}` which does not exist on disk"
        ));
    }
    if let Some(attester) = &contract.attester
        && let Some(res) = &attester.resource
        && !res.starts_with("http://")
        && !res.starts_with("https://")
        && bundle.resolve_path_field(&cx.id, res).is_none()
    {
        cx.warn(format!(
            "`attester.resource` points to `{res}` which does not exist on disk"
        ));
    }
    if let okf_core::computation::ComputationSource::File(path) = &contract.computation
        && !path.starts_with("http://")
        && !path.starts_with("https://")
        && bundle.resolve_path_field(&cx.id, path).is_none()
    {
        cx.warn(format!(
            "`computation` file `{path}` does not exist on disk"
        ));
    }
}

fn check_links_to_deprecated(cx: &mut Context, bundle: &Bundle) {
    let mut warned: BTreeSet<ConceptId> = BTreeSet::new();
    for link in bundle.links_from(&cx.id) {
        if !link.exists || !warned.insert(link.target.clone()) {
            continue;
        }
        if let Some(target) = bundle.get(&link.target)
            && target.status().is_deprecated()
        {
            cx.warn(format!("links to deprecated concept `{}`", link.target));
        }
    }
}

/// Path-valued fields that point inside the bundle but resolve to nothing.
/// Informational, since a bundle may legitimately be shipped without
/// the files its executor or attester references.
///
/// `resource` is only checked when it is written unambiguously as a path
/// (`/...`, `./...`, `../...`). A bare `resource` such as
/// `acme.sales.orders` is an opaque asset identifier, not a promise that a file
/// exists, and reporting it as a broken path would be noise.
fn check_path_fields(cx: &mut Context, bundle: &Bundle, fm: &Frontmatter) {
    let id = cx.id.clone();
    for (field, raw) in fm.path_fields() {
        let target = raw.trim();
        let explicit_path =
            target.starts_with('/') || target.starts_with("./") || target.starts_with("../");
        if field == "resource" && !explicit_path {
            continue;
        }
        if okf_core::links::field_path_candidates(target, &id).is_empty() {
            continue; // a URI, nothing in the bundle to resolve
        }
        if bundle.resolve_path_field(&id, target).is_none() {
            cx.info(format!(
                "`{field}` does not resolve to a file in the bundle: {raw}"
            ));
        }
    }
}

/// Concept-id segments outside the reference implementation's
/// `[A-Za-z0-9_][A-Za-z0-9_.\-]*` convention.
///
/// Never an error. The spec places no character constraint on filenames and
/// conformance is a question of frontmatter, so [`ConceptId`] accepts
/// these names and the bundle stays conformant. It is still worth telling a
/// producer: such a name has to be written as `<...>` or percent-encoded to be
/// linked from markdown, and is not guaranteed to survive every filesystem
/// unchanged.
///
/// Each distinct segment is reported once, against the first concept that uses
/// it, so one awkwardly named directory does not warn on every file inside it.
fn check_segment_portability(bundle: &Bundle, report: &mut Report) {
    let mut seen: HashSet<&str> = HashSet::new();
    for concept in bundle.concepts() {
        for segment in concept.id.segments() {
            if okf_core::concept_id::is_portable_segment(segment) || !seen.insert(segment) {
                continue;
            }
            report.warn(
                Some(concept.path.clone()),
                Some(concept.id.clone()),
                format!(
                    "concept-id segment {segment:?} is outside the conventional \
                     `[A-Za-z0-9_][A-Za-z0-9_.-]*` set; the bundle is still conformant, but \
                     such a name needs `<...>` or percent-encoded to link portably and is \
                     not guaranteed to survive every filesystem unchanged"
                ),
            );
        }
    }
}

fn check_duplicate_titles(bundle: &Bundle, report: &mut Report) {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for c in bundle.concepts() {
        if let Some(title) = c.document.frontmatter.title() {
            *counts.entry(title.into_owned()).or_default() += 1;
        }
    }
    for c in bundle.concepts() {
        if let Some(title) = c.document.frontmatter.title()
            && counts.get(title.as_ref()).copied().unwrap_or(0) > 1
        {
            report.warn(
                Some(c.path.clone()),
                Some(c.id.clone()),
                format!(
                    "`title` {title:?} is shared with another concept; titles should disambiguate"
                ),
            );
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
            report.warn(
                Some(concept.path.clone()),
                Some(concept.id.clone()),
                format!("circular concept derivation: {}", cycle_str.join(" ~> ")),
            );
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

/// Synthesizes the concept id an `index.md` would have if it were itself a
/// concept, so [`okf_core::links::Link::resolve_all`] can resolve its relative links against the
/// index's own directory.
#[must_use]
pub fn index_source_id(bundle_root: &Path, index_path: &Path) -> Option<ConceptId> {
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

/// Every link target an `index.md` lists, paired with the raw target as
/// written, resolved to a concept id whether or not that concept exists in the
/// bundle.
#[must_use]
pub fn index_listed_targets(bundle: &Bundle, index_path: &Path) -> Vec<(String, ConceptId)> {
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
#[must_use]
pub fn is_concept_link(raw: &str) -> bool {
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
    #[allow(clippy::case_sensitive_file_extension_comparisons)]
    {
        basename.ends_with(".md") || !basename.contains('.')
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
        let index_dir = index_id.parent();

        let actual: BTreeSet<ConceptId> = bundle
            .concepts()
            .iter()
            .filter(|c| c.path.parent() == Some(dir))
            .map(|c| c.id.clone())
            .collect();

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

        report.add_fixable(
            Severity::Warning,
            Some(index_path.clone()),
            None,
            format!(
                "index.md is out of sync with its directory ({})",
                parts.join("; ")
            ),
            true,
        );
    }
}

fn validate_reserved(bundle: &Bundle, report: &mut Report) {
    let root_index = bundle.root().join("index.md");

    for path in bundle.index_files() {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) => {
                report.error(
                    Some(path.clone()),
                    None,
                    format!("unreadable reserved index.md: {error}"),
                );
                continue;
            }
        };
        let doc = match Document::parse(&text) {
            Ok(doc) => doc,
            Err(error) => {
                report.error(
                    Some(path.clone()),
                    None,
                    format!("unparseable reserved index.md: {error}"),
                );
                continue;
            }
        };
        if doc.frontmatter.is_empty() {
            continue;
        }
        // Frontmatter is only permitted in the bundle-root index.md, and only
        // to declare `okf_version`.
        let is_root = path == &root_index;
        if is_root {
            let only_version = doc
                .frontmatter
                .as_mapping()
                .keys()
                .all(|k| k == "okf_version");
            if !only_version {
                report.error(
                    Some(path.clone()),
                    None,
                    "root index.md frontmatter should declare only `okf_version`".to_string(),
                );
            }
        } else {
            report.error(
                Some(path.clone()),
                None,
                "index.md should not contain frontmatter".to_string(),
            );
        }
    }

    for path in bundle.log_files() {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) => {
                report.error(
                    Some(path.clone()),
                    None,
                    format!("unreadable reserved log.md: {error}"),
                );
                continue;
            }
        };
        let log = Log::parse(&text);
        for issue in log.structural_errors(&text) {
            report.error(Some(path.clone()), None, issue);
        }
        for bad in log.invalid_dates() {
            report.error(
                Some(path.clone()),
                None,
                format!("log date heading is not ISO-8601 `YYYY-MM-DD`: {bad:?}"),
            );
        }
        let mut seen = HashMap::new();
        for day in &log.days {
            *seen.entry(day.date.clone()).or_insert(0) += 1;
        }
        for (date, count) in seen {
            if count > 1 {
                report.add_fixable(
                    Severity::Warning,
                    Some(path.clone()),
                    None,
                    format!(
                        "log.md contains duplicate date heading `## {date}` (entries should be grouped under a single heading)"
                    ),
                    true,
                );
            }
        }
    }
}

/// The `okf_version` a bundle declares.
///
/// Never an error. The spec is explicit that a consumer which does not understand
/// the declared version should attempt best-effort consumption rather than
/// refusing the bundle.
fn check_declared_version(bundle: &Bundle, report: &mut Report) {
    let Some(declared) = bundle.okf_version() else {
        return;
    };
    let declared = declared.trim();
    if declared == okf_core::OKF_VERSION {
        return;
    }
    let message = if okf_core::SUPPORTED_OKF_VERSIONS.contains(&declared) {
        format!(
            "bundle targets OKF v{declared}; read as v{} under documented fallbacks",
            okf_core::OKF_VERSION
        )
    } else {
        format!(
            "bundle declares an unrecognized `okf_version: {declared}`; consuming it \
             best-effort as v{}",
            okf_core::OKF_VERSION
        )
    };
    report.info(Some(bundle.root().join("index.md")), None, message);
}

fn is_blank(fm: &Frontmatter, key: &str) -> bool {
    fm.get(key).is_none_or(Value::is_empty_value)
}

/// A short YAML type name, for diagnostics about a mis-shaped value.
const fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Int(_) => "an integer",
        Value::Float(_) => "a float",
        Value::String(_) => "a string",
        Value::Sequence(_) => "a list",
        Value::Mapping(_) => "a mapping",
    }
}

/// Checks an ISO-8601 datetime with a time of day and an explicit UTC offset.
///
/// OKF's timestamp fields require a time of day with an explicit offset.
#[must_use]
pub fn is_iso8601_datetime(s: &str) -> bool {
    DateTime::parse(s)
        .is_some_and(|datetime| datetime.has_time && datetime.offset_minutes.is_some())
}
