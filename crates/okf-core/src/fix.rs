//! Automated remediation and migrations for OKF concepts, bundles, and logs.
//!
//! Continuous authoring by agents and humans naturally introduces hygiene issues
//! or leaves documents in older specification versions (such as v0.1 `timestamp`
//! and body `# Citations`). This module provides deterministic, safe, automated
//! remediation and migration transformations:
//!
//! - **Frontmatter key ordering** (`L18`): Normalizes frontmatter keys to the
//!   canonical [`PREFERRED_KEY_ORDER`].
//! - **v0.1 `timestamp` migration** (`L5`): Migrates legacy `timestamp` into a
//!   structured `generated: { by, at }` block (or removes redundant timestamp).
//! - **v0.1 `# Citations` migration** (`L6`): Converts legacy body citations to
//!   frontmatter `sources` entries and turns inline references into footnotes.
//! - **Missing title** (`L1`): Derives and inserts human-readable `title` from filename stem.
//! - **Missing generated** (`L3`): Inserts `generated: { by, at }` attribution.
//! - **Missing top heading** (`L8`): Prepends `# <title>` heading to document body.
//! - **Computation syntax tagging** (`L26`): Adds language syntax tag to `# Computation` code blocks.
//! - **Duplicate log dates** (`L27`): Consolidates duplicate `## YYYY-MM-DD` headings in `log.md`.
//! - **Index sync** (`L16`): Re-indexes all bundle directories.

use crate::computation::ATTESTED_COMPUTATION_TYPE;
use crate::document::Document;
use crate::frontmatter::{Frontmatter, PREFERRED_KEY_ORDER};
use crate::index::regenerate_indexes;
use crate::links::Citation;
use crate::log::{Log, LogDay};
use crate::scaffold::{current_iso_timestamp, default_author, title_from_name};
use crate::yaml::{Mapping, Value};
use std::collections::HashSet;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// What kind of remediation or migration was applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemediationKind {
    /// Reordered frontmatter keys to canonical order (L18).
    KeyOrder,
    /// Migrated legacy v0.1 `timestamp` to `generated` or removed redundant `timestamp` (L5).
    MigratedTimestamp,
    /// Migrated legacy v0.1 `# Citations` body section to frontmatter `sources` (L6).
    MigratedCitations,
    /// Added missing `title` inferred from filename (L1).
    AddedTitle(String),
    /// Added missing `generated` block (L3).
    AddedGenerated,
    /// Added missing top-level `# <title>` heading to document body (L8).
    AddedTopHeading(String),
    /// Tagged unlabeled `# Computation` code block with runtime language (L26).
    AddedComputationLanguage(String),
    /// Consolidated duplicate date headings in `log.md` (L27).
    ConsolidatedLogDates(String),
    /// Stripped trailing whitespace and normalized excess blank lines (L28).
    CleanedWhitespace,
}

/// A single remediation action applied to a document or file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Remediation {
    /// The rule or action kind.
    pub kind: RemediationKind,
    /// A human-readable description of the fix applied.
    pub description: String,
}

/// Options controlling automated remediation and migration.
#[allow(clippy::struct_excessive_bools)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FixOptions {
    /// Default author attribution string (e.g. "human:alice").
    pub author: Option<String>,
    /// Whether to insert missing titles (L1).
    pub add_missing_title: bool,
    /// Whether to insert missing `generated` blocks (L3).
    pub add_missing_generated: bool,
    /// Whether to insert missing top-level `# Heading` (L8).
    pub add_missing_top_heading: bool,
    /// Whether to migrate legacy v0.1 timestamp (L5).
    pub migrate_legacy_timestamp: bool,
    /// Whether to migrate legacy v0.1 Citations (L6).
    pub migrate_legacy_citations: bool,
    /// Whether to reorder frontmatter keys canonically (L18).
    pub reorder_keys: bool,
    /// Whether to tag unlabeled computation blocks (L26).
    pub tag_computation_blocks: bool,
    /// Whether to consolidate duplicate date headings in log.md (L27).
    pub fix_log_duplicates: bool,
    /// Whether to strip trailing whitespace and excess blank lines (L28).
    pub clean_whitespace: bool,
    /// Whether to regenerate index files (L16).
    pub regenerate_indexes: bool,
}

