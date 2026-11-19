//! Conformance checking and structural validation against OKF v0.3.
//!
//! A bundle is **conformant** when (1) each frontmatter block in a `.md` file
//! under the bundle root parses as a YAML mapping, and (2) each reserved file
//! (`index.md`, `log.md`) agrees with its structure when it is present. A
//! `.md` file without frontmatter, or without `type`, is simply not a concept;
//! it does not make the bundle non-conformant. Everything else is soft
//! guidance: consumers MUST NOT reject a bundle for missing optional fields,
//! unknown types or keys, broken links, or missing `index.md` files.
//!
//! Accordingly, [`validate_bundle`] reports only true conformance violations as
//! [`Severity::Error`]. Values that do not follow the other sections of the
//! specification, data integrity issues, temporal inconsistencies, contract
//! discrepancies, and syntax errors are reported as [`Severity::Warning`]
//! (producer mistakes worth fixing) or [`Severity::Info`] (permitted states
//! worth noting).
//!
//! Staleness is the one check that depends on the wall clock, so it is opt-in:
//! [`validate_bundle`] is deterministic and [`validate_bundle_at`] takes the
//! date to compare against.
//!
//! | Code | Severity | Finding                                                                               |
//! |------|----------|---------------------------------------------------------------------------------------|
//! | V1   | error    | frontmatter block does not parse as a YAML mapping                                    |
//! | V2   | error    | frontmatter repeats a key                                                             |
//! | V3   | error    | `index.md` below the bundle root has frontmatter, or a reserved file is unreadable    |
//! | V4   | error    | `log.md` has frontmatter                                                              |
//! | V5   | error    | `log.md` date heading is not `YYYY-MM-DD`                                             |
//! | V6   | warning  | markdown file is not a concept (no frontmatter, or no non-empty `type`)               |
//! | V7   | warning  | `type`, `title`, `description`, or `status` is not a string                           |
//! | V8   | warning  | missing recommended frontmatter field (`title`, `description`, `generated`)           |
//! | V9   | warning  | concept body is empty                                                                 |
//! | V10  | warning  | `tags` is not a YAML list (a scalar reads as one tag)                                 |
//! | V11  | warning  | `generated` is malformed or missing `by`                                              |
//! | V12  | warning  | `verified` entry is malformed or missing `by` or `at`; consumers ignore it            |
//! | V13  | warning  | actor is neither `<producer>/<version>` nor `<scheme>:<id>`                           |
//! | V14  | warning  | timestamp is not an RFC 3339 date-time with an offset, or a date                      |
//! | V15  | warning  | `generated.at` or `verified[].at` is a date where a date-time is expected             |
//! | V16  | warning  | timestamp in `generated`, `verified`, or `sources` is in the future                   |
//! | V17  | info     | no verification is current: each is older than `generated.at`                         |
//! | V18  | warning  | unknown `status` value; consumers treat the concept as not current                    |
//! | V19  | warning  | `superseded_by` without `status: deprecated`, or not naming a concept                 |
//! | V20  | warning  | `stale_after` does not parse; consumers never treat the concept as fresh              |
//! | V21  | info     | stale concept (past `stale_after`)                                                    |
//! | V22  | warning  | `sources` entry is malformed, missing `resource`, or repeats an `id`                  |
//! | V23  | warning  | `usage_count` without a `usage_window`, not an integer, or a malformed window         |
//! | V24  | warning  | footnote attribution matches no `sources[].id`, or is cited but never defined         |
//! | V25  | warning  | circular concept derivation in the sources graph                                      |
//! | V26  | warning  | legacy v0.1 `timestamp` or body `# Citations` list present                            |
//! | V27  | warning  | `Attested Computation` without `runtime`, or without a computation that can be found  |
//! | V28  | warning  | `parameters`, `executor`, or `attester` entry missing a required key                  |
//! | V29  | info     | `Attested Computation` without an `attester`: its values cannot be shown as attested  |
//! | V30  | warning  | path field does not resolve to a file in the bundle                                   |
//! | V31  | warning  | path field resolves only from the bundle root; write it with a leading `/`            |
//! | V32  | warning  | syntax error in the computation, or in an executor or attester script                 |
//! | V33  | info     | computation fields on a concept that is not an `Attested Computation`                 |
//! | V34  | info     | broken link (target names nothing in the bundle)                                      |
//! | V35  | warning  | link anchor matches no heading in its target                                          |
//! | V36  | warning  | links to a `status: deprecated` concept                                               |
//! | V37  | warning  | `title` shared with another concept                                                   |
//! | V38  | warning  | concept-id segment outside the portable ASCII set                                     |
//! | V39  | warning  | `log.md` repeats a date heading, is not newest first, or is not a list of dated entries |
//! | V40  | warning  | existing `index.md` is out of sync with its directory                                 |
//! | V41  | warning  | no bundle-root `index.md`, or it does not declare `okf_version`                       |
//! | V42  | warning  | `license` is neither an SPDX expression nor a file in the bundle                      |
//! | V43  | info     | bundle declares an earlier or unrecognized `okf_version`                              |
//! | V44  | info     | extension keys without the `x-` prefix                                                |

