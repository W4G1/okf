//! Attested Computation concepts.
//!
//! An Attested Computation carries not just what a value *means* but a
//! sanctioned way to *compute* it, so a consumer can confirm an agent ran the
//! approved computation instead of writing its own. Provenance
//! answers "where did this claim come from"; attestation answers "was this
//! number made the way that we said it must be."
//!
//! ````markdown
//! ---
//! type: Attested Computation
//! runtime: bigquery
//! parameters:
//!   - { name: year, type: integer, required: true }
//! executor:
//!   resource: /references/skills/run-on-bq.md
//!   receipt: [job_id, executed_sql, result]
//! attester:
//!   resource: /references/attesters/sql-equality.py
//! ---
//!
//! # Computation
//!
//! ```sql
//! SELECT SUM(amount) AS revenue
//! FROM finance.recognized_revenue
//! WHERE fiscal_year = @year
//! ```
//! ````
//!
//! **This crate records and checks the contract; it never executes anything.**
//! Running the computation, producing a receipt, and running the attester over
//! that receipt are consumer-side concerns, and the runtime artifacts they
//! produce are explicitly *not* stored in the bundle. What
//! [`AttestedComputation`] gives you is the contract in typed form: the
//! `runtime` that defines what `parameters` mean, the computation itself
//! (inline or by path), and the executor/attester interfaces.
//!
//! ## Finding the computation
//!
//! 1. If `computation` is present, the computation is the file it refers to.
//! 2. Otherwise it is the first fenced code block in the section with the
//!    heading `Computation`, at any ATX heading level. The section ends at the
//!    next heading of the same or a higher level.
//! 3. If the section has no fenced code block, or the file does not exist, the
//!    computation is missing and the concept is not attestable.
//!
//! Earlier versions of the specification wrote their examples as indented
//! code blocks, so [`extract_inline_computation`] still reads one when the
//! section has no fenced block. Such a block is reported with
//! [`InlineComputation::fenced`] set to `false`, and it does not make the
//! concept attestable: see [`AttestedComputation::is_attestable`].

use crate::links;
use crate::yaml::Value;
use std::fmt;

/// The `type` value that marks a concept as an Attested Computation.
pub const ATTESTED_COMPUTATION_TYPE: &str = "Attested Computation";

/// The conventional body heading that introduces an inline computation.
pub const COMPUTATION_HEADING: &str = "Computation";

/// The parameter types the specification names as common. Others are specific
/// to a runtime, so a type outside this list is not a mistake.
pub const COMMON_PARAMETER_TYPES: [&str; 6] = [
    "string",
    "integer",
    "number",
    "boolean",
    "date",
    "timestamp",
];

/// Reads a scalar at `key`, treating a null or empty value as absent.
fn non_blank(map: &crate::yaml::Mapping, key: &str) -> Option<String> {
    map.get(key)
        .and_then(Value::as_display_string)
        .filter(|s| !s.trim().is_empty())
}

/// One typed, named hole an agent may fill.
///
/// Binding semantics follow `runtime`: the same entry is a SQL bind variable, a
/// dbt var, or a Python argument depending on it. An agent may supply only
/// *values* for declared parameters; it must not author or edit the
/// computation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Parameter {
    /// The parameter name. REQUIRED within an entry.
    pub name: Option<String>,
    /// The parameter's declared type, e.g. `integer`, `string`. REQUIRED
    /// within an entry.
    pub type_: Option<String>,
    /// Whether a value must be supplied. Optional; the default is `false`.
    pub required: Option<bool>,
}

impl Parameter {
    /// Reads one `parameters` entry. Returns `None` when it is not a mapping.
    pub fn from_value(value: &Value) -> Option<Self> {
        let map = value.as_mapping()?;
        Some(Self {
            name: non_blank(map, "name"),
            type_: non_blank(map, "type"),
            required: map.get("required").and_then(Value::as_bool),
        })
    }

    /// Reads the whole `parameters` list. One mapping is read as a list with
    /// one entry. Every mapping-shaped entry is returned, including one that
    /// lacks `name` or `type`, so a validator can report it.
    pub fn list_from_value(value: &Value) -> Vec<Self> {
        match value {
            Value::Sequence(items) => items.iter().filter_map(Self::from_value).collect(),
            Value::Mapping(_) => Self::from_value(value).into_iter().collect(),
            _ => Vec::new(),
        }
    }