impl FixOptions {
    /// Creates options that only fix strict spec conformance and migration issues (for `okf validate --fix`).
    #[must_use]
    pub const fn validation_only(author: Option<String>) -> Self {
        Self {
            author,
            add_missing_title: true,
            add_missing_generated: true,
            add_missing_top_heading: false,
            migrate_legacy_timestamp: true,
            migrate_legacy_citations: true,
            reorder_keys: false,
            tag_computation_blocks: false,
            fix_log_duplicates: false,
            clean_whitespace: false,
            regenerate_indexes: false,
        }
    }
}

impl Default for FixOptions {
    fn default() -> Self {
        Self {
            author: None,
            add_missing_title: true,
            add_missing_generated: true,
            add_missing_top_heading: true,
            migrate_legacy_timestamp: true,
            migrate_legacy_citations: true,
            reorder_keys: true,
            tag_computation_blocks: true,
            fix_log_duplicates: true,
            clean_whitespace: true,
            regenerate_indexes: true,
        }
    }
}

/// Remediates and migrates a single [`Document`].
///
/// Returns the updated document and the list of applied remediations.
#[allow(clippy::too_many_lines)]
#[must_use]
pub fn remediate_document(
    doc: &Document,
    filename_stem: Option<&str>,
    options: &FixOptions,
) -> (Document, Vec<Remediation>) {
    let mut new_doc = doc.clone();
    let mut remediations = Vec::new();

    // 1. Missing title (L1)
    if options.add_missing_title {
        let has_title = new_doc
            .frontmatter
            .title()
            .is_some_and(|t| !t.trim().is_empty());
        if !has_title && let Some(stem) = filename_stem {
            let title = title_from_name(stem);
            new_doc
                .frontmatter
                .set("title", Value::String(title.clone()));
            remediations.push(Remediation {
                kind: RemediationKind::AddedTitle(title.clone()),
                description: format!("added missing title `{title}`"),
            });
        }
    }

    // 2. Legacy timestamp (L5)
    if options.migrate_legacy_timestamp && new_doc.frontmatter.get("timestamp").is_some() {
        let raw_ts = new_doc
            .frontmatter
            .get("timestamp")
            .and_then(Value::as_display_string);
        new_doc.frontmatter.remove("timestamp");

        if new_doc.frontmatter.get("generated").is_none() {
            if let Some(ts) = raw_ts {
                let author = options.author.clone().unwrap_or_else(default_author);
                let mut gen_map = Mapping::new();
                gen_map.insert("by", Value::String(author));
                gen_map.insert("at", Value::String(ts.clone()));
                new_doc
                    .frontmatter
                    .set("generated", Value::Mapping(gen_map));
                remediations.push(Remediation {
                    kind: RemediationKind::MigratedTimestamp,
                    description: format!("migrated legacy timestamp `{ts}` to generated block"),
                });
            }
        } else {
            remediations.push(Remediation {
                kind: RemediationKind::MigratedTimestamp,
                description: "removed redundant legacy timestamp".to_string(),
            });
        }
    }

    // 3. Missing generated (L3)
    if options.add_missing_generated
        && new_doc.frontmatter.get("generated").is_none()
        && new_doc.frontmatter.get("timestamp").is_none()
    {
        let author = options.author.clone().unwrap_or_else(default_author);
        let at = current_iso_timestamp();
        let mut gen_map = Mapping::new();
        gen_map.insert("by", Value::String(author));
        gen_map.insert("at", Value::String(at));
        new_doc
            .frontmatter
            .set("generated", Value::Mapping(gen_map));
        remediations.push(Remediation {
            kind: RemediationKind::AddedGenerated,
            description: "added missing generated metadata block".to_string(),
        });
    }

    // 4. Legacy citations (L6)
    if options.migrate_legacy_citations {
        let citations = new_doc.citations();
        if !citations.is_empty() {
            let mut existing_ids: HashSet<String> = HashSet::new();
            let mut sources_vec: Vec<Value> = Vec::new();

            if let Some(Value::Sequence(seq)) = new_doc.frontmatter.get("sources") {
                for item in seq {
                    if let Some(map) = item.as_mapping() {
                        if let Some(id) = map.get("id").and_then(Value::as_str) {
                            existing_ids.insert(id.to_string());
                        }
                        sources_vec.push(item.clone());
                    }
                }
            } else if let Some(Value::Mapping(map)) = new_doc.frontmatter.get("sources") {
                if let Some(id) = map.get("id").and_then(Value::as_str) {
                    existing_ids.insert(id.to_string());
                }
                sources_vec.push(Value::Mapping(map.clone()));
            }

            for cit in &citations {
                let id_str = cit.number.to_string();
                if existing_ids.insert(id_str.clone()) {
                    let mut s_map = Mapping::new();
                    s_map.insert("id", Value::String(id_str));
                    let resource_val = cit.target.as_ref().unwrap_or(&cit.raw);
                    s_map.insert("resource", Value::String(resource_val.clone()));
                    if let Some(title) = &cit.text {
                        s_map.insert("title", Value::String(title.clone()));
                    }
                    sources_vec.push(Value::Mapping(s_map));
                }
            }

            new_doc
                .frontmatter
                .set("sources", Value::Sequence(sources_vec));
            new_doc.body = migrate_body_citations(&new_doc.body, &citations);

            remediations.push(Remediation {
                kind: RemediationKind::MigratedCitations,
                description: format!(
                    "migrated {} legacy citation(s) to frontmatter sources",
                    citations.len()
                ),
            });
        }
    }

    // 5. Missing top heading (L8)
    if options.add_missing_top_heading {
        let trimmed = new_doc.body.trim();
        let has_top_heading = new_doc
            .body
            .lines()
            .any(|l| l.trim_start().starts_with("# "));
        if !trimmed.is_empty() && !has_top_heading {
            let title = new_doc.frontmatter.title().map_or_else(
                || filename_stem.map_or_else(|| "Concept".to_string(), title_from_name),
                |t| t.to_string(),
            );
            new_doc.body = format!("# {title}\n\n{}", new_doc.body.trim_start());
            remediations.push(Remediation {
                kind: RemediationKind::AddedTopHeading(title.clone()),
                description: format!("added missing top-level heading `# {title}`"),
            });
        }
    }

    // 6. Unlabeled computation block (L26)
    if options.tag_computation_blocks
        && (new_doc.frontmatter.is_attested_computation()
            || new_doc.frontmatter.type_().as_deref() == Some(ATTESTED_COMPUTATION_TYPE))
        && let Some(runtime) = new_doc.frontmatter.runtime()
    {
        let runtime_str = runtime.trim();
        if !runtime_str.is_empty() {
            let (tagged_body, changed) = tag_computation_block(&new_doc.body, runtime_str);
            if changed {
                new_doc.body = tagged_body;
                remediations.push(Remediation {
                    kind: RemediationKind::AddedComputationLanguage(runtime_str.to_string()),
                    description: format!(
                        "tagged computation code block with runtime `{runtime_str}`"
                    ),
                });
            }
        }
    }

    // 7. Canonical key order (L18)
    if options.reorder_keys && is_key_order_remediation_needed(&new_doc.frontmatter) {
        new_doc.frontmatter.reorder_preferred();
        remediations.push(Remediation {
            kind: RemediationKind::KeyOrder,
            description: "reordered frontmatter keys to canonical order".to_string(),
        });
    }

    // 8. Trailing whitespace and excess blank lines (L28)
    if options.clean_whitespace {
        let (cleaned_body, whitespace_changed) = clean_body_whitespace(&new_doc.body);
        if whitespace_changed {
            new_doc.body = cleaned_body;
            remediations.push(Remediation {
                kind: RemediationKind::CleanedWhitespace,
                description: "stripped trailing whitespace and normalized excess blank lines"
                    .to_string(),
            });
        }
    }

    (new_doc, remediations)
}

