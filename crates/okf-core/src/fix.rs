//! Automated remediation and migrations for OKF concepts, bundles, and logs.
//!
//! Continuous authoring by agents and humans naturally introduces hygiene issues
//! or leaves documents in the form of an older specification version. This
//! module provides deterministic, safe, automated remediation and migration
//! transformations:
//!
//! - **Frontmatter key ordering** (`L18`): Normalizes frontmatter keys to the
//!   canonical [`PREFERRED_KEY_ORDER`].
//! - **v0.1 `timestamp` migration** (`L5`): Migrates legacy `timestamp` into a
//!   structured `generated: { by, at }` block (or removes redundant timestamp).
//! - **v0.1 `# Citations` migration** (`L6`): Converts legacy body citations to
//!   frontmatter `sources` entries and turns inline references into footnotes.
//! - **Root-relative path fields**: Adds the leading `/` to a path field that
//!   earlier versions wrote from the bundle root without one.
//! - **Scalar `tags`**: Rewrites a scalar `tags` value as a YAML list, splitting
//!   a comma-separated string into separate tags.
//! - **Timestamps**: Repairs a timestamp that is close to valid (a missing
//!   offset, a space separator) and expands a date-only `generated.at` or
//!   `verified[].at` to a date-time.
//! - **Indented computation**: Fences an indented `# Computation` block, which
//!   is the only form a consumer locates.
//! - **`log.md` frontmatter**: Removes it; a log has none.
//! - **`okf_version`**: Quotes an unquoted value (`L13`) and declares the
//!   version in a bundle-root `index.md` that has none.
//! - **Missing title** (`L1`): Derives and inserts human-readable `title` from filename stem.
//! - **Missing generated** (`L3`): Inserts `generated: { by, at }` attribution.
//! - **Missing top heading** (`L8`): Prepends `# <title>` heading to document body.
//! - **Computation syntax tagging** (`L26`): Adds language syntax tag to `# Computation` code blocks.
//! - **Duplicate log dates** (`L27`): Consolidates duplicate `## YYYY-MM-DD` headings in `log.md`.
//! - **Index sync** (`L16`): Re-indexes all bundle directories.
//!
//! A fix rewrites only what it changes. Frontmatter entries it does not touch
//! are written back exactly as they were, and a markdown file that is not a
//! concept (no frontmatter, or no `type`) is left alone entirely.

