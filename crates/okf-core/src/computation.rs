//! Attested Computation concepts.
//!
//! An Attested Computation carries not just what a value *means* but a
//! sanctioned way to *compute* it, so a consumer can confirm an agent ran the
//! blessed computation instead of improvising its own. Provenance
//! answers "where did this claim come from"; attestation answers "was this
//! number produced the way we said it must be."
//!
//! ```markdown
//! ---
//! type: Attested Computation
//! runtime: bigquery
//! parameters:
//!   - { name: year, type: integer, required: true }
//! executor:
//!   resource: references/skills/run-on-bq.md
//!   receipt: [job_id, executed_sql, result]
//! attester:
//!   resource: references/attesters/revenue.py
//! ---
//!
//! # Computation
//!
//!     SELECT SUM(amount) AS revenue
//!     FROM finance.recognized_revenue
//!     WHERE fiscal_year = @year
//! ```
//!
//! **This crate records and checks the contract; it never executes anything.**
//! Running the computation, producing a receipt, and running the attester over
//! that receipt are consumer-side concerns, and the runtime artifacts they
//! produce are explicitly *not* stored in the bundle. What
//! [`AttestedComputation`] gives you is the contract in typed form: the
//! `runtime` that defines what `parameters` mean, the computation itself
//! (inline or by path), and the executor/attester interfaces.

use crate::links;
use crate::yaml::Value;
use std::fmt;

/// The `type` value that marks a concept as an Attested Computation.
pub const ATTESTED_COMPUTATION_TYPE: &str = "Attested Computation";

/// The conventional body heading that introduces an inline computation.
pub const COMPUTATION_HEADING: &str = "Computation";

/// One typed, named hole an agent may fill.
///
/// Binding semantics follow `runtime`: the same entry is a SQL bind variable, a
/// dbt var, or a Python argument depending on it. An agent may supply only
/// *values* for declared parameters; it must not author or edit the
/// computation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Parameter {
    /// The parameter name.
    pub name: Option<String>,
    /// The parameter's declared type, e.g. `integer`, `string`.
    pub type_: Option<String>,
    /// Whether a value must be supplied.
    pub required: Option<bool>,
}

impl Parameter {
    /// Reads one `parameters` entry. Returns `None` when it is not a mapping.
    pub fn from_value(value: &Value) -> Option<Self> {
        let map = value.as_mapping()?;
        Some(Self {
            name: map.get("name").and_then(Value::as_display_string),
            type_: map.get("type").and_then(Value::as_display_string),
            required: map.get("required").and_then(Value::as_bool),
        })
    }

    /// Reads the whole `parameters` list.
    pub fn list_from_value(value: &Value) -> Vec<Self> {
        match value {
            Value::Sequence(items) => items.iter().filter_map(Self::from_value).collect(),
            Value::Mapping(_) => Self::from_value(value).into_iter().collect(),
            _ => Vec::new(),
        }
    }

    /// `true` when the parameter is explicitly marked required.
    #[must_use]
    pub fn is_required(&self) -> bool {
        self.required.unwrap_or(false)
    }
}

impl fmt::Display for Parameter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name.as_deref().unwrap_or("(unnamed)"))?;
        if let Some(t) = &self.type_ {
            write!(f, ": {t}")?;
        }
        if self.is_required() {
            f.write_str(" (required)")?;
        }
        Ok(())
    }
}

/// How the computation is run.
///
/// `resource` names run instructions or code that a runner (an agent, or
/// deterministic consumer code) follows. `receipt` declares the fields a run
/// must return: the evidence the attester inspects, for example a `BigQuery`
/// `job_id` and the SQL the job actually executed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Executor {
    /// Path to run instructions or code.
    pub resource: Option<String>,
    /// The fields a run must return.
    pub receipt: Vec<String>,
}

impl Executor {
    /// Reads an `executor` mapping. Returns `None` when it is not a mapping.
    pub fn from_value(value: &Value) -> Option<Self> {
        let map = value.as_mapping()?;
        Some(Self {
            resource: map.get("resource").and_then(Value::as_display_string),
            receipt: match map.get("receipt") {
                Some(Value::Sequence(items)) => {
                    items.iter().filter_map(Value::as_display_string).collect()
                }
                Some(other) => other.as_display_string().into_iter().collect(),
                None => Vec::new(),
            },
        })
    }
}

/// The deterministic check.
///
/// `resource` names code (no LLM) that takes a receipt and returns a verdict.
/// It is meant to run consumer-side.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Attester {
    /// Path to the attester code.
    pub resource: Option<String>,
}