/// Checks if frontmatter keys deviate from the canonical preferred order.
fn is_key_order_remediation_needed(fm: &Frontmatter) -> bool {
    let keys: Vec<&str> = fm.as_mapping().keys().collect();
    if keys.len() < 2 {
        return false;
    }
    let mut last_rank = None;
    for key in keys {
        if let Some(rank) = PREFERRED_KEY_ORDER.iter().position(|&k| k == key) {
            if let Some(prev) = last_rank
                && rank < prev
            {
                return true;
            }
            last_rank = Some(rank);
        }
    }
    false
}

/// Removes the legacy `# Citations` section and converts citation references `[n]` to `[^n]`.
fn migrate_body_citations(body: &str, citations: &[Citation]) -> String {
    let lines: Vec<&str> = body.lines().collect();
    let mut out_lines: Vec<String> = Vec::new();
    let mut in_citations = false;
    let mut citations_heading_level = 1;

    let cit_numbers: Vec<u32> = citations.iter().map(|c| c.number).collect();

    for line in lines {
        let trimmed = line.trim();
        if let Some(heading) = trimmed.strip_prefix('#') {
            let count = trimmed.chars().take_while(|&c| c == '#').count();
            let title = heading.trim_start_matches('#').trim();
            if in_citations {
                if count <= citations_heading_level {
                    in_citations = false;
                } else {
                    continue;
                }
            }
            if title.eq_ignore_ascii_case("citations") {
                in_citations = true;
                citations_heading_level = count;
                continue;
            }
        }
        if in_citations {
            continue;
        }

        out_lines.push(replace_citation_refs_in_line(line, &cit_numbers));
    }

    // Trim trailing empty lines, keeping a clean single trailing newline
    while out_lines.last().is_some_and(|l| l.trim().is_empty()) {
        out_lines.pop();
    }
    let mut result = out_lines.join("\n");
    if !result.is_empty() {
        result.push('\n');
    }
    result
}