use crate::computation::ATTESTED_COMPUTATION_TYPE;
use crate::document::Document;
use crate::frontmatter::{Frontmatter, PREFERRED_KEY_ORDER};
use crate::index::regenerate_indexes;
use crate::links::Citation;
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
    /// Stripped trailing whitespace and normalized excess blank lines (L8).
    CleanedWhitespace,
    /// Normalized non-compliant date/datetime string to explicit ISO-8601 UTC format.
    NormalizedDate(String),
    /// Quoted unquoted `okf_version` in root `index.md` (L13).
    QuotedOkfVersion,
    /// Declared `okf_version` in a bundle-root `index.md` that had none.
    DeclaredOkfVersion,
    /// Added the leading `/` to a path field that was written from the bundle
    /// root without one. Carries the field name.
    RootedPath(String),
    /// Rewrote a scalar `tags` value as a YAML list.
    ListifiedTags,
    /// Fenced an indented `# Computation` code block.
    FencedComputation,
    /// Removed the frontmatter block from a `log.md`.
    StrippedLogFrontmatter,
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
    /// Whether to normalize date/datetime strings lacking explicit UTC offset.
    pub normalize_date_formats: bool,
    /// Whether to reorder frontmatter keys canonically (L18).
    pub reorder_keys: bool,
    /// Whether to tag unlabeled computation blocks (L26).
    pub tag_computation_blocks: bool,
    /// Whether to consolidate duplicate date headings in log.md (L27).
    pub fix_log_duplicates: bool,
    /// Whether to strip trailing whitespace and excess blank lines (L8).
    pub clean_whitespace: bool,
    /// Whether to regenerate index files (L16).
    pub regenerate_indexes: bool,
    /// Whether to quote unquoted `okf_version` in root index.md (L13).
    pub quote_okf_version: bool,
    /// Whether to declare `okf_version` in a bundle-root `index.md` that has
    /// none. Only done when no path field still depends on the root-relative
    /// reading of earlier versions, since declaring the current version turns
    /// that fallback off.
    pub declare_okf_version: bool,
    /// Whether to add the leading `/` to path fields that only resolve from
    /// the bundle root.
    pub root_legacy_paths: bool,
    /// Whether to rewrite a scalar `tags` value as a list, splitting it at
    /// commas.
    pub listify_tags: bool,
    /// Whether to fence an indented `# Computation` code block.
    pub fence_computation: bool,
    /// Whether to remove the frontmatter block from `log.md` files.
    pub strip_log_frontmatter: bool,
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
            normalize_date_formats: true,
            reorder_keys: false,
            tag_computation_blocks: false,
            fix_log_duplicates: false,
            clean_whitespace: false,
            regenerate_indexes: false,
            quote_okf_version: true,
            declare_okf_version: true,
            root_legacy_paths: true,
            listify_tags: true,
            fence_computation: true,
            strip_log_frontmatter: true,
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
            normalize_date_formats: true,
            reorder_keys: true,
            tag_computation_blocks: true,
            fix_log_duplicates: true,
            clean_whitespace: true,
            regenerate_indexes: true,
            quote_okf_version: true,
            declare_okf_version: true,
            root_legacy_paths: true,
            listify_tags: true,
            fence_computation: true,
            strip_log_frontmatter: true,
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
            .and_then(Value::as_display_string)
            .filter(|ts| !ts.trim().is_empty());
        new_doc.frontmatter.remove("timestamp");

        let generated = new_doc.frontmatter.get("generated").cloned();
        match (generated, raw_ts) {
            // No `generated` yet: the timestamp becomes its `at`.
            (None, Some(ts)) => {
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
            // A `generated` that records who but not when: the timestamp is
            // the one record of when, so it moves there instead of being
            // dropped.
            (Some(Value::Mapping(mut gen_map)), Some(ts))
                if gen_map.get("at").is_none_or(Value::is_blank) =>
            {
                gen_map.insert("at", Value::String(ts.clone()));
                new_doc
                    .frontmatter
                    .set("generated", Value::Mapping(gen_map));
                remediations.push(Remediation {
                    kind: RemediationKind::MigratedTimestamp,
                    description: format!("moved legacy timestamp `{ts}` to `generated.at`"),
                });
            }
            _ => remediations.push(Remediation {
                kind: RemediationKind::MigratedTimestamp,
                description: "removed redundant legacy timestamp".to_string(),
            }),
        }
    }

    // 3. Missing generated (L3)
    if options.add_missing_generated
        && new_doc.frontmatter.get("generated").is_none()
        && new_doc.frontmatter.get("timestamp").is_none()
    {
        let author = options.author.clone().unwrap_or_else(default_author);
        let mut gen_map = Mapping::new();
        gen_map.insert("by", Value::String(author));
        // `generated.at` is the time of the last meaningful change, and a
        // verification counts only if it is at or after it. Stamping "now"
        // on a concept that someone has verified would turn that review into
        // history without the text having changed, so `at` is left out there.
        let verified = !new_doc.frontmatter.verified().is_empty();
        if !verified {
            gen_map.insert("at", Value::String(current_iso_timestamp()));
        }
        new_doc
            .frontmatter
            .set("generated", Value::Mapping(gen_map));
        remediations.push(Remediation {
            kind: RemediationKind::AddedGenerated,
            description: if verified {
                "added missing generated metadata block (without `at`, so that existing \
                 verifications stay current)"
                    .to_string()
            } else {
                "added missing generated metadata block".to_string()
            },
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

    // A scalar `tags` value reads as one tag, commas and all. Write the list
    // the producer meant.
    if options.listify_tags
        && let Some(tags) = new_doc
            .frontmatter
            .get("tags")
            .filter(|value| !value.is_blank())
            .and_then(Value::as_display_string)
    {
        let list: Vec<Value> = tags
            .split(',')
            .map(str::trim)
            .filter(|tag| !tag.is_empty())
            .map(|tag| Value::String(tag.to_string()))
            .collect();
        let count = list.len();
        new_doc.frontmatter.set("tags", Value::Sequence(list));
        remediations.push(Remediation {
            kind: RemediationKind::ListifiedTags,
            description: format!("rewrote scalar `tags` as a list of {count} tag(s)"),
        });
    }

    // An indented block under `# Computation` is not a form a consumer
    // locates. Fence it.
    if options.fence_computation
        && new_doc.frontmatter.is_attested_computation()
        && new_doc.frontmatter.computation().is_none()
        && new_doc
            .inline_computation()
            .is_some_and(|inline| !inline.fenced)
        && let Some(fenced_body) = fence_indented_computation(
            &new_doc.body,
            new_doc.frontmatter.runtime().as_deref().map(str::trim),
        )
    {
        new_doc.body = fenced_body;
        remediations.push(Remediation {
            kind: RemediationKind::FencedComputation,
            description: "fenced the indented `# Computation` code block".to_string(),
        });
    }

    if options.normalize_date_formats {
        remediate_date_fields(&mut new_doc.frontmatter, &mut remediations);
    }

    if options.reorder_keys && is_key_order_remediation_needed(&new_doc.frontmatter) {
        new_doc.frontmatter.reorder_preferred();
        remediations.push(Remediation {
            kind: RemediationKind::KeyOrder,
            description: "reordered frontmatter keys to canonical order".to_string(),
        });
    }

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

/// Describes a timestamp repair. Filling in a missing offset is a guess, and
/// the description says so.
fn date_remediation(field_desc: &str, original: &str, repaired: &str) -> Remediation {
    let assumed_utc = crate::date::DateTime::parse(original)
        .is_some_and(|dt| dt.has_time && dt.offset_minutes.is_none());
    let note = if assumed_utc {
        " (no offset was given; UTC assumed)"
    } else {
        ""
    };
    Remediation {
        kind: RemediationKind::NormalizedDate(field_desc.to_string()),
        description: format!("normalized `{field_desc}` timestamp to `{repaired}`{note}"),
    }
}

/// Repairs the timestamp at `key`, leaving a value that is already valid
/// untouched. With `prefer_datetime`, a date-only value is expanded to the
/// equivalent date-time.
fn normalize_mapping_date_key(
    map: &mut Mapping,
    key: &str,
    field_desc: &str,
    prefer_datetime: bool,
    remediations: &mut Vec<Remediation>,
) -> bool {
    if let Some(val) = map.get(key).and_then(Value::as_display_string)
        && let Some(repaired) = crate::date::repair_timestamp(&val, prefer_datetime)
        && repaired != val
    {
        remediations.push(date_remediation(field_desc, &val, &repaired));
        map.insert(key, Value::String(repaired));
        return true;
    }
    false
}

/// Repairs every timestamp key of a concept.
///
/// `generated.at` and `verified[].at` are written as date-times, because
/// consumers compare the two. The other keys keep a date-only value as it is.
#[allow(clippy::too_many_lines)]
fn remediate_date_fields(fm: &mut Frontmatter, remediations: &mut Vec<Remediation>) {
    if let Some(Value::Mapping(mut gen_map)) = fm.get("generated").cloned()
        && normalize_mapping_date_key(&mut gen_map, "at", "generated.at", true, remediations)
    {
        fm.set("generated", Value::Mapping(gen_map));
    }

    if let Some(v_val) = fm.get("verified").cloned() {
        match v_val {
            Value::Sequence(mut seq) => {
                let mut changed = false;
                for (i, item) in seq.iter_mut().enumerate() {
                    if let Some(map) = item.as_mapping_mut() {
                        changed |= normalize_mapping_date_key(
                            map,
                            "at",
                            &format!("verified[{i}].at"),
                            true,
                            remediations,
                        );
                    }
                }
                if changed {
                    fm.set("verified", Value::Sequence(seq));
                }
            }
            Value::Mapping(mut map) => {
                if normalize_mapping_date_key(&mut map, "at", "verified.at", true, remediations) {
                    fm.set("verified", Value::Mapping(map));
                }
            }
            _ => {}
        }
    }

    if let Some(stale_val) = fm.get("stale_after").and_then(Value::as_display_string)
        && let Some(repaired) = crate::date::repair_timestamp(&stale_val, false)
        && repaired != stale_val
    {
        remediations.push(date_remediation("stale_after", &stale_val, &repaired));
        fm.set("stale_after", Value::String(repaired));
    }

    if let Some(Value::Mapping(mut uw_map)) = fm.get("usage_window").cloned() {
        let mut changed = false;
        for field in ["from", "to"] {
            changed |= normalize_mapping_date_key(
                &mut uw_map,
                field,
                &format!("usage_window.{field}"),
                false,
                remediations,
            );
        }
        if changed {
            fm.set("usage_window", Value::Mapping(uw_map));
        }
    }

    let normalize_source =
        |map: &mut Mapping, prefix: &str, remediations: &mut Vec<Remediation>| {
            let mut changed = normalize_mapping_date_key(
                map,
                "last_modified",
                &format!("{prefix}.last_modified"),
                false,
                remediations,
            );
            if let Some(Value::Mapping(mut uw_map)) = map.get("usage_window").cloned() {
                let mut uw_changed = false;
                for field in ["from", "to"] {
                    uw_changed |= normalize_mapping_date_key(
                        &mut uw_map,
                        field,
                        &format!("{prefix}.usage_window.{field}"),
                        false,
                        remediations,
                    );
                }
                if uw_changed {
                    map.insert("usage_window", Value::Mapping(uw_map));
                    changed = true;
                }
            }
            changed
        };

    if let Some(s_val) = fm.get("sources").cloned() {
        match s_val {
            Value::Sequence(mut seq) => {
                let mut changed = false;
                for (i, item) in seq.iter_mut().enumerate() {
                    if let Some(map) = item.as_mapping_mut() {
                        changed |= normalize_source(map, &format!("sources[{i}]"), remediations);
                    }
                }
                if changed {
                    fm.set("sources", Value::Sequence(seq));
                }
            }
            Value::Mapping(mut map) => {
                if normalize_source(&mut map, "sources", remediations) {
                    fm.set("sources", Value::Mapping(map));
                }
            }
            _ => {}
        }
    }
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
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len() + 8);
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == '['
            // Already a footnote (`[^1]`) or an image (`![1]`).
            && !(i > 0 && matches!(chars[i - 1], '^' | '!'))
            && let Some(close) = (i + 1..chars.len()).find(|&j| chars[j] == ']')
        {
            let inside: String = chars[i + 1..close].iter().collect();
            if let Ok(num) = inside.trim().parse::<u32>()
                && cit_numbers.contains(&num)
                // `[1](url)` and `[1][ref]` are links, not citation markers.
                && !matches!(chars.get(close + 1), Some('(' | '['))
            {
                let _ = write!(out, "[^{num}]");
                i = close + 1;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Rewrites the indented code block that stands in for the computation as a
/// fenced block, tagged with `language` when one is known. Returns `None` when
/// the `Computation` section has no such block.
///
/// The block is located by the same scanner that reads the computation, so a
/// `# Computation` line inside some other fenced block is never mistaken for
/// the heading.
fn fence_indented_computation(body: &str, language: Option<&str>) -> Option<String> {
    let (start, end) = crate::computation::indented_computation_lines(body)?;
    let lines: Vec<&str> = body.lines().collect();
    let fence = if lines[start..end].iter().any(|line| line.contains("```")) {
        "~~~"
    } else {
        "```"
    };
    let mut out: Vec<String> = lines[..start].iter().map(ToString::to_string).collect();
    out.push(format!("{fence}{}", language.unwrap_or_default()));
    for line in &lines[start..end] {
        out.push(
            line.strip_prefix("    ")
                .or_else(|| line.strip_prefix('\t'))
                .unwrap_or_else(|| line.trim_start())
                .to_string(),
        );
    }
    out.push(fence.to_string());
    out.extend(lines[end..].iter().map(ToString::to_string));
    let mut result = out.join("\n");
    if body.ends_with('\n') {
        result.push('\n');
    }
    Some(result)
}

/// Tags the computation's opening fence with the given runtime, when it has
/// no language tag of its own.
///
/// Only that one line changes. A fence that already names a language is left
/// alone: `runtime: bigquery` above a `sql` block is exactly how the
/// specification writes it.
fn tag_computation_block(body: &str, runtime: &str) -> (String, bool) {
    let Some(fence) = crate::computation::computation_fence(body).filter(|f| !f.has_info) else {
        return (body.to_string(), false);
    };
    let mut lines: Vec<String> = body.lines().map(ToString::to_string).collect();
    let line = &lines[fence.line];
    let indent = &line[..line.len() - line.trim_start().len()];
    lines[fence.line] = format!("{indent}{}{runtime}", fence.marker);
    let mut result = lines.join("\n");
    if body.ends_with('\n') {
        result.push('\n');
    }
    (result, true)
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
            if consecutive_empty > 2 {
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

/// Remediates a `log.md` file: removes a frontmatter block, which a log does
/// not have, and consolidates duplicate date headings.
///
/// The text is returned unchanged when neither applies.
#[must_use]
pub fn remediate_log(text: &str, options: &FixOptions) -> (String, Vec<Remediation>) {
    let mut out = text.to_string();
    let mut remediations = Vec::new();

    // Both fixes edit the text in place. The log is never re-rendered from
    // its parsed form, which keeps only the entries it recognizes and would
    // drop wrapped lines, nested bullets, and prose.
    if options.strip_log_frontmatter
        && Document::has_frontmatter_block(&out)
        && Document::parse(&out).is_ok()
    {
        out = strip_frontmatter_block(&out);
        remediations.push(Remediation {
            kind: RemediationKind::StrippedLogFrontmatter,
            description: "removed the frontmatter block; a log.md has none".to_string(),
        });
    }

    if options.fix_log_duplicates
        && let Some((merged, dates)) = crate::log::consolidate_duplicate_dates(&out)
    {
        out = merged;
        for date in dates {
            remediations.push(Remediation {
                description: format!("consolidated duplicate date headings for `## {date}`"),
                kind: RemediationKind::ConsolidatedLogDates(date),
            });
        }
    }

    (out, remediations)
}

/// Returns `text` without its leading frontmatter block and the blank lines
/// that follow it.
fn strip_frontmatter_block(text: &str) -> String {
    let body = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = body.lines();
    if lines
        .next()
        .is_none_or(|line| !crate::document::is_delimiter(line))
    {
        return text.to_string();
    }
    let mut closed = false;
    let mut rest: Vec<&str> = Vec::new();
    for line in lines {
        if closed {
            rest.push(line);
        } else if crate::document::is_delimiter(line) {
            closed = true;
        }
    }
    if !closed {
        return text.to_string();
    }
    let first_content = rest
        .iter()
        .position(|line| !line.trim().is_empty())
        .unwrap_or(rest.len());
    let mut out = rest[first_content..].join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out
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
        let mut remediations = Vec::new();
        let (remediated_content, changed) = if options.quote_okf_version
            && let Ok(mut doc) = Document::parse(&original)
            && let Some(val) = doc.frontmatter.get("okf_version")
            && !matches!(val, Value::String(_))
            && let Some(ver_str) = val.as_display_string()
        {
            doc.frontmatter.set("okf_version", Value::String(ver_str));
            remediations.push(Remediation {
                kind: RemediationKind::QuotedOkfVersion,
                description: "Quoted unquoted `okf_version` in root index.md (L13)".to_string(),
            });
            (doc.serialize(), true)
        } else {
            (original.clone(), false)
        };

        return Ok(FileFixReport {
            path,
            remediations,
            original_content: original,
            remediated_content,
            changed,
        });
    }

    let doc = Document::parse(&original).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("could not parse {}: {e}", path.display()),
        )
    })?;
    // A markdown file without frontmatter, or without `type`, is not a
    // concept. It may be a README or a skill file: leave it as it is.
    if !doc.is_concept() {
        return Ok(FileFixReport {
            path,
            remediations: Vec::new(),
            remediated_content: original.clone(),
            original_content: original,
            changed: false,
        });
    }
    let stem = path.file_stem().and_then(|s| s.to_str());
    let (remediated_doc, remediations) = remediate_document(&doc, stem, options);
    // Nothing to fix means nothing to write: the file is not re-serialized
    // just to normalize it.
    let remediated_content = if remediations.is_empty() {
        original.clone()
    } else {
        remediated_doc.serialize()
    };
    let changed = remediated_content != original;

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
            if path == &bundle_root.join("index.md")
                && let Ok(report) = remediate_file(path, options)
                && report.changed
            {
                files.push(report);
            }
            continue;
        }
        if let Ok(report) = remediate_file(path, options) {
            files.push(report);
        }
    }

    if options.root_legacy_paths {
        root_legacy_paths(&bundle_root, &mut files);
    }
    if options.declare_okf_version {
        declare_okf_version(&bundle_root, &mut files);
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

/// Adds the leading `/` to every path field that only resolves when read from
/// the bundle root, updating the reports of the files concerned.
///
/// This is the one fix that needs the whole bundle: whether a path resolves
/// depends on what else is on disk.
fn root_legacy_paths(bundle_root: &Path, files: &mut [FileFixReport]) {
    let Ok(bundle) = crate::bundle::Bundle::load(bundle_root) else {
        return;
    };
    for (concept, field, raw, rooted) in bundle.legacy_path_fields() {
        let path = concept.to_path(bundle_root);
        let Some(report) = files.iter_mut().find(|f| f.path == path) else {
            continue;
        };
        let Ok(mut doc) = Document::parse(&report.remediated_content) else {
            continue;
        };
        if !doc.frontmatter.set_path_field(&field, &rooted) {
            continue;
        }
        report.remediated_content = doc.serialize();
        report.changed = report.remediated_content != report.original_content;
        report.remediations.push(Remediation {
            kind: RemediationKind::RootedPath(field.clone()),
            description: format!(
                "rewrote `{field}` from `{raw}` to `{rooted}` so that it resolves from the bundle root"
            ),
        });
    }
}

/// Declares `okf_version` in a bundle-root `index.md` that has none, keeping
/// the rest of the file as it is.
///
/// Skipped while a path field still depends on the root-relative reading of
/// earlier versions and is not being fixed in this run: declaring the current
/// version would stop it from resolving.
fn declare_okf_version(bundle_root: &Path, files: &mut Vec<FileFixReport>) {
    let index_path = bundle_root.join("index.md");
    let existing = files.iter().position(|f| f.path == index_path);
    let current = match existing {
        Some(i) => files[i].remediated_content.clone(),
        None => match fs::read_to_string(&index_path) {
            Ok(text) => text,
            Err(_) => return,
        },
    };
    let Ok(doc) = Document::parse(&current) else {
        return;
    };
    if doc
        .frontmatter
        .get("okf_version")
        .is_some_and(|value| !value.is_blank())
    {
        return;
    }
    let unresolved_legacy = crate::bundle::Bundle::load(bundle_root).is_ok_and(|bundle| {
        bundle
            .legacy_path_fields()
            .iter()
            .any(|(concept, field, _, _)| {
                let path = concept.to_path(bundle_root);
                !files.iter().any(|f| {
                    f.path == path
                        && f.remediations
                            .iter()
                            .any(|r| r.kind == RemediationKind::RootedPath(field.clone()))
                })
            })
    });
    if unresolved_legacy {
        return;
    }

    let declaration = format!("okf_version: \"{}\"\n", crate::OKF_VERSION);
    let stripped = current.strip_prefix('\u{feff}').unwrap_or(&current);
    let remediated_content = if doc.frontmatter.contains_key("okf_version") {
        // The key is there with an empty value: fill it in where it stands.
        let mut doc = doc;
        doc.frontmatter
            .set("okf_version", Value::String(crate::OKF_VERSION.to_string()));
        doc.serialize()
    } else if Document::has_frontmatter_block(&current) {
        // Insert the key as the first line of the existing block, leaving
        // every other line as written.
        stripped.split_once('\n').map_or_else(
            || format!("---\n{declaration}---\n"),
            |(open, rest)| format!("{open}\n{declaration}{rest}"),
        )
    } else {
        format!("---\n{declaration}---\n\n{stripped}")
    };
    let remediation = Remediation {
        kind: RemediationKind::DeclaredOkfVersion,
        description: format!(
            "declared `okf_version: \"{}\"` in the bundle-root index.md",
            crate::OKF_VERSION
        ),
    };
    match existing {
        Some(i) => {
            files[i].remediated_content = remediated_content;
            files[i].changed = files[i].remediated_content != files[i].original_content;
            files[i].remediations.push(remediation);
        }
        None => files.push(FileFixReport {
            path: index_path,
            remediations: vec![remediation],
            original_content: current,
            remediated_content,
            changed: true,
        }),
    }
}

/// The markdown files a fix may rewrite: those that are part of the bundle.
///
/// The bundle's own walk is used, so that a dot-path is skipped and a
/// symbolic link that leads out of the bundle root is not followed: a fix
/// must never write to a file outside the bundle. Build output and installed
/// packages are left out as well, for a bundle that sits at the root of a
/// code repository.
fn collect_md_files(dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    let skipped = |path: &Path| {
        path.strip_prefix(dir).is_ok_and(|relative| {
            relative
                .components()
                .any(|c| matches!(c.as_os_str().to_str(), Some("target" | "node_modules")))
        })
    };
    out.extend(
        crate::bundle::markdown_files(dir)?
            .into_iter()
            .filter(|path| !skipped(path)),
    );
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