impl Attester {
    /// Reads an `attester` mapping. Returns `None` when it is not a mapping.
    pub fn from_value(value: &Value) -> Option<Self> {
        let map = value.as_mapping()?;
        Some(Self {
            resource: map.get("resource").and_then(Value::as_display_string),
        })
    }
}

/// A computation held in the body under `# Computation`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineComputation {
    /// The code, dedented, without the fence or indent.
    pub code: String,
    /// The fence info string (`sql`, `python`, …), when the block is fenced and
    /// carries one.
    pub language: Option<String>,
    /// `true` for a ```` ``` ````/`~~~` fenced block, `false` for an indented
    /// one. The spec's prose says "fenced"; its own examples are indented, so
    /// both are read.
    pub fenced: bool,
}

/// Where the sanctioned computation lives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComputationSource {
    /// A code block in the body under `# Computation`.
    Inline(InlineComputation),
    /// The path named by the `computation` frontmatter key.
    File(String),
    /// Neither form is present, so the contract is incomplete.
    Missing,
}

impl ComputationSource {
    /// The inline code, if the computation is inline.
    #[must_use]
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Inline(c) => Some(&c.code),
            _ => None,
        }
    }

    /// The path, if the computation is held in a file.
    #[must_use]
    pub fn path(&self) -> Option<&str> {
        match self {
            Self::File(p) => Some(p),
            _ => None,
        }
    }

    /// `true` when neither an inline block nor a `computation` path is present.
    #[must_use]
    pub fn is_missing(&self) -> bool {
        *self == Self::Missing
    }
}

impl fmt::Display for ComputationSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Inline(c) => {
                write!(f, "inline ({} line(s))", c.code.lines().count())
            }
            Self::File(p) => write!(f, "file {p}"),
            Self::Missing => f.write_str("(missing)"),
        }
    }
}

/// The contract of an `Attested Computation` concept: its top-level frontmatter
/// plus the computation itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttestedComputation {
    /// REQUIRED for this type: how to run the computation, and so how the
    /// executor and attester interpret it and what `parameters` mean.
    pub runtime: Option<String>,
    /// The typed, named holes an agent may fill.
    pub parameters: Vec<Parameter>,
    /// The sanctioned computation, inline or by path.
    pub computation: ComputationSource,
    /// How the computation is run.
    pub executor: Option<Executor>,
    /// The deterministic check over a run's receipt.
    pub attester: Option<Attester>,
    /// `true` when the body carries a `# Computation` block *and* the
    /// `computation` key names a file. The spec asks for one or the other, so this
    /// flags a contract whose two halves may disagree.
    pub has_redundant_inline: bool,
}

impl AttestedComputation {
    /// Reads the contract from a concept's frontmatter and body.
    ///
    /// This does not check that the concept's `type` is
    /// [`ATTESTED_COMPUTATION_TYPE`], since the computation keys are ordinary
    /// frontmatter and a producer may use them on another type. Use
    /// [`Frontmatter::is_attested_computation`](crate::Frontmatter::is_attested_computation)
    /// to test the type.
    pub fn from_parts(frontmatter: &crate::Frontmatter, body: &str) -> Self {
        let inline = extract_inline_computation(body);
        let path = frontmatter
            .get("computation")
            .and_then(Value::as_display_string)
            .filter(|p| !p.trim().is_empty());

        let (computation, has_redundant_inline) = match (path, inline) {
            (Some(p), Some(_)) => (ComputationSource::File(p), true),
            (Some(p), None) => (ComputationSource::File(p), false),
            (None, Some(c)) => (ComputationSource::Inline(c), false),
            (None, None) => (ComputationSource::Missing, false),
        };

        Self {
            runtime: frontmatter
                .get("runtime")
                .and_then(Value::as_display_string)
                .filter(|r| !r.trim().is_empty()),
            parameters: frontmatter
                .get("parameters")
                .map(Parameter::list_from_value)
                .unwrap_or_default(),
            computation,
            executor: frontmatter.get("executor").and_then(Executor::from_value),
            attester: frontmatter.get("attester").and_then(Attester::from_value),
            has_redundant_inline,
        }
    }

    /// The parameters an agent must supply a value for.
    pub fn required_parameters(&self) -> impl Iterator<Item = &Parameter> {
        self.parameters.iter().filter(|p| p.is_required())
    }