/// Replaces `[n]` citation markers with footnote markers `[^n]` in prose lines.
fn replace_citation_refs_in_line(line: &str, cit_numbers: &[u32]) -> String {
    let mut out = String::with_capacity(line.len() + 8);
    let bytes = line.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'[' {
            // Check if preceded by `^` or `!`
            let prev_char = if i > 0 { Some(bytes[i - 1]) } else { None };
            if prev_char == Some(b'^') || prev_char == Some(b'!') {
                out.push(bytes[i] as char);
                i += 1;
                continue;
            }

            if let Some(close_rel) = line[i + 1..].find(']') {
                let close_idx = i + 1 + close_rel;
                let inside = &line[i + 1..close_idx].trim();
                let is_number = inside.parse::<u32>().ok();

                if let Some(num) = is_number
                    && cit_numbers.contains(&num)
                {
                    let next_char = line.as_bytes().get(close_idx + 1).copied();
                    if next_char != Some(b'(') && next_char != Some(b'[') {
                        let _ = write!(out, "[^{num}]");
                        i = close_idx + 1;
                        continue;
                    }
                }
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// Tags untagged code blocks under `# Computation` with the given runtime.
fn tag_computation_block(body: &str, runtime: &str) -> (String, bool) {
    let mut out_lines = Vec::new();
    let mut in_computation_section = false;
    let mut changed = false;
    let mut in_code_block = false;

    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("# ") {
            in_computation_section = trimmed == "# Computation";
            out_lines.push(line.to_string());
            continue;
        }

        if in_computation_section {
            if trimmed == "```" {
                if !in_code_block {
                    // Opening untagged fence
                    let leading_spaces = line.len() - line.trim_start().len();
                    let indent = " ".repeat(leading_spaces);
                    out_lines.push(format!("{indent}```{runtime}"));
                    changed = true;
                    in_code_block = true;
                    continue;
                }
                in_code_block = false;
            } else if trimmed == "~~~" {
                if !in_code_block {
                    let leading_spaces = line.len() - line.trim_start().len();
                    let indent = " ".repeat(leading_spaces);
                    out_lines.push(format!("{indent}~~~{runtime}"));
                    changed = true;
                    in_code_block = true;
                    continue;
                }
                in_code_block = false;
            }
        }
        out_lines.push(line.to_string());
    }

    (out_lines.join("\n"), changed)
}

/// Strips trailing whitespace on each line and collapses excess consecutive blank lines in markdown body.
fn clean_body_whitespace(body: &str) -> (String, bool) {
    let mut out_lines: Vec<String> = Vec::new();
    let mut consecutive_empty = 0;

    for line in body.lines() {
        let trimmed_cr = line.trim_end_matches('\r');
        let trimmed_end = trimmed_cr.trim_end();

        if trimmed_end.is_empty() {
            consecutive_empty += 1;
            if consecutive_empty > 1 {
                continue;
            }
            out_lines.push(String::new());
        } else {
            consecutive_empty = 0;
            out_lines.push(trimmed_end.to_string());
        }
    }

    while out_lines.last().is_some_and(String::is_empty) {
        out_lines.pop();
    }

    let mut result = out_lines.join("\n");
    if !result.is_empty() {
        result.push('\n');
    }

    let normalized_input = if body.is_empty() {
        String::new()
    } else if body.ends_with('\n') {
        body.to_string()
    } else {
        format!("{body}\n")
    };

    let changed = result != normalized_input;
    (result, changed)
}

/// Remediates a parsed `log.md` file (consolidating duplicate date headings).
#[must_use]
pub fn remediate_log(text: &str, options: &FixOptions) -> (String, Vec<Remediation>) {
    let log = Log::parse(text);
    let mut remediations = Vec::new();

    if !options.fix_log_duplicates {
        return (text.to_string(), remediations);
    }

    let mut consolidated_days: Vec<LogDay> = Vec::new();
    let mut seen_dates: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    for day in log.days {
        if let Some(&existing_idx) = seen_dates.get(&day.date) {
            consolidated_days[existing_idx].entries.extend(day.entries);
            remediations.push(Remediation {
                kind: RemediationKind::ConsolidatedLogDates(day.date.clone()),
                description: format!("consolidated duplicate date headings for `## {}`", day.date),
            });
        } else {
            seen_dates.insert(day.date.clone(), consolidated_days.len());
            consolidated_days.push(day);
        }
    }

    if remediations.is_empty() {
        (text.to_string(), remediations)
    } else {
        let new_log = Log {
            frontmatter: log.frontmatter,
            title: log.title,
            days: consolidated_days,
        };
        (new_log.to_markdown(), remediations)
    }
}

/// Remediation report for a single file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileFixReport {
    /// File path.
    pub path: PathBuf,
    /// Remediations applied.
    pub remediations: Vec<Remediation>,
    /// Content before remediation.
    pub original_content: String,
    /// Content after remediation.
    pub remediated_content: String,
    /// Whether any content changed.
    pub changed: bool,
}

/// Remediation report for a whole bundle.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BundleFixReport {
    /// Bundle root directory.
    pub bundle_root: PathBuf,
    /// Reports for individual files.
    pub files: Vec<FileFixReport>,
    /// Index files that are out of sync or would be regenerated.
    pub index_files_to_regenerate: Vec<PathBuf>,
    /// Options used.
    pub options: FixOptions,
}