use okf_core::Actor;
use okf_core::bundle::{Bundle, NonConceptReason, PathResolution};
use okf_core::computation::{ATTESTED_COMPUTATION_TYPE, ComputationSource, Parameter};
use okf_core::concept_id::ConceptId;
use okf_core::date::{Date, DateTime, DateTimeField};
use okf_core::document::Document;
use okf_core::frontmatter::Frontmatter;
use okf_core::log::Log;
use okf_core::provenance::{ResourceKind, Source, UsageWindow};
use okf_core::trust::{STATUS_VALUES, Verification};
use okf_core::yaml::Value;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

/// Severity of a diagnostic.
///
/// Ordered by increasing severity: `Info < Warning < Error`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// Informational note, for example a permitted but noteworthy cross-link or draft state.
    Info,
    /// A soft-guidance deviation (the bundle is still conformant).
    Warning,
    /// A conformance violation.
    Error,
}

impl Severity {
    /// The string representation of this severity level (`"info"`, `"warning"`, `"error"`).
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

impl AsRef<str> for Severity {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

/// Error returned when a string cannot be parsed into a [`Severity`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseSeverityError(pub String);

impl fmt::Display for ParseSeverityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown severity: {:?}", self.0)
    }
}

impl std::error::Error for ParseSeverityError {}