    /// The path-valued fields of this contract, as `(field name, raw path)`
    /// pairs, the inputs to [`links::field_path_candidates`] when checking
    /// that a contract points at something real.
    #[must_use]
    pub fn path_fields(&self) -> Vec<(&'static str, &str)> {
        let mut out = Vec::new();
        if let Some(p) = self.computation.path() {
            out.push(("computation", p));
        }
        if let Some(r) = self.executor.as_ref().and_then(|e| e.resource.as_deref()) {
            out.push(("executor.resource", r));
        }
        if let Some(r) = self.attester.as_ref().and_then(|a| a.resource.as_deref()) {
            out.push(("attester.resource", r));
        }
        out
    }
}

/// Extracts the code block under a `# Computation` heading.
///
/// The section runs from the heading to the next heading of the same or a
/// higher level. Within it, the first fenced block wins; failing that, the
/// first indented block does, because the spec's own examples are written that
/// way. Returns `None` when there is no `# Computation` section or it holds no
/// code.
#[must_use]
pub fn extract_inline_computation(body: &str) -> Option<InlineComputation> {
    let section = computation_section(body)?;
    fenced_block(&section).or_else(|| indented_block(&section))
}

/// The lines of the `# Computation` section, heading excluded.
fn computation_section(body: &str) -> Option<Vec<&str>> {
    let mut lines = body.lines();
    let mut level = 0;
    for line in lines.by_ref() {
        if let Some((l, title)) = heading(line)
            && title.eq_ignore_ascii_case(COMPUTATION_HEADING)
        {
            level = l;
            break;
        }
    }
    if level == 0 {
        return None;
    }
    let mut section = Vec::new();
    for line in lines {
        if let Some((l, _)) = heading(line)
            && l <= level
        {
            break;
        }
        section.push(line);
    }
    Some(section)
}

/// Splits an ATX heading into its level and title.
fn heading(line: &str) -> Option<(usize, &str)> {
    let t = line.trim_start();
    let hashes = t.len() - t.trim_start_matches('#').len();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &t[hashes..];
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return None;
    }
    Some((hashes, rest.trim().trim_end_matches('#').trim()))
}

/// The first fenced code block in a section.
fn fenced_block(section: &[&str]) -> Option<InlineComputation> {
    for (i, line) in section.iter().enumerate() {
        let t = line.trim_start();
        for marker in ["```", "~~~"] {
            if let Some(info) = t.strip_prefix(marker) {
                let info = info.trim();
                let language = (!info.is_empty()).then(|| info.to_string());
                return finish_fenced(section, i, marker, language);
            }
        }
    }
    None
}

fn finish_fenced(
    section: &[&str],
    open: usize,
    marker: &str,
    language: Option<String>,
) -> Option<InlineComputation> {
    let indent = section[open].len() - section[open].trim_start().len();
    let mut code: Vec<String> = Vec::new();
    for line in &section[open + 1..] {
        if line.trim_start().starts_with(marker) {
            break;
        }
        code.push(dedent(line, indent));
    }
    let code = trim_blank_edges(code);
    (!code.is_empty()).then(|| InlineComputation {
        code: code.join("\n"),
        language,
        fenced: true,
    })
}

/// The first indented (4-space or tab) code block in a section.
fn indented_block(section: &[&str]) -> Option<InlineComputation> {
    let mut code: Vec<String> = Vec::new();
    let mut started = false;
    for line in section {
        let is_code = line.starts_with("    ") || line.starts_with('\t');
        if is_code {
            started = true;
            code.push(dedent(line, 4));
        } else if line.trim().is_empty() {
            if started {
                code.push(String::new());
            }
        } else if started {
            break;
        }
    }
    let code = trim_blank_edges(code);
    (!code.is_empty()).then(|| InlineComputation {
        code: code.join("\n"),
        language: None,
        fenced: false,
    })
}

/// Removes up to `n` columns of leading whitespace (a tab counts as one level).
fn dedent(line: &str, n: usize) -> String {
    if let Some(rest) = line.strip_prefix('\t') {
        return rest.to_string();
    }
    let strip = line.len() - line.trim_start_matches(' ').len();
    line[strip.min(n)..].to_string()
}