impl BundleFixReport {
    /// Writes all modified files to disk and regenerates index files if needed.
    ///
    /// Returns a tuple of `(files_written_count, regenerated_indexes)`.
    ///
    /// # Errors
    ///
    /// Returns [`io::Error`] if writing any file fails.
    pub fn apply(&self) -> io::Result<(usize, Vec<PathBuf>)> {
        let mut count = 0;
        for file in &self.files {
            if file.changed {
                fs::write(&file.path, &file.remediated_content)?;
                count += 1;
            }
        }
        let regenerated = if self.options.regenerate_indexes
            && (count > 0 || !self.index_files_to_regenerate.is_empty())
        {
            regenerate_indexes(&self.bundle_root)?
        } else {
            Vec::new()
        };
        Ok((count, regenerated))
    }

    /// Total number of remediation actions applied across all files.
    #[must_use]
    pub fn total_remediations(&self) -> usize {
        self.files.iter().map(|f| f.remediations.len()).sum()
    }

    /// All files that were or would be modified.
    pub fn changed_files(&self) -> impl Iterator<Item = &FileFixReport> {
        self.files.iter().filter(|f| f.changed)
    }

    /// `true` if no files need fixes.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.files.iter().all(|f| !f.changed) && self.index_files_to_regenerate.is_empty()
    }
}