impl std::str::FromStr for Severity {
    type Err = ParseSeverityError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "info" => Ok(Self::Info),
            "warning" | "warn" => Ok(Self::Warning),
            "error" => Ok(Self::Error),
            other => Err(ParseSeverityError(other.to_string())),
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
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
/// Deterministic: `stale_after` values are checked for *syntax* but not against
/// the clock. Use [`validate_bundle_at`] to also flag stale concepts.
#[must_use]
pub fn validate_bundle(bundle: &Bundle) -> Report {
    validate_bundle_at(bundle, None)
}

/// Validates a bundle, additionally reporting concepts that are stale on
/// `today`, judged at the start of that day in UTC.
#[must_use]
pub fn validate_bundle_at(bundle: &Bundle, today: Option<Date>) -> Report {
    validate_bundle_at_instant(bundle, today.map(|day| day.to_utc_datetime()))
}

/// Validates a bundle, additionally reporting concepts that are stale at the
/// instant `now`.
///
/// A concept is stale when `now >= stale_after`, so this is the exact form of
/// [`validate_bundle_at`]: a `stale_after` earlier today counts.
#[must_use]
pub fn validate_bundle_at_instant(bundle: &Bundle, now: Option<DateTime>) -> Report {
    let mut report = Report::default();
    let today = now.map(|now| now.utc_date());

    // (1) A frontmatter block that does not parse as a YAML mapping is a
    // conformance error, in any markdown file of the bundle.
    for (path, error) in bundle.parse_errors() {
        report.error(
            Some(path.clone()),
            None,
            format!("frontmatter does not parse as a YAML mapping: {error}"),
        );
    }

    // A markdown file without frontmatter or without `type` is not a concept.
    // Consumers skip it and the bundle stays conformant, but it is worth
    // telling the producer in case a concept was meant.
    check_non_concepts(bundle, &mut report);

    // Everything the families add to a concept is soft guidance.
    for concept in bundle.concepts() {
        let mut cx = Context {
            report: &mut report,
            path: concept.path.clone(),
            id: concept.id.clone(),
        };
        let fm = &concept.document.frontmatter;

        check_duplicate_keys(&mut cx, fm);
        check_scalar_strings(&mut cx, fm);
        check_recommended(&mut cx, &concept.document);
        check_empty_body(&mut cx, &concept.document);
        check_tags(&mut cx, fm);
        check_trust(&mut cx, fm, today);
        check_lifecycle(&mut cx, bundle, fm, now);
        check_provenance(&mut cx, fm, today);
        check_attribution(&mut cx, &concept.document);
        check_legacy(&mut cx, &concept.document);
        check_computation(&mut cx, &concept.document);
        check_inline_computation_syntax(&mut cx, &concept.document);
        check_computation_script_syntax(&mut cx, bundle, &concept.document);
        check_path_fields(&mut cx, bundle, fm);
        check_links_to_deprecated(&mut cx, bundle);
        check_link_anchors(&mut cx, bundle, &concept.document);
        check_extension_keys(&mut cx, fm);
    }

    // (2) Reserved files must follow their structure when present.
    check_segment_portability(bundle, &mut report);
    validate_reserved(bundle, &mut report);
    check_bundle_metadata(bundle, &mut report);
    check_declared_version(bundle, &mut report);

    // Cross-bundle checks:
    check_duplicate_titles(bundle, &mut report);
    check_circular_derivation(bundle, &mut report);
    check_stale_indexes(bundle, &mut report);

    // Broken links are permitted by the spec: the target can be knowledge
    // that nobody has written yet. Report them as info. A link to an asset or
    // to a markdown file that is not a concept is valid and is not reported.
    for (source, raw) in bundle.broken_links() {
        report.info(
            None,
            Some(source),
            format!("link target is not in the bundle: {raw}"),
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

/// Markdown files that are not concepts.
///
/// Never an error: such a file does not affect conformance, which is what
/// lets a bundle hold a `README.md` or a `SKILL.md`. A file that has
/// frontmatter but no `type` is still checked for repeated keys, since that
/// rule covers every frontmatter block in the bundle.
fn check_non_concepts(bundle: &Bundle, report: &mut Report) {
    for file in bundle.non_concepts() {
        report.warn(
            Some(file.path.clone()),
            None,
            format!("not a concept: {}; consumers skip it", file.reason),
        );
        if file.reason == NonConceptReason::NoType
            && let Ok(text) = fs::read_to_string(&file.path)
            && let Ok(doc) = Document::parse(&text)
        {
            for key in Value::Mapping(doc.frontmatter.as_mapping().clone()).duplicate_key_paths() {
                report.error(
                    Some(file.path.clone()),
                    None,
                    format!("frontmatter repeats the key `{key}`; keys must be unique"),
                );
            }
        }
    }
}

/// Repeated keys, at any depth of the frontmatter.
///
/// A producer must not write them, and a consumer may reject a block that has
/// them, so this is a conformance error. The loader is lenient and reads the
/// first occurrence, which is exactly why it has to be reported: two tools can
/// read the same file two ways.
fn check_duplicate_keys(cx: &mut Context, fm: &Frontmatter) {
    for key in Value::Mapping(fm.as_mapping().clone()).duplicate_key_paths() {
        cx.error(format!(
            "frontmatter repeats the key `{key}`; keys must be unique"
        ));
    }
}

/// Fields that are strings, written so that YAML reads them as something
/// else: `title: 2026`, `description: true`. The value is still read, as its
/// text, but quoting it makes every tool read the same thing.
fn check_scalar_strings(cx: &mut Context, fm: &Frontmatter) {
    for key in ["type", "title", "description", "status"] {
        if let Some(value) = fm.get(key)
            && !value.is_blank()
            && !matches!(value, Value::String(_))
        {
            cx.warn(format!(
                "`{key}` is {}, not a string; quote the value",
                type_name(value)
            ));
        }
    }
}

/// Recommended fields (`title`, `description`, `generated`).
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
/// A scalar `tags` value is one tag, and consumers do not split it at commas.
/// A producer that writes `tags: a, b, c` therefore gets the single tag
/// `a, b, c`, which is almost never what was meant.
fn check_tags(cx: &mut Context, fm: &Frontmatter) {
    let Some(value) = fm.get("tags").filter(|v| !v.is_blank()) else {
        return;
    };
    match value {
        Value::Sequence(_) => {}
        Value::Mapping(_) => cx.warn("`tags` should be a list of short strings, found a mapping"),
        scalar => {
            let text = scalar.as_display_string().unwrap_or_default();
            let hint = if text.contains(',') {
                format!("; consumers read it as the single tag {text:?}")
            } else {
                String::new()
            };
            cx.warn_fixable(format!(
                "`tags` should be a YAML list, found {}{hint}",
                type_name(scalar)
            ));
        }
    }
}

/// The number of seconds since the epoch after which a timestamp counts as
/// being in the future: the end of the day after `today`, to allow for time
/// zones.
fn future_threshold(today: Option<Date>) -> Option<i64> {
    today
        .or_else(Date::today_utc)
        .map(|d| (d.days_since_epoch() + 1) * 86_400)
}

/// Checks one timestamp value. `prefer_datetime` is set for `generated.at` and
/// `verified[].at`, which consumers compare with each other.
fn check_timestamp(
    cx: &mut Context,
    field: &str,
    value: &DateTimeField,
    prefer_datetime: bool,
    threshold_seconds: Option<i64>,
) {
    let Some(instant) = value.instant() else {
        cx.push_fixable(
            Severity::Warning,
            format!(
                "`{field}` is not an RFC 3339 date-time with an offset (such as \
                 2026-06-30T14:00:00Z) or a date (such as 2026-06-30): {:?}",
                value.raw
            ),
            value.is_repairable(),
        );
        return;
    };
    if prefer_datetime && value.is_date_only() {
        cx.warn_fixable(format!(
            "`{field}` is a date; write it as a date-time, because consumers compare \
             `generated.at` with `verified[].at`"
        ));
    }
    if let Some(threshold) = threshold_seconds
        && instant.to_utc_seconds() > threshold
    {
        cx.warn(format!(
            "`{field}` timestamp `{}` is in the future",
            value.raw
        ));
    }
}

/// Reports an actor string that follows neither actor form.
fn check_actor(cx: &mut Context, field: &str, actor: &Actor) {
    if !actor.is_conventional() {
        cx.warn(format!(
            "`{field}` is not an actor: expected `<producer>/<version>` or `<scheme>:<id>` \
             (such as `human:<id>`), found {:?}",
            actor.as_str()
        ));
    }
}

/// `generated` and `verified`.
fn check_trust(cx: &mut Context, fm: &Frontmatter, today: Option<Date>) {
    let threshold_seconds = future_threshold(today);

    if let Some(value) = fm.get("generated").filter(|v| !v.is_blank()) {
        match fm.generated() {
            None => cx.warn(format!(
                "`generated` should be a `{{ by, at }}` mapping, found {}",
                type_name(value)
            )),
            Some(generated) => {
                match &generated.by {
                    None => cx.warn("`generated.by` is required within `generated`"),
                    Some(by) => check_actor(cx, "generated.by", by),
                }
                if let Some(at) = &generated.at {
                    check_timestamp(cx, "generated.at", at, true, threshold_seconds);
                }
            }
        }
    }

    let Some(value) = fm.get("verified").filter(|v| !v.is_blank()) else {
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
    for (i, item) in value.as_list().into_iter().enumerate() {
        let Some(event) = Verification::from_value(item) else {
            cx.warn(format!(
                "`verified[{i}]` should be a mapping with `by` and `at`, found {}; consumers \
                 ignore it",
                type_name(item)
            ));
            continue;
        };
        check_verification_event(cx, i, &event, threshold_seconds);
    }

    // Content can change without a new confirmation. When every check
    // predates the last change, nobody has confirmed the current text.
    if fm.verified_before_change() {
        cx.info(
            "no verification is current: each `verified` event is older than `generated.at`, \
             so consumers read the concept as unverified",
        );
    }
}

fn check_verification_event(
    cx: &mut Context,
    i: usize,
    event: &Verification,
    threshold_seconds: Option<i64>,
) {
    if !event.is_complete() {
        let missing = match (&event.by, &event.at) {
            (None, None) => "`by` and `at`",
            (None, Some(_)) => "`by`",
            _ => "`at`",
        };
        cx.warn(format!(
            "`verified[{i}]` is missing {missing}; consumers ignore an event without both"
        ));
    }
    if let Some(by) = &event.by {
        check_actor(cx, &format!("verified[{i}].by"), by);
    }
    if let Some(at) = &event.at {
        check_timestamp(
            cx,
            &format!("verified[{i}].at"),
            at,
            true,
            threshold_seconds,
        );
    }
}

/// Validates syntax of the inline `# Computation` block of an Attested
/// Computation.
///
/// Only the sanctioned computation is checked (§10.3): it is the code an agent
/// is expected to execute, so whether it parses is worth knowing there. Every
/// other fenced block is documentation, and documentation is
/// routinely a fragment (a bare join condition, a formula, an elided snippet),
/// so it is deliberately left alone rather than reported against a
/// whole-statement grammar. The language comes from the fence tag, falling
/// back to `runtime`; a language this build has no parser for (an unknown tag,
/// or a parser compiled out by a disabled crate feature) is skipped.
fn check_inline_computation_syntax(cx: &mut Context, doc: &Document) {
    let Some(contract) = doc.attested_computation() else {
        return;
    };
    let ComputationSource::Inline(inline) = &contract.computation else {
        return;
    };
    let Some(lang_tag) = inline.language.as_deref().or(contract.runtime.as_deref()) else {
        return;
    };
    if crate::syntax::is_templated(&inline.code) {
        return;
    }
    if crate::syntax::Language::from_tag(lang_tag).is_supported()
        && let Err(err) = crate::syntax::check_syntax(lang_tag, &inline.code)
    {
        cx.warn(format!(
            "`# Computation` code block syntax check failed ({}): {err}",
            err.language
        ));
    }
}

/// Validates syntax of external script files referenced by Attested Computation contracts.
///
/// A script is read only when its path resolves to a file inside the bundle
/// root.
fn check_computation_script_syntax(cx: &mut Context, bundle: &Bundle, doc: &Document) {
    let Some(contract) = doc.attested_computation() else {
        return;
    };
    let read = |raw: &str| -> Option<String> {
        let path = bundle.resolve_path_field(&cx.id, raw)?;
        path.is_file().then(|| fs::read_to_string(path).ok())?
    };
    let extension = |raw: &str| -> String {
        Path::new(okf_core::links::path_part(raw))
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string()
    };

    let mut findings = Vec::new();
    if let ComputationSource::File(path) = &contract.computation
        && let Some(content) = read(path)
        && !crate::syntax::is_templated(&content)
    {
        let ext = extension(path);
        let lang_tag = contract.runtime.as_deref().unwrap_or(&ext);
        if let Err(err) = crate::syntax::check_syntax(lang_tag, &content) {
            findings.push(format!(
                "computation script `{path}` syntax check failed ({}): {err}",
                err.language
            ));
        }
    }
    for (role, resource) in [
        (
            "executor",
            contract
                .executor
                .as_ref()
                .and_then(|e| e.resource.as_deref()),
        ),
        (
            "attester",
            contract
                .attester
                .as_ref()
                .and_then(|a| a.resource.as_deref()),
        ),
    ] {
        if let Some(res) = resource
            && let Some(content) = read(res)
            && let Err(err) = crate::syntax::check_syntax(&extension(res), &content)
        {
            findings.push(format!(
                "{role} script `{res}` syntax check failed ({}): {err}",
                err.language
            ));
        }
    }
    for finding in findings {
        cx.warn(finding);
    }
}

/// `status`, `superseded_by`, and `stale_after`.
fn check_lifecycle(cx: &mut Context, bundle: &Bundle, fm: &Frontmatter, now: Option<DateTime>) {
    let status = fm.status();
    if !status.is_known() {
        cx.warn(format!(
            "unknown `status` value {:?}; the spec defines {}, and consumers treat any other \
             value as not current",
            status.to_string(),
            STATUS_VALUES.join(", ")
        ));
    }

    if let Some(superseded_by) = fm.superseded_by() {
        if !status.is_deprecated() {
            cx.warn(
                "`superseded_by` has a meaning only with `status: deprecated`; consumers ignore \
                 it otherwise",
            );
        }
        // Whether the path resolves at all is reported with the other path
        // fields. This is about what it resolves to.
        if bundle.resolve_path(&cx.id, &superseded_by).path().is_some()
            && !bundle
                .path_edges_from(&cx.id)
                .iter()
                .any(|edge| edge.field == "superseded_by")
        {
            cx.warn(format!(
                "`superseded_by` should name the concept that replaces this one, but \
                 `{superseded_by}` is not a concept"
            ));
        }
    }

    let Some(stale_after) = fm.stale_after() else {
        return;
    };
    match stale_after.instant() {
        Some(instant) => {
            if now.is_some_and(|now| now >= instant) {
                cx.info(format!("stale since {stale_after}"));
            }
        }
        None => cx.push_fixable(
            Severity::Warning,
            format!(
                "`stale_after` does not parse as an RFC 3339 date-time with an offset or a \
                 date, so consumers never treat the concept as fresh: {:?}",
                stale_after.raw
            ),
            stale_after.is_repairable(),
        ),
    }
}

/// A `usage_window`: a `{ from, to }` mapping of timestamps.
fn check_usage_window(cx: &mut Context, field: &str, value: &Value) {
    let Some(window) = UsageWindow::from_value(value) else {
        cx.warn(format!(
            "`{field}` should be a `{{ from, to }}` mapping of timestamps, found {}",
            type_name(value)
        ));
        return;
    };
    if !window.is_complete() {
        cx.warn(format!("`{field}` should give both `from` and `to`"));
    }
    for (end, date) in [("from", &window.from), ("to", &window.to)] {
        if let Some(date) = date {
            check_timestamp(cx, &format!("{field}.{end}"), date, false, None);
        }
    }
}

/// `sources` and `usage_window`.
fn check_provenance(cx: &mut Context, fm: &Frontmatter, today: Option<Date>) {
    let threshold_seconds = future_threshold(today);
    let shared_window = fm.get("usage_window").filter(|v| !v.is_blank());

    let Some(value) = fm.get("sources").filter(|v| !v.is_blank()) else {
        if shared_window.is_some() {
            cx.warn("`usage_window` is present without `sources` to frame");
        }
        return;
    };
    if let Some(window) = shared_window {
        check_usage_window(cx, "usage_window", window);
    }
    if !matches!(value, Value::Sequence(_) | Value::Mapping(_)) {
        cx.warn(format!(
            "`sources` should be a list of entries, found {}",
            type_name(value)
        ));
        return;
    }

    let mut seen_ids: HashSet<String> = HashSet::new();
    for (i, item) in value.as_list().into_iter().enumerate() {
        let Some(map) = item.as_mapping() else {
            cx.warn(format!(
                "`sources[{i}]` should be a mapping entry, found {}; consumers ignore it",
                type_name(item)
            ));
            continue;
        };
        let Some(source) = Source::from_value(item) else {
            continue;
        };
        if source.resource_kind() == ResourceKind::Missing {
            cx.warn(format!(
                "`sources[{i}].resource` is required within an entry; consumers ignore the entry"
            ));
            continue;
        }
        if let Some(id) = &source.id
            && !seen_ids.insert(id.clone())
        {
            cx.warn(format!(
                "duplicate `sources[].id` {id:?}; ids are the join key for attribution"
            ));
        }
        if let Some(author) = &source.author {
            check_actor(cx, &format!("sources[{i}].author"), author);
        }
        if let Some(last_modified) = &source.last_modified {
            check_timestamp(
                cx,
                &format!("sources[{i}].last_modified"),
                last_modified,
                false,
                threshold_seconds,
            );
        }
        let own_window = map.get("usage_window").filter(|v| !v.is_blank());
        if let Some(window) = own_window {
            check_usage_window(cx, &format!("sources[{i}].usage_window"), window);
        }
        // A non-integer `usage_count` is dropped by the typed reader, so look
        // at the raw value.
        if let Some(raw) = map.get("usage_count").filter(|v| !v.is_blank()) {
            if raw.as_int().is_none() {
                cx.warn(format!(
                    "`sources[{i}].usage_count` should be an integer, found {}",
                    type_name(raw)
                ));
            }
            if own_window.is_none() && shared_window.is_none() {
                cx.warn(format!(
                    "`sources[{i}].usage_count` needs a `usage_window` that gives its period"
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

/// v0.1 constructs that v0.2 superseded.
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
///
/// `runtime` is the one field the type requires. `executor` and `attester`
/// are optional, but a consumer must not show a value as attested without an
/// `attester`, so its absence is noted.
fn check_computation(cx: &mut Context, doc: &Document) {
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
            "no computation: set `computation` to a path or put a fenced code block in a \
             `# Computation` section; consumers treat the concept as not attestable",
        ),
        ComputationSource::Inline(inline) if !inline.fenced => cx.warn_fixable(
            "the `# Computation` section has an indented code block but no fenced one; \
             consumers locate only a fenced block, so the concept is not attestable",
        ),
        ComputationSource::Inline(inline) if inline.fenced_blocks > 1 => cx.warn(format!(
            "the `# Computation` section has {} fenced code blocks; put exactly one there, \
             since consumers use the first",
            inline.fenced_blocks
        )),
        ComputationSource::File(_) if contract.fenced_blocks > 0 => cx.warn(
            "`computation` names a file and the `# Computation` section also has a fenced \
             block; the spec asks for one or the other",
        ),
        ComputationSource::File(_) if contract.has_redundant_inline => cx.warn(
            "`computation` names a file and the body also has a `# Computation` block; the \
             spec asks for one or the other",
        ),
        _ => {}
    }

    check_contract_entries(cx, fm, &contract);
}

/// The entries of the contract that have required keys of their own:
/// `parameters`, `executor`, and `attester`.
fn check_contract_entries(
    cx: &mut Context,
    fm: &Frontmatter,
    contract: &okf_core::AttestedComputation,
) {
    if let Some(value) = fm.get("parameters").filter(|v| !v.is_blank()) {
        for (i, item) in value.as_list().into_iter().enumerate() {
            match Parameter::from_value(item) {
                None => cx.warn(format!(
                    "`parameters[{i}]` should be a `{{ name, type, required }}` mapping, found \
                     {}; consumers ignore it",
                    type_name(item)
                )),
                Some(parameter) if !parameter.is_complete() => {
                    let missing = match (&parameter.name, &parameter.type_) {
                        (None, None) => "`name` and `type`",
                        (None, Some(_)) => "`name`",
                        _ => "`type`",
                    };
                    cx.warn(format!(
                        "`parameters[{i}]` is missing {missing}; consumers ignore an entry \
                         without both"
                    ));
                }
                Some(_) => {}
            }
        }
    }

    if let Some(value) = fm.get("executor").filter(|v| !v.is_blank()) {
        match contract.executor.as_ref() {
            None => cx.warn(format!(
                "`executor` should be a mapping with `resource` and `receipt`, found {}",
                type_name(value)
            )),
            Some(executor) => {
                if executor.resource.is_none() {
                    cx.warn(
                        "`executor.resource` is missing; it names the run instructions or code",
                    );
                }
                if executor.receipt.is_empty() {
                    cx.warn(
                        "`executor.receipt` is empty; it lists the fields a run returns, which \
                         are the evidence the attester examines",
                    );
                }
            }
        }
    }

    match (
        fm.get("attester").filter(|v| !v.is_blank()),
        &contract.attester,
    ) {
        (None, _) => {
            cx.info(
                "no `attester`: consumers cannot show values from this computation as attested",
            );
        }
        (Some(value), None) => cx.warn(format!(
            "`attester` should be a mapping with `resource`, found {}",
            type_name(value)
        )),
        (Some(_), Some(attester)) if attester.resource.is_none() => {
            cx.warn("`attester.resource` is missing; it names the deterministic check");
        }
        _ => {}
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
            // A deprecated concept that names its replacement is allowed to
            // be linked from that replacement and back.
            let replacement = bundle
                .superseded_by(&link.target)
                .filter(|replacement| **replacement != cx.id)
                .map_or_else(String::new, |replacement| {
                    format!("; it is superseded by `{replacement}`")
                });
            cx.warn(format!(
                "links to deprecated concept `{}`{replacement}",
                link.target
            ));
        }
    }
}

/// Path fields that should lead into the bundle but do not.
///
/// A path field resolves like a link: from the bundle root when it starts
/// with `/`, otherwise from the directory of its file. A value that only
/// resolves from the bundle root is how earlier versions wrote it; that still
/// works for a bundle that declares OKF 0.2 or earlier (or nothing), and is
/// reported so the producer can add the `/`.
///
/// A top-level `resource` is only checked when it is written unambiguously as
/// a path (`/...`, `./...`, `../...`). A bare `resource` such as
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
        match bundle.resolve_path(&id, target) {
            PathResolution::External | PathResolution::Found(_) => {}
            PathResolution::Fallback(_) => {
                let rooted = bundle
                    .rooted_path_suggestion(&id, target)
                    .unwrap_or_else(|| format!("/{target}"));
                cx.warn_fixable(format!(
                    "`{field}` resolves only from the bundle root, through the fallback for \
                     bundles from OKF v0.2 or earlier; write it as `{rooted}`: {raw}"
                ));
            }
            PathResolution::Missing => match bundle.rooted_path_suggestion(&id, target) {
                Some(rooted) => cx.warn_fixable(format!(
                    "`{field}` does not resolve from the directory of its file; it would \
                     resolve from the bundle root as `{rooted}`: {raw}"
                )),
                None => cx.warn(format!(
                    "`{field}` does not resolve to a file in the bundle: {raw}"
                )),
            },
        }
    }
}

/// Extension keys that do not start with `x-`.
///
/// Permitted: producers may add keys at any level, and consumers keep them.
/// It is noted because a future OKF version could define a key of the same
/// name, which it will never do for one that starts with `x-`.
fn check_extension_keys(cx: &mut Context, fm: &Frontmatter) {
    let unprefixed = fm.unprefixed_extension_keys();
    if !unprefixed.is_empty() {
        cx.info(format!(
            "extension key(s) without the `x-` prefix: `{}`; a future OKF version could \
             define a key of the same name",
            unprefixed.join("`, `")
        ));
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
    // Markdown files that exist but are not concepts. An index may list one
    // (a README, say); that is not a stale entry.
    let non_concepts: HashSet<&Path> = bundle
        .non_concepts()
        .iter()
        .map(|file| file.path.as_path())
        .collect();

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
            .filter(|c| !non_concepts.contains(c.to_path(bundle.root()).as_path()))
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

/// The reserved files: `index.md` and `log.md`.
///
/// These are the only checks besides frontmatter parsing that decide
/// conformance. A bundle-root `index.md` may carry frontmatter, which is the
/// bundle metadata; an index below the root has none, and neither does a
/// `log.md`.
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
        let is_root = path == &root_index;
        if !is_root && Document::has_frontmatter_block(&text) {
            report.error(
                Some(path.clone()),
                None,
                "an index.md below the bundle root has no frontmatter".to_string(),
            );
            continue;
        }
        match Document::parse(&text) {
            Ok(doc) => {
                for key in
                    Value::Mapping(doc.frontmatter.as_mapping().clone()).duplicate_key_paths()
                {
                    report.error(
                        Some(path.clone()),
                        None,
                        format!("frontmatter repeats the key `{key}`; keys must be unique"),
                    );
                }
            }
            Err(error) => report.error(
                Some(path.clone()),
                None,
                format!("frontmatter does not parse as a YAML mapping: {error}"),
            ),
        }
    }

    validate_logs(bundle, report);
}

/// Every `log.md`: no frontmatter, `YYYY-MM-DD` date headings, and a flat
/// list of dated entries.
fn validate_logs(bundle: &Bundle, report: &mut Report) {
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
        if Document::has_frontmatter_block(&text) {
            // An unterminated block is reported by the structural pass below.
            let fixable = Document::parse(&text).is_ok();
            report.add_fixable(
                Severity::Error,
                Some(path.clone()),
                None,
                "log.md has no frontmatter".to_string(),
                fixable,
            );
        }
        // What makes a log non-conformant is frontmatter and a date heading
        // that is not `YYYY-MM-DD`. The rest of its shape (a list of entries
        // under each date) is the format's convention: a log with an intro
        // paragraph or a wrapped line still reads, so those are warnings.
        let log = Log::parse(&text);
        for issue in log.structural_errors(&text) {
            report.warn(Some(path.clone()), None, issue);
        }
        for bad in log.invalid_dates() {
            report.error(
                Some(path.clone()),
                None,
                format!("log date heading is not ISO-8601 `YYYY-MM-DD`: {bad:?}"),
            );
        }
        for (date, previous) in log.misordered_dates() {
            report.warn(
                Some(path.clone()),
                None,
                format!("log.md lists the newest date first: {date:?} follows {previous:?}"),
            );
        }
        let mut seen = HashMap::new();
        for day in &log.days {
            *seen.entry(day.date.clone()).or_insert(0) += 1;
        }
        let mut repeated: Vec<&String> = seen
            .iter()
            .filter(|(_, count)| **count > 1)
            .map(|(date, _)| date)
            .collect();
        repeated.sort();
        for date in repeated {
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

/// `true` for a value shaped like an SPDX license expression: identifiers
/// joined by `AND`, `OR`, or `WITH`. Whether each identifier is a registered
/// license is not checked, so a bare file name such as `LICENSE` passes too;
/// the aim is only to catch a value that is neither an expression nor a path.
fn is_spdx_expression(license: &str) -> bool {
    let is_identifier = |word: &str| {
        let word = word.trim_matches(|c| c == '(' || c == ')');
        !word.is_empty()
            && word
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'))
    };
    let mut expect_identifier = true;
    for word in license.split_whitespace() {
        if expect_identifier {
            if !is_identifier(word) {
                return false;
            }
        } else if !matches!(word, "AND" | "OR" | "WITH") {
            return false;
        }
        expect_identifier = !expect_identifier;
    }
    // An expression ends on an identifier, never on an operator.
    !expect_identifier
}

/// The bundle metadata of the bundle-root `index.md`.
///
/// Producers should put a bundle-root `index.md` with `okf_version` in each
/// bundle. Its absence is never an error: a bundle that declares nothing is
/// read with the fallbacks for earlier versions.
fn check_bundle_metadata(bundle: &Bundle, report: &mut Report) {
    let root_index = bundle.root().join("index.md");
    if !root_index.is_file() {
        report.warn(
            None,
            None,
            format!(
                "no bundle-root index.md; add one that declares `okf_version: \"{}\"` \
                 (run `okf index`)",
                okf_core::OKF_VERSION
            ),
        );
        return;
    }
    let metadata = bundle.metadata();
    if metadata.okf_version.is_none() {
        // Declaring the current version turns off the fallback that resolves
        // root-relative path fields, so say so when the bundle relies on it.
        let relies_on_fallback = !bundle.legacy_path_fields().is_empty();
        let note = if relies_on_fallback {
            "; first write the path fields that resolve only from the bundle root with a \
             leading `/`"
        } else {
            ""
        };
        report.add_fixable(
            Severity::Warning,
            Some(root_index.clone()),
            None,
            format!(
                "the bundle-root index.md does not declare `okf_version`; add \
                 `okf_version: \"{}\"`{note}",
                okf_core::OKF_VERSION
            ),
            true,
        );
    }
    if let Some(license) = &metadata.license
        && !is_spdx_expression(license)
        && bundle.resolve_path_in(&[], license).path().is_none()
    {
        report.warn(
            Some(root_index),
            None,
            format!(
                "`license` is neither an SPDX expression nor a file in the bundle: {license:?}"
            ),
        );
    }
}

/// The `okf_version` a bundle declares.
///
/// Never an error. The spec is explicit that a consumer which does not know
/// the declared version should try best-effort consumption rather than
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
            "bundle declares OKF v{declared}; it is read as v{} with the fallbacks for earlier \
             versions",
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
    fm.get(key).is_none_or(Value::is_blank)
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

/// Checks an RFC 3339 date-time with a time of day and an explicit UTC offset.
///
/// This is the stricter of the two forms an OKF timestamp may take; a
/// date-only value is also valid there, as the start of that day in UTC.
#[must_use]
pub fn is_iso8601_datetime(s: &str) -> bool {
    DateTime::parse_rfc3339(s).is_some_and(|datetime| datetime.has_time)
}

fn check_link_anchors(cx: &mut Context, bundle: &Bundle, doc: &Document) {
    let own_slugs = concept_heading_slugs(doc);

    for link in doc.links() {
        if link.kind == okf_core::LinkKind::Anchor {
            let anchor = link.target.trim_start_matches('#');
            if !anchor.is_empty() {
                let decoded = anchor.replace("%20", " ").replace('+', " ");
                let slug = okf_core::heading_slug(&decoded);
                if !own_slugs.contains(&slug)
                    && !own_slugs.contains(anchor)
                    && !own_slugs.contains(&decoded.to_lowercase())
                {
                    cx.warn(format!(
                        "internal link anchor `#{anchor}` does not match any heading in this document"
                    ));
                }
            }
        } else if let Some(anchor_idx) = link.target.find('#') {
            let anchor = &link.target[anchor_idx + 1..];
            if !anchor.is_empty()
                && let Some(target_id) = link.resolve(&cx.id)
                && let Some(target_concept) = bundle.get(&target_id)
            {
                let target_slugs = concept_heading_slugs(&target_concept.document);
                let decoded = anchor.replace("%20", " ").replace('+', " ");
                let slug = okf_core::heading_slug(&decoded);
                if !target_slugs.contains(&slug)
                    && !target_slugs.contains(anchor)
                    && !target_slugs.contains(&decoded.to_lowercase())
                {
                    cx.warn(format!(
                        "link anchor `#{anchor}` does not match any heading in target concept `{target_id}`"
                    ));
                }
            }
        }
    }
}

fn concept_heading_slugs(doc: &Document) -> std::collections::HashSet<String> {
    let mut slugs = std::collections::HashSet::new();
    for heading in okf_core::extract_headings(&doc.body) {
        let text = heading.text;
        if !text.is_empty() {
            slugs.insert(heading.slug());
            slugs.insert(text.to_lowercase());
            slugs.insert(text.replace(['-', '_'], " ").to_lowercase());
        }
    }
    slugs
}

#[cfg(test)]
mod tests {
    use super::is_spdx_expression;

    #[test]
    fn recognizes_spdx_expressions() {
        for ok in [
            "CC-BY-4.0",
            "Apache-2.0",
            "MIT OR Apache-2.0",
            "GPL-2.0-only WITH Classpath-exception-2.0",
            "(MIT AND BSD-3-Clause) OR Apache-2.0",
        ] {
            assert!(is_spdx_expression(ok), "{ok}");
        }
        for bad in [
            "",
            "/LICENSE",
            "docs/LICENSE.md",
            "MIT OR",
            "all rights reserved",
            "MIT Apache-2.0",
        ] {
            assert!(!is_spdx_expression(bad), "{bad}");
        }
    }
}