    /// `true` when the entry has both keys it requires, `name` and `type`. A
    /// consumer ignores an entry that does not.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        self.name.is_some() && self.type_.is_some()
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
            resource: non_blank(map, "resource"),
            receipt: map.get("receipt").map_or_else(Vec::new, |value| {
                value
                    .as_list()
                    .into_iter()
                    .filter_map(Value::as_display_string)
                    .filter(|field| !field.trim().is_empty())
                    .collect()
            }),
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
    #[must_use]
    pub fn from_value(value: &Value) -> Option<Self> {
        let map = value.as_mapping()?;
        Some(Self {
            resource: non_blank(map, "resource"),
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
    /// one.
    ///
    /// The specification locates the computation in a *fenced* block only. An
    /// indented block is read as a courtesy to bundles written from the
    /// indented examples of earlier spec versions, but it does not make the
    /// concept attestable until it is fenced.
    pub fenced: bool,
    /// How many fenced code blocks the `Computation` section holds. Producers
    /// should write exactly one; the first is the computation.
    pub fenced_blocks: usize,
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

    /// `true` when the computation is given in a form the specification
    /// locates: a `computation` path, or a fenced block in the `Computation`
    /// section. An indented block does not count.
    ///
    /// Whether a `computation` path names a file that exists is a question for
    /// the bundle; see
    /// [`Bundle::is_attestable`](crate::Bundle::is_attestable).
    #[must_use]
    pub const fn is_locatable(&self) -> bool {
        match self {
            Self::Inline(inline) => inline.fenced,
            Self::File(_) => true,
            Self::Missing => false,
        }
    }
}

impl fmt::Display for ComputationSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Inline(c) if c.fenced => {
                write!(f, "inline ({} line(s))", c.code.lines().count())
            }
            Self::Inline(c) => {
                write!(
                    f,
                    "inline ({} line(s), indented: fence it to make it attestable)",
                    c.code.lines().count()
                )
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
    /// The typed, named holes an agent may fill. Entries without both `name`
    /// and `type` are left out.
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
    /// How many fenced code blocks the body's `Computation` section holds,
    /// whichever form the computation takes. Producers should write exactly
    /// one for an inline computation and none beside a `computation` path.
    pub fenced_blocks: usize,
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
        let fenced_blocks = inline.as_ref().map_or(0, |inline| inline.fenced_blocks);
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
            parameters: frontmatter.parameters(),
            computation,
            executor: frontmatter.executor(),
            attester: frontmatter.attester(),
            has_redundant_inline,
            fenced_blocks,
        }
    }

    /// Whether a consumer may show a value from this computation as attested,
    /// as far as the contract alone can tell.
    ///
    /// That needs a `runtime`, an `attester` that names its code, and a
    /// computation in a form the specification locates. A concept that fails
    /// any of these is still readable; its values are simply not attested.
    ///
    /// This does not look at the file system. A `computation` path that names
    /// a missing file also makes the concept not attestable, which only
    /// [`Bundle::is_attestable`](crate::Bundle::is_attestable) can check.
    #[must_use]
    pub fn is_attestable(&self) -> bool {
        self.runtime.is_some()
            && self
                .attester
                .as_ref()
                .is_some_and(|attester| attester.resource.is_some())
            && self.computation.is_locatable()
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
/// The section runs from a heading named `Computation`, at any ATX level, to
/// the next heading of the same or a higher level. Within it, the first fenced
/// block is the computation. Failing that, the first indented block is
/// returned with [`InlineComputation::fenced`] set to `false`: earlier
/// versions of the specification wrote their examples that way, but a
/// conforming consumer does not locate such a block. Returns `None` when there
/// is no `Computation` section or it holds no code.
///
/// Headings are not looked for inside fenced code, so a `# comment` line in a
/// Python or shell computation neither starts nor ends the section.
#[must_use]
pub fn extract_inline_computation(body: &str) -> Option<InlineComputation> {
    let (_, section) = computation_section(body)?;
    let fenced_blocks = count_fenced_blocks(&section);
    fenced_block(&section)
        .or_else(|| indented_block(&section))
        .map(|block| InlineComputation {
            fenced_blocks,
            ..block
        })
}

/// The fence a line opens, as `(marker character, run length)`.
fn fence_open(line: &str) -> Option<(char, usize)> {
    let t = line.trim_start();
    // More than three columns of indentation makes an indented code block.
    if line.len() - t.len() > 3 {
        return None;
    }
    let marker = t.chars().next().filter(|c| matches!(c, '`' | '~'))?;
    let run = t.chars().take_while(|c| *c == marker).count();
    (run >= 3).then_some((marker, run))
}

/// Whether `line` closes a fence opened with `run` or more `marker`s.
fn fence_close(line: &str, marker: char, run: usize) -> bool {
    let t = line.trim();
    t.chars().all(|c| c == marker) && t.chars().count() >= run
}

/// The fence that opens the inline computation, by its place in the body.
pub(crate) struct ComputationFence {
    /// Index of the fence line among the body's lines.
    pub line: usize,
    /// The fence itself, such as three backticks.
    pub marker: String,
    /// Whether the fence carries an info string (a language tag).
    pub has_info: bool,
}

/// Finds the opening fence of the first fenced block in the `Computation`
/// section, for a caller that wants to edit that one line.
pub(crate) fn computation_fence(body: &str) -> Option<ComputationFence> {
    let (start, section) = computation_section(body)?;
    section.iter().enumerate().find_map(|(i, line)| {
        let (marker, run) = fence_open(line)?;
        let text = line.trim_start();
        Some(ComputationFence {
            line: start + i,
            marker: marker.to_string().repeat(run),
            has_info: !text[run..].trim().is_empty(),
        })
    })
}

/// The `[start, end)` body lines of the indented code block that stands in
/// for the computation, when the `Computation` section has no fenced block.
pub(crate) fn indented_computation_lines(body: &str) -> Option<(usize, usize)> {
    let (start, section) = computation_section(body)?;
    if count_fenced_blocks(&section) > 0 {
        return None;
    }
    let mut first = None;
    let mut last = 0;
    for (i, line) in section.iter().enumerate() {
        if line.starts_with("    ") || line.starts_with('\t') {
            first.get_or_insert(i);
            last = i;
        } else if first.is_some() && !line.trim().is_empty() {
            break;
        }
    }
    first.map(|first| (start + first, start + last + 1))
}

/// The lines of the `# Computation` section, heading excluded, with the index
/// of the first of them among the body's lines.
fn computation_section(body: &str) -> Option<(usize, Vec<&str>)> {
    let mut section: Option<(usize, Vec<&str>)> = None;
    let mut first_line = 0;
    let mut fence: Option<(char, usize)> = None;

    for (index, line) in body.lines().enumerate() {
        if let Some((marker, run)) = fence {
            if fence_close(line, marker, run) {
                fence = None;
            }
        } else if let Some(open) = fence_open(line) {
            fence = Some(open);
        } else if let Some((level, title)) = heading(line) {
            match &section {
                Some((section_level, _)) if level <= *section_level => break,
                Some(_) => {}
                None => {
                    if title.eq_ignore_ascii_case(COMPUTATION_HEADING) {
                        section = Some((level, Vec::new()));
                        first_line = index + 1;
                    }
                    continue;
                }
            }
        }
        if let Some((_, lines)) = section.as_mut() {
            lines.push(line);
        }
    }
    section.map(|(_, lines)| (first_line, lines))
}

/// Splits an ATX heading into its level and title.
fn heading(line: &str) -> Option<(usize, &str)> {
    let t = line.trim_start();
    // Four or more columns of indentation is code, not a heading.
    if line.len() - t.len() > 3 {
        return None;
    }
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

/// How many fenced code blocks a section holds.
fn count_fenced_blocks(section: &[&str]) -> usize {
    let mut count = 0;
    let mut fence: Option<(char, usize)> = None;
    for line in section {
        if let Some((marker, run)) = fence {
            if fence_close(line, marker, run) {
                fence = None;
            }
        } else if let Some(open) = fence_open(line) {
            fence = Some(open);
            count += 1;
        }
    }
    count
}

/// The first fenced code block in a section.
fn fenced_block(section: &[&str]) -> Option<InlineComputation> {
    let (open, (marker, run)) = section
        .iter()
        .enumerate()
        .find_map(|(i, line)| fence_open(line).map(|fence| (i, fence)))?;

    let opening = section[open];
    let indent = opening.len() - opening.trim_start().len();
    let info = opening.trim_start()[run..].trim();
    // The info string's first word is the language; the rest is metadata.
    let language = info.split_whitespace().next().map(ToString::to_string);

    let mut code: Vec<String> = Vec::new();
    for line in &section[open + 1..] {
        if fence_close(line, marker, run) {
            break;
        }
        code.push(dedent(line, indent));
    }
    let code = trim_blank_edges(code);
    (!code.is_empty()).then(|| InlineComputation {
        code: code.join("\n"),
        language,
        fenced: true,
        fenced_blocks: 0,
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
        fenced_blocks: 0,
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

/// Resolves a contract's path fields to bundle-relative candidates, as
/// `(field, raw, candidates)`. See [`links::field_path_candidates`].
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
  resource: /references/skills/run-on-bq.md
  receipt: [job_id, executed_sql, result]
attester:
  resource: /references/attesters/sql-equality.py
---

# Computation

```sql
SELECT SUM(amount) AS revenue
FROM finance.recognized_revenue
WHERE fiscal_year = @year
```

Recognized revenue per the recognition policy.[^rev-policy]

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
            Some("/references/skills/run-on-bq.md")
        );
        assert_eq!(executor.receipt, vec!["job_id", "executed_sql", "result"]);
        assert_eq!(
            c.attester.as_ref().unwrap().resource.as_deref(),
            Some("/references/attesters/sql-equality.py")
        );

        let code = c.computation.code().unwrap();
        assert!(code.starts_with("SELECT SUM(amount) AS revenue"));
        assert!(code.ends_with("WHERE fiscal_year = @year"));
        // Prose after the block is not part of the computation.
        assert!(!code.contains("recognition policy"));
        assert!(!c.has_redundant_inline);
        assert_eq!(c.fenced_blocks, 1);
        assert!(c.is_attestable());
    }

    #[test]
    fn fenced_blocks_win_and_carry_a_language() {
        let body = "# Computation\n\n    SELECT 0\n\n```sql title=\"x\"\nSELECT 1\n```\n\nProse.\n";
        let c = extract_inline_computation(body).unwrap();
        assert!(c.fenced);
        assert_eq!(c.language.as_deref(), Some("sql"));
        assert_eq!(c.code, "SELECT 1");
    }

    #[test]
    fn an_indented_block_is_read_but_not_attestable() {
        let doc = Document::parse(
            "---\ntype: Attested Computation\nruntime: bigquery\nattester:\n  resource: /a.py\n---\n\n\
             # Computation\n\n    SELECT 1\n",
        )
        .unwrap();
        let c = doc.attested_computation().unwrap();
        let inline = match &c.computation {
            ComputationSource::Inline(inline) => inline,
            other => panic!("expected an inline computation, found {other:?}"),
        };
        assert!(!inline.fenced);
        assert_eq!(inline.code, "SELECT 1");
        assert_eq!(c.fenced_blocks, 0);
        assert!(!c.computation.is_locatable());
        assert!(!c.is_attestable());
    }

    #[test]
    fn attestable_needs_runtime_attester_and_computation() {
        let attestable = |frontmatter: &str, body: &str| {
            Document::parse(&format!(
                "---\ntype: Attested Computation\n{frontmatter}---\n\n{body}"
            ))
            .unwrap()
            .attested_computation()
            .unwrap()
            .is_attestable()
        };
        let fence = "# Computation\n\n```sql\nSELECT 1\n```\n";
        assert!(attestable(
            "runtime: bigquery\nattester:\n  resource: /a.py\n",
            fence
        ));
        assert!(!attestable("attester:\n  resource: /a.py\n", fence));
        assert!(!attestable("runtime: bigquery\n", fence));
        assert!(!attestable("runtime: bigquery\nattester: {}\n", fence));
        assert!(!attestable(
            "runtime: bigquery\nattester:\n  resource: /a.py\n",
            "# Definition\n"
        ));
        // The file form is located by its path.
        assert!(attestable(
            "runtime: bigquery\ncomputation: /lib/revenue.sql\nattester:\n  resource: /a.py\n",
            "# Definition\n"
        ));
    }

    #[test]
    fn file_form_replaces_the_body_block() {
        let doc = Document::parse(
            "---\ntype: Attested Computation\nruntime: bigquery\n\
             computation: /references/computations/lib/revenue.sql\n---\n\n# Definition\n\nProse.\n",
        )
        .unwrap();
        let c = AttestedComputation::from_parts(&doc.frontmatter, &doc.body);
        assert_eq!(
            c.computation.path(),
            Some("/references/computations/lib/revenue.sql")
        );
        assert!(!c.has_redundant_inline);
        assert_eq!(
            c.path_fields(),
            vec![("computation", "/references/computations/lib/revenue.sql")]
        );
    }

    #[test]
    fn both_forms_present_is_flagged() {
        let doc = Document::parse(
            "---\ntype: Attested Computation\ncomputation: x.sql\n---\n\n# Computation\n\n```sql\nSELECT 1\n```\n",
        )
        .unwrap();
        let c = AttestedComputation::from_parts(&doc.frontmatter, &doc.body);
        assert!(c.has_redundant_inline);
        assert_eq!(c.fenced_blocks, 1);
        assert_eq!(c.computation.path(), Some("x.sql"));
    }

    #[test]
    fn missing_computation_is_representable() {
        let doc =
            Document::parse("---\ntype: Attested Computation\n---\n\n# Definition\n").unwrap();
        let c = AttestedComputation::from_parts(&doc.frontmatter, &doc.body);
        assert!(c.computation.is_missing());
        assert!(c.runtime.is_none());
        assert!(!c.is_attestable());
    }

    #[test]
    fn section_ends_at_the_next_same_level_heading() {
        let body =
            "# Computation\n\n## Detail\n\n```\nSELECT 1\n```\n\n# Notes\n\n```\nSELECT 2\n```\n";
        let c = extract_inline_computation(body).unwrap();
        assert_eq!(c.code, "SELECT 1");
        assert_eq!(c.fenced_blocks, 1);
    }

    #[test]
    fn the_heading_is_found_at_any_level_and_case() {
        let body = "# Revenue\n\n### computation\n\n```sql\nSELECT 1\n```\n\n#### Detail\n\n```sql\nSELECT 2\n```\n\n## Notes\n\n```sql\nSELECT 3\n```\n";
        let c = extract_inline_computation(body).unwrap();
        assert_eq!(c.code, "SELECT 1");
        // The deeper `####` heading stays inside the section; `##` ends it.
        assert_eq!(c.fenced_blocks, 2);
    }

    #[test]
    fn hash_comments_inside_a_fence_are_not_headings() {
        // A `#` comment in the computation must neither end the section nor
        // be taken for a `Computation` heading itself.
        let body = "# Notes\n\n```python\n# Computation\nx = 0\n```\n\n# Computation\n\n```python\n# compute the rate\ndef rate(miles):\n    return miles * 0.67\n```\n";
        let c = extract_inline_computation(body).unwrap();
        assert_eq!(c.language.as_deref(), Some("python"));
        assert!(c.code.starts_with("# compute the rate\n"));
        assert!(c.code.ends_with("return miles * 0.67"));
        assert_eq!(c.fenced_blocks, 1);
    }

    #[test]
    fn a_longer_fence_can_hold_a_shorter_one() {
        let body = "# Computation\n\n````markdown\n```sql\nSELECT 1\n```\n````\n";
        let c = extract_inline_computation(body).unwrap();
        assert_eq!(c.code, "```sql\nSELECT 1\n```");
        assert_eq!(c.fenced_blocks, 1);
    }

    #[test]
    fn dbt_template_syntax_survives() {
        let body = "# Computation\n\n```sql\nSELECT gross_profit\nFROM {{ ref('fct_income_statement') }}\nWHERE fiscal_year = {{ var('year') }}\n```\n";
        let c = extract_inline_computation(body).unwrap();
        assert!(c.code.contains("{{ ref('fct_income_statement') }}"));
        assert_eq!(c.code.lines().count(), 3);
    }

    #[test]
    fn entries_without_required_keys_are_ignored() {
        let doc = Document::parse(
            "---\ntype: Attested Computation\nruntime: python\nparameters:\n  - { name: miles, type: number }\n  - { name: orphan }\n  - { type: string }\nexecutor:\n  resource: /run.md\n  receipt: job_id\n---\n",
        )
        .unwrap();
        let c = doc.attested_computation().unwrap();
        assert_eq!(c.parameters.len(), 1);
        assert_eq!(c.parameters[0].name.as_deref(), Some("miles"));
        // `required` defaults to false.
        assert!(!c.parameters[0].is_required());
        // One receipt field is read as a list with one item.
        assert_eq!(c.executor.unwrap().receipt, vec!["job_id"]);
    }
}