/// Remediates a single file on disk.
///
/// # Errors
///
/// Returns [`io::Error`] on unreadable file or parse failures.
pub fn remediate_file(path: impl AsRef<Path>, options: &FixOptions) -> io::Result<FileFixReport> {
    let path = path.as_ref().to_path_buf();
    let original = fs::read_to_string(&path)?;
    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();

    if filename == "log.md" {
        let (remediated_content, remediations) = remediate_log(&original, options);
        let changed = remediated_content != original;
        return Ok(FileFixReport {
            path,
            remediations,
            original_content: original,
            remediated_content,
            changed,
        });
    }

    if filename == "index.md" {
        return Ok(FileFixReport {
            path,
            remediations: Vec::new(),
            original_content: original.clone(),
            remediated_content: original,
            changed: false,
        });
    }

    let doc = Document::parse(&original).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("could not parse {}: {e}", path.display()),
        )
    })?;
    let stem = path.file_stem().and_then(|s| s.to_str());
    let (remediated_doc, remediations) = remediate_document(&doc, stem, options);
    let remediated_content = remediated_doc.serialize();
    let changed = remediated_content != original || !remediations.is_empty();

    Ok(FileFixReport {
        path,
        remediations,
        original_content: original,
        remediated_content,
        changed,
    })
}

/// Remediates an entire bundle directory tree.
///
/// # Errors
///
/// Returns [`io::Error`] on filesystem read errors.
pub fn remediate_bundle(
    bundle_root: impl AsRef<Path>,
    options: &FixOptions,
) -> io::Result<BundleFixReport> {
    let bundle_root = bundle_root.as_ref().to_path_buf();
    let mut files = Vec::new();
    let mut md_paths = Vec::new();

    collect_md_files(&bundle_root, &mut md_paths)?;
    md_paths.sort();

    for path in &md_paths {
        let filename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if filename == "index.md" {
            continue;
        }
        if let Ok(report) = remediate_file(path, options) {
            files.push(report);
        }
    }

    let mut index_files_to_regenerate = Vec::new();
    if options.regenerate_indexes {
        let any_file_changed = files.iter().any(|f| f.changed);
        if any_file_changed {
            let mut all_dirs = std::collections::BTreeSet::new();
            for md in &md_paths {
                if let Some(parent) = md.parent() {
                    all_dirs.insert(parent.to_path_buf());
                }
            }
            index_files_to_regenerate = all_dirs.into_iter().map(|d| d.join("index.md")).collect();
        } else if let Ok(bundle) = crate::bundle::Bundle::load(&bundle_root) {
            let mut all_dirs = std::collections::BTreeSet::new();
            for c in bundle.concepts() {
                if let Some(parent) = c.path.parent() {
                    all_dirs.insert(parent.to_path_buf());
                }
            }
            for dir in all_dirs {
                let idx = dir.join("index.md");
                if !idx.exists() {
                    index_files_to_regenerate.push(idx);
                }
            }
        }
    }

    Ok(BundleFixReport {
        bundle_root,
        files,
        index_files_to_regenerate,
        options: options.clone(),
    })
}