fn trim_blank_edges(mut lines: Vec<String>) -> Vec<String> {
    while lines.first().is_some_and(|l| l.trim().is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    lines
}

/// Resolves a contract's path-valued fields to bundle-relative candidates,
/// as `(field, raw, candidates)`.
#[must_use]
pub fn contract_path_candidates(
    contract: &AttestedComputation,
    from: &crate::ConceptId,
) -> Vec<(&'static str, String, Vec<String>)> {
    contract
        .path_fields()
        .into_iter()
        .map(|(field, raw)| {
            let candidates = links::field_path_candidates(raw, from);
            (field, raw.to_string(), candidates)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Document;

    const REVENUE: &str = "\
---
type: Attested Computation
title: Revenue for fiscal year
runtime: bigquery
parameters:
  - { name: year, type: integer, required: true }
executor:
  resource: references/skills/run-on-bq.md
  receipt: [job_id, executed_sql, result]
attester:
  resource: references/attesters/revenue.py
---

# Computation

    SELECT SUM(amount) AS revenue
    FROM finance.recognized_revenue
    WHERE fiscal_year = @year

The computation binds only the declared `parameters`, per the recognition
policy.[^rev-policy]

[^rev-policy]: Revenue recognition policy
";

    #[test]
    fn reads_the_spec_contract() {
        let doc = Document::parse(REVENUE).unwrap();
        assert!(doc.frontmatter.is_attested_computation());
        let c = AttestedComputation::from_parts(&doc.frontmatter, &doc.body);

        assert_eq!(c.runtime.as_deref(), Some("bigquery"));
        assert_eq!(c.parameters.len(), 1);
        assert_eq!(c.parameters[0].name.as_deref(), Some("year"));
        assert_eq!(c.parameters[0].type_.as_deref(), Some("integer"));
        assert!(c.parameters[0].is_required());
        assert_eq!(c.required_parameters().count(), 1);

        let executor = c.executor.as_ref().unwrap();
        assert_eq!(
            executor.resource.as_deref(),
            Some("references/skills/run-on-bq.md")
        );
        assert_eq!(executor.receipt, vec!["job_id", "executed_sql", "result"]);
        assert_eq!(
            c.attester.as_ref().unwrap().resource.as_deref(),
            Some("references/attesters/revenue.py")
        );

        let code = c.computation.code().unwrap();
        assert!(code.starts_with("SELECT SUM(amount) AS revenue"));
        assert!(code.ends_with("WHERE fiscal_year = @year"));
        // Prose after the block is not part of the computation.
        assert!(!code.contains("binds only"));
        assert!(!c.has_redundant_inline);
    }

    #[test]
    fn fenced_blocks_win_and_carry_a_language() {
        let body = "# Computation\n\n```sql\nSELECT 1\n```\n\nProse.\n";
        let c = extract_inline_computation(body).unwrap();
        assert!(c.fenced);
        assert_eq!(c.language.as_deref(), Some("sql"));
        assert_eq!(c.code, "SELECT 1");
    }

    #[test]
    fn file_form_replaces_the_body_block() {
        let doc = Document::parse(
            "---\ntype: Attested Computation\nruntime: bigquery\n\
             computation: references/computations/lib/revenue.sql\n---\n\n# Definition\n\nProse.\n",
        )
        .unwrap();
        let c = AttestedComputation::from_parts(&doc.frontmatter, &doc.body);
        assert_eq!(
            c.computation.path(),
            Some("references/computations/lib/revenue.sql")
        );
        assert!(!c.has_redundant_inline);
        assert_eq!(
            c.path_fields(),
            vec![("computation", "references/computations/lib/revenue.sql")]
        );
    }

    #[test]
    fn both_forms_present_is_flagged() {
        let doc = Document::parse(
            "---\ntype: Attested Computation\ncomputation: x.sql\n---\n\n# Computation\n\n    SELECT 1\n",
        )
        .unwrap();
        let c = AttestedComputation::from_parts(&doc.frontmatter, &doc.body);
        assert!(c.has_redundant_inline);
        assert_eq!(c.computation.path(), Some("x.sql"));
    }

    #[test]
    fn missing_computation_is_representable() {
        let doc =
            Document::parse("---\ntype: Attested Computation\n---\n\n# Definition\n").unwrap();
        let c = AttestedComputation::from_parts(&doc.frontmatter, &doc.body);
        assert!(c.computation.is_missing());
        assert!(c.runtime.is_none());
    }

    #[test]
    fn section_ends_at_the_next_same_level_heading() {
        let body = "# Computation\n\n## Detail\n\n    SELECT 1\n\n# Notes\n\n    SELECT 2\n";
        let c = extract_inline_computation(body).unwrap();
        assert_eq!(c.code, "SELECT 1");
    }

    #[test]
    fn dbt_template_syntax_survives() {
        let body = "# Computation\n\n    SELECT gross_profit\n    FROM {{ ref('fct_income_statement') }}\n    WHERE fiscal_year = {{ var('year') }}\n";
        let c = extract_inline_computation(body).unwrap();
        assert!(c.code.contains("{{ ref('fct_income_statement') }}"));
        assert_eq!(c.code.lines().count(), 3);
    }
}