fn collect_md_files(dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.starts_with('.') && name != "target" && name != "node_modules" {
                collect_md_files(&path, out)?;
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("md") {
            out.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remediates_missing_title_and_generated_and_heading() {
        let input = "---\ntype: Concept\n---\nBody text.\n";
        let doc = Document::parse(input).unwrap();
        let opts = FixOptions {
            author: Some("human:alice".to_string()),
            ..Default::default()
        };

        let (fixed_doc, remediations) = remediate_document(&doc, Some("revenue_stream"), &opts);

        assert_eq!(
            fixed_doc.frontmatter.title().as_deref(),
            Some("Revenue Stream")
        );
        assert_eq!(
            fixed_doc
                .frontmatter
                .generated()
                .unwrap()
                .by
                .unwrap()
                .as_str(),
            "human:alice"
        );
        assert!(fixed_doc.body.starts_with("# Revenue Stream\n\n"));
        assert_eq!(remediations.len(), 3);
    }

    #[test]
    fn migrates_legacy_timestamp_and_citations() {
        let input = "---\n\
                     type: Concept\n\
                     title: Revenue\n\
                     timestamp: 2026-05-01T00:00:00Z\n\
                     ---\n\n\
                     # Definition\n\
                     Defined in [1] and [2].\n\n\
                     # Citations\n\
                     [1] [GAAP Standards](https://example.com/gaap)\n\
                     [2] https://example.com/sec\n";
        let doc = Document::parse(input).unwrap();
        let opts = FixOptions {
            author: Some("human:bob".to_string()),
            ..Default::default()
        };

        let (fixed_doc, remediations) = remediate_document(&doc, Some("revenue"), &opts);

        assert!(fixed_doc.frontmatter.get("timestamp").is_none());
        assert_eq!(
            fixed_doc.frontmatter.generated().unwrap().at.unwrap().raw,
            "2026-05-01T00:00:00Z"
        );
        assert_eq!(
            fixed_doc
                .frontmatter
                .generated()
                .unwrap()
                .by
                .unwrap()
                .as_str(),
            "human:bob"
        );

        let sources = fixed_doc.frontmatter.sources();
        assert_eq!(sources.len(), 2);
        assert_eq!(sources[0].id.as_deref(), Some("1"));
        assert_eq!(
            sources[0].resource.as_deref(),
            Some("https://example.com/gaap")
        );
        assert_eq!(sources[0].title.as_deref(), Some("GAAP Standards"));

        assert!(!fixed_doc.body.contains("# Citations"));
        assert!(fixed_doc.body.contains("Defined in [^1] and [^2]."));
        assert!(
            remediations
                .iter()
                .any(|r| matches!(r.kind, RemediationKind::MigratedTimestamp))
        );
        assert!(
            remediations
                .iter()
                .any(|r| matches!(r.kind, RemediationKind::MigratedCitations))
        );
    }

    #[test]
    fn tags_computation_code_block() {
        let input = "---\n\
                     type: Attested Computation\n\
                     runtime: python\n\
                     ---\n\n\
                     # Computation\n\n\
                     ```\n\
                     def compute():\n\
                         return 42\n\
                     ```\n";
        let doc = Document::parse(input).unwrap();
        let opts = FixOptions::default();

        let (fixed_doc, remediations) = remediate_document(&doc, Some("calc"), &opts);
        assert!(fixed_doc.body.contains("```python\n"));
        assert!(
            remediations
                .iter()
                .any(|r| matches!(r.kind, RemediationKind::AddedComputationLanguage(_)))
        );
    }

    #[test]
    fn consolidates_duplicate_log_dates() {
        let input = "# Update Log\n\n\
                     ## 2026-06-01\n\
                     * **Update**: Changed formula.\n\n\
                     ## 2026-06-01\n\
                     * **Creation**: Added concept.\n";
        let (fixed, remediations) = remediate_log(input, &FixOptions::default());
        assert_eq!(remediations.len(), 1);
        assert_eq!(fixed.matches("## 2026-06-01").count(), 1);
        assert!(fixed.contains("* **Update**: Changed formula."));
        assert!(fixed.contains("* **Creation**: Added concept."));
    }
}
