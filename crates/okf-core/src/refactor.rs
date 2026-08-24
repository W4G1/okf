//! Knowledge refactoring, concept relocation, deletion, splitting, and merging.
//!
//! Refactoring a concept graph requires maintaining link integrity across the
//! entire bundle:
//!
//! - [`move_concept`]: Moves/renames a concept, rewrites all incoming backlinks
//!   across the bundle, and rebases outgoing relative links and frontmatter paths.
//! - [`remove_concept`]: Safely deletes a concept, checking for inbound links
//!   and optionally redirecting or unlinking them.
//! - [`split_concept`]: Extracts a section/heading into a new concept, moves
//!   relevant footnote citations/sources, and links to the new concept.
//! - [`merge_concepts`]: Consolidates two concepts, merging bodies, sources,
//!   and verification events, while redirecting incoming backlinks.

use crate::bundle::Bundle;
use crate::concept_id::ConceptId;
use crate::date::Date;
use crate::document::Document;
use crate::frontmatter::Frontmatter;
use crate::index::regenerate_indexes;
use crate::links::{self, Link, LinkKind};
use crate::log::{Log, LogDay, LogEntry};
use crate::provenance::Source;
use crate::scaffold::{current_iso_timestamp, default_author};
use crate::yaml::Value;
use std::collections::HashSet;
use std::fmt;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Error returned when a refactoring operation fails.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefactorError {
    /// The specified source concept was not found in the bundle.
    ConceptNotFound(ConceptId),
    /// The destination concept already exists in the bundle.
    ConceptAlreadyExists(ConceptId),
    /// The source and target concepts are the same.
    SameSourceAndTarget(ConceptId),
    /// The concept cannot be deleted because incoming links point to it.
    HasInboundLinks {
        /// Target concept being removed.
        target: ConceptId,
        /// Number of incoming links.
        inbound_count: usize,
        /// Concept IDs linking to the target.
        inbound_concepts: Vec<ConceptId>,
    },
    /// The requested section heading was not found in the concept body.
    SectionNotFound {
        /// The concept searched.
        concept: ConceptId,
        /// The section name searched for.
        section: String,
    },
    /// Invalid concept ID.
    InvalidConceptId(String),
    /// Filesystem I/O error.
    Io(String),
}

impl fmt::Display for RefactorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConceptNotFound(id) => write!(f, "concept '{id}' not found in bundle"),
            Self::ConceptAlreadyExists(id) => {
                write!(
                    f,
                    "destination concept '{id}' already exists (use --force to overwrite)"
                )
            }
            Self::SameSourceAndTarget(id) => {
                write!(f, "source and target concepts are identical: '{id}'")
            }
            Self::HasInboundLinks {
                target,
                inbound_count,
                inbound_concepts,
            } => {
                let joined = inbound_concepts
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(
                    f,
                    "cannot remove concept '{target}' because {inbound_count} other concept(s) link to it: [{joined}]. Use --redirect-to <target> to re-route links, --unlink to remove links, or --force to delete anyway"
                )
            }
            Self::SectionNotFound { concept, section } => {
                write!(f, "section '{section}' not found in concept '{concept}'")
            }
            Self::InvalidConceptId(msg) => write!(f, "invalid concept ID: {msg}"),
            Self::Io(msg) => write!(f, "I/O error: {msg}"),
        }
    }
}

impl std::error::Error for RefactorError {}

impl From<io::Error> for RefactorError {
    fn from(err: io::Error) -> Self {
        Self::Io(err.to_string())
    }
}

/// Options for moving or renaming a concept.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct MoveOptions {
    /// If true, simulate changes without writing to disk.
    pub dry_run: bool,
    /// Overwrite destination file if it exists.
    pub force: bool,
    /// Regenerate index.md listings after move.
    pub update_index: bool,
    /// Record the move in log.md.
    pub update_log: bool,
    /// Author attribution for the log entry.
    pub author: Option<String>,
}

impl Default for MoveOptions {
    fn default() -> Self {
        Self {
            dry_run: false,
            force: false,
            update_index: true,
            update_log: true,
            author: None,
        }
    }
}

/// Summary report of a move/rename operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MoveReport {
    /// Original concept ID.
    pub source: ConceptId,
    /// New concept ID.
    pub target: ConceptId,
    /// Original file path.
    pub source_path: PathBuf,
    /// New file path.
    pub target_path: PathBuf,
    /// Number of incoming links rewritten across the bundle.
    pub rewritten_incoming_links: usize,
    /// Number of outgoing relative links rebased in the moved concept.
    pub rebased_outgoing_links: usize,
    /// Number of frontmatter path fields rebased.
    pub rebased_frontmatter_paths: usize,
    /// List of all modified or created file paths.
    pub affected_files: Vec<PathBuf>,
    /// Whether this was a dry run.
    pub dry_run: bool,
}

impl fmt::Display for MoveReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let prefix = if self.dry_run {
            "[dry-run] would rename"
        } else {
            "renamed"
        };
        writeln!(f, "{prefix} concept {} -> {}", self.source, self.target)?;
        writeln!(
            f,
            "  rewrote {} incoming link(s)",
            self.rewritten_incoming_links
        )?;
        writeln!(
            f,
            "  rebased {} outgoing link(s)",
            self.rebased_outgoing_links
        )?;
        if self.rebased_frontmatter_paths > 0 {
            writeln!(
                f,
                "  rebased {} frontmatter path(s)",
                self.rebased_frontmatter_paths
            )?;
        }
        write!(f, "  affected {} file(s)", self.affected_files.len())
    }
}

/// Options for removing a concept.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct RemoveOptions {
    /// If true, simulate changes without writing to disk.
    pub dry_run: bool,
    /// Force deletion even if inbound links exist.
    pub force: bool,
    /// Re-route all inbound links to this concept instead.
    pub redirect_to: Option<ConceptId>,
    /// Convert inbound links to plain text (`[Text](dest)` -> `Text`).
    pub unlink: bool,
    /// Regenerate index.md listings after removal.
    pub update_index: bool,
    /// Record the deletion in log.md.
    pub update_log: bool,
    /// Author attribution for the log entry.
    pub author: Option<String>,
}

impl Default for RemoveOptions {
    fn default() -> Self {
        Self {
            dry_run: false,
            force: false,
            redirect_to: None,
            unlink: false,
            update_index: true,
            update_log: true,
            author: None,
        }
    }
}

/// Summary report of a removal operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoveReport {
    /// The concept ID that was removed.
    pub target: ConceptId,
    /// The file path that was removed.
    pub removed_path: PathBuf,
    /// Replacement concept ID if links were redirected.
    pub redirected_to: Option<ConceptId>,
    /// Number of links rewritten to a redirect target.
    pub redirected_count: usize,
    /// Number of links unlinked to plain text.
    pub unlinked_count: usize,
    /// List of all modified or removed file paths.
    pub affected_files: Vec<PathBuf>,
    /// Whether this was a dry run.
    pub dry_run: bool,
}

impl fmt::Display for RemoveReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let prefix = if self.dry_run {
            "[dry-run] would remove"
        } else {
            "removed"
        };
        if let Some(r) = &self.redirected_to {
            writeln!(
                f,
                "{prefix} concept {} (redirected {} link(s) to {r})",
                self.target, self.redirected_count
            )?;
        } else if self.unlinked_count > 0 {
            writeln!(
                f,
                "{prefix} concept {} (unlinked {} link(s))",
                self.target, self.unlinked_count
            )?;
        } else {
            writeln!(f, "{prefix} concept {}", self.target)?;
        }
        write!(f, "  affected {} file(s)", self.affected_files.len())
    }
}

/// Options for splitting a concept section into a new concept.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct SplitOptions {
    /// The heading/section title to extract (e.g. "Pricing Model" or "## Pricing Model").
    pub section: String,
    /// Optional title for the new concept (defaults to section heading text).
    pub title: Option<String>,
    /// Optional concept type for the new concept (defaults to "Concept").
    pub type_: Option<String>,
    /// Optional custom text for the replacement link in the source document.
    pub link_text: Option<String>,
    /// Overwrite destination file if it exists.
    pub force: bool,
    /// If true, simulate changes without writing to disk.
    pub dry_run: bool,
    /// Regenerate index.md listings after split.
    pub update_index: bool,
    /// Record the split in log.md.
    pub update_log: bool,
    /// Author attribution for generated frontmatter and log entry.
    pub author: Option<String>,
}

impl Default for SplitOptions {
    fn default() -> Self {
        Self {
            section: String::new(),
            title: None,
            type_: None,
            link_text: None,
            force: false,
            dry_run: false,
            update_index: true,
            update_log: true,
            author: None,
        }
    }
}

impl SplitOptions {
    /// Creates split options for the given section name.
    #[must_use]
    pub const fn new(section: String) -> Self {
        Self {
            section,
            title: None,
            type_: None,
            link_text: None,
            force: false,
            dry_run: false,
            update_index: true,
            update_log: true,
            author: None,
        }
    }
}

/// Summary report of a split operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SplitReport {
    /// Source concept ID.
    pub source: ConceptId,
    /// Newly created target concept ID.
    pub target: ConceptId,
    /// Name of the extracted section.
    pub section: String,
    /// Title of the newly created concept.
    pub target_title: String,
    /// File path of the new concept document.
    pub target_path: PathBuf,
    /// Number of lines extracted.
    pub extracted_lines_count: usize,
    /// Number of sources/footnotes moved or copied.
    pub moved_sources_count: usize,
    /// List of all modified or created file paths.
    pub affected_files: Vec<PathBuf>,
    /// Whether this was a dry run.
    pub dry_run: bool,
}

impl fmt::Display for SplitReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let prefix = if self.dry_run {
            "[dry-run] would extract"
        } else {
            "extracted"
        };
        writeln!(
            f,
            "{prefix} section '{}' from {} -> {}",
            self.section, self.source, self.target
        )?;
        writeln!(f, "  extracted {} line(s)", self.extracted_lines_count)?;
        writeln!(f, "  moved {} source/footnote(s)", self.moved_sources_count)?;
        write!(f, "  created {}", self.target_path.display())
    }
}

/// Options for merging one concept into another.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct MergeOptions {
    /// Optional heading under which to append source content in target.
    pub heading: Option<String>,
    /// Force merge even if non-fatal warnings exist.
    pub force: bool,
    /// If true, simulate changes without writing to disk.
    pub dry_run: bool,
    /// Regenerate index.md listings after merge.
    pub update_index: bool,
    /// Record the merge in log.md.
    pub update_log: bool,
    /// Author attribution for the log entry.
    pub author: Option<String>,
}

impl Default for MergeOptions {
    fn default() -> Self {
        Self {
            heading: None,
            force: false,
            dry_run: false,
            update_index: true,
            update_log: true,
            author: None,
        }
    }
}

/// Summary report of a merge operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeReport {
    /// Source concept that was merged and removed.
    pub source: ConceptId,
    /// Target concept that received the merged content.
    pub target: ConceptId,
    /// Path of the removed source file.
    pub removed_path: PathBuf,
    /// Path of the updated target file.
    pub updated_path: PathBuf,
    /// Number of incoming links rewritten from source to target.
    pub rewritten_links_count: usize,
    /// Number of sources merged into target frontmatter.
    pub merged_sources_count: usize,
    /// List of all modified or removed file paths.
    pub affected_files: Vec<PathBuf>,
    /// Whether this was a dry run.
    pub dry_run: bool,
}

impl fmt::Display for MergeReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let prefix = if self.dry_run {
            "[dry-run] would merge"
        } else {
            "merged"
        };
        writeln!(f, "{prefix} concept {} -> {}", self.source, self.target)?;
        writeln!(
            f,
            "  rewrote {} incoming link(s)",
            self.rewritten_links_count
        )?;
        writeln!(f, "  merged {} source(s)", self.merged_sources_count)?;
        writeln!(f, "  removed {}", self.removed_path.display())?;
        write!(f, "  updated {}", self.updated_path.display())
    }
}

/// Options for renaming a section heading within a concept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenameSectionOptions {
    /// If true, simulate changes without writing to disk.
    pub dry_run: bool,
    /// Record the section rename in log.md.
    pub update_log: bool,
    /// Author attribution for the log entry.
    pub author: Option<String>,
}

impl Default for RenameSectionOptions {
    fn default() -> Self {
        Self {
            dry_run: false,
            update_log: true,
            author: None,
        }
    }
}

/// Summary report of a section rename operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenameSectionReport {
    /// Concept ID containing the section.
    pub concept: ConceptId,
    /// Original section name / query.
    pub old_section: String,
    /// New section name.
    pub new_section: String,
    /// Old anchor slug.
    pub old_slug: String,
    /// New anchor slug.
    pub new_slug: String,
    /// Number of in-document anchors updated.
    pub internal_links_updated: usize,
    /// Number of external backlinks updated across the bundle.
    pub external_links_updated: usize,
    /// Affected files.
    pub affected_files: Vec<PathBuf>,
    /// Whether this was a dry run.
    pub dry_run: bool,
}

impl fmt::Display for RenameSectionReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let prefix = if self.dry_run {
            "[dry-run] would rename"
        } else {
            "renamed"
        };
        writeln!(
            f,
            "{prefix} section '{}' -> '{}' in {}",
            self.old_section, self.new_section, self.concept
        )?;
        writeln!(
            f,
            "  updated {} internal link(s)",
            self.internal_links_updated
        )?;
        writeln!(
            f,
            "  updated {} external backlink(s)",
            self.external_links_updated
        )?;
        write!(f, "  affected {} file(s)", self.affected_files.len())
    }
}

/// Computes the relative markdown link path from `from_concept` to `to_concept`.
///
/// Example:
/// - `from: auth/tokens/jwt`, `to: auth/tokens/refresh` -> `"refresh.md"`
/// - `from: auth/tokens/jwt`, `to: users/profile` -> `"../../users/profile.md"`
/// - `from: overview`, `to: tables/users` -> `"tables/users.md"`
/// - `from: tables/users`, `to: overview` -> `"../overview.md"`
#[must_use]
pub fn compute_relative_path(from_concept: &ConceptId, to_concept: &ConceptId) -> String {
    let from_dir: Vec<String> = from_concept
        .parent()
        .map(|p| p.segments().to_vec())
        .unwrap_or_default();
    let to_segments = to_concept.segments();
    let to_dir = &to_segments[..to_segments.len().saturating_sub(1)];
    let to_name = to_segments.last().map_or("", String::as_str);

    // Find common directory prefix
    let mut common_len = 0;
    while common_len < from_dir.len()
        && common_len < to_dir.len()
        && from_dir[common_len] == to_dir[common_len]
    {
        common_len += 1;
    }

    let steps_up = from_dir.len() - common_len;
    let mut parts: Vec<&str> = Vec::new();
    parts.extend(std::iter::repeat_n("..", steps_up));
    for seg in &to_dir[common_len..] {
        parts.push(seg);
    }
    let file_part = format!("{to_name}.md");
    parts.push(&file_part);
    parts.join("/")
}

/// Rebases a relative file/resource path from `old_dir` to `new_dir`.
///
/// If `rel_path` is external or absolute (`/`), it is returned as-is.
#[must_use]
pub fn rebase_relative_path(old_dir: &[String], new_dir: &[String], rel_path: &str) -> String {
    let trimmed = rel_path.trim();
    if trimmed.starts_with('/') || links::Link::classify(trimmed) == LinkKind::External {
        return trimmed.to_string();
    }

    // Split anchor if present
    let (path_part, anchor_part) = trimmed
        .find('#')
        .map_or((trimmed, ""), |idx| (&trimmed[..idx], &trimmed[idx..]));

    if path_part.is_empty() {
        return trimmed.to_string();
    }

    // Normalize old_dir + path_part into bundle-relative segments
    let mut resolved = old_dir.to_vec();
    for seg in path_part.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                resolved.pop();
            }
            other => resolved.push(other.to_string()),
        }
    }

    // Compute relative path from new_dir to resolved segments
    let resolved_dir = if resolved.is_empty() {
        &[][..]
    } else {
        &resolved[..resolved.len() - 1]
    };
    let filename = resolved.last().map_or("", String::as_str);

    let mut common_len = 0;
    while common_len < new_dir.len()
        && common_len < resolved_dir.len()
        && new_dir[common_len] == resolved_dir[common_len]
    {
        common_len += 1;
    }

    let steps_up = new_dir.len() - common_len;
    let mut parts: Vec<&str> = Vec::new();
    parts.extend(std::iter::repeat_n("..", steps_up));
    for seg in &resolved_dir[common_len..] {
        parts.push(seg);
    }
    if !filename.is_empty() {
        parts.push(filename);
    }

    let rebased = if parts.is_empty() {
        ".".to_string()
    } else {
        parts.join("/")
    };
    format!("{rebased}{anchor_part}")
}

/// Action to perform on a detected markdown link during rewrite.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkRewriteAction {
    /// Keep the link destination as-is.
    Keep,
    /// Rewrite the destination URL to a new target (preserving title and brackets).
    Rewrite(String),
    /// Unlink: replace `[Text](dest)` with plain `Text`.
    Unlink,
}

/// Rewrites inline markdown links in a document body using a callback function.
///
/// Preserves fenced code blocks and inline code spans untouched.
pub fn rewrite_markdown_links<F>(body: &str, mut rewrite_fn: F) -> (String, usize)
where
    F: FnMut(&Link, &str) -> LinkRewriteAction,
{
    let mut out_lines = Vec::new();
    let mut total_rewritten = 0;
    let mut fence: Option<char> = None;

    for line in body.lines() {
        let trimmed_start = line.trim_start();
        if let Some(f) = fence {
            if trimmed_start.starts_with(&f.to_string().repeat(3)) {
                fence = None;
            }
            out_lines.push(line.to_string());
            continue;
        }
        if trimmed_start.starts_with("```") {
            fence = Some('`');
            out_lines.push(line.to_string());
            continue;
        }
        if trimmed_start.starts_with("~~~") {
            fence = Some('~');
            out_lines.push(line.to_string());
            continue;
        }

        // Process non-code line
        let (new_line, count) = rewrite_line_links(line, &mut rewrite_fn);
        total_rewritten += count;
        out_lines.push(new_line);
    }

    let mut result = out_lines.join("\n");
    if body.ends_with('\n') {
        result.push('\n');
    }
    (result, total_rewritten)
}

fn rewrite_line_links<F>(line_text: &str, rewrite_fn: &mut F) -> (String, usize)
where
    F: FnMut(&Link, &str) -> LinkRewriteAction,
{
    let chars: Vec<char> = line_text.chars().collect();
    let mut out = String::with_capacity(line_text.len());
    let mut i = 0;
    let mut in_code_span = false;
    let mut count = 0;

    while i < chars.len() {
        if chars[i] == '`' && !is_escaped(&chars, i) {
            in_code_span = !in_code_span;
            out.push(chars[i]);
            i += 1;
            continue;
        }

        if !in_code_span
            && chars[i] == '['
            && !is_escaped(&chars, i)
            && chars.get(i + 1) != Some(&'^')
            && let Some((text, dest_raw, next_i)) = parse_inline_link_raw(&chars, i)
        {
            let clean_dest = clean_destination(&dest_raw);
            let parsed_link = Link {
                text: text.clone(),
                kind: Link::classify(&clean_dest),
                target: clean_dest.clone(),
            };

            match rewrite_fn(&parsed_link, &dest_raw) {
                LinkRewriteAction::Keep => {
                    let slice: String = chars[i..next_i].iter().collect();
                    out.push_str(&slice);
                }
                LinkRewriteAction::Rewrite(new_dest) => {
                    let title_suffix = extract_title_suffix(&dest_raw);
                    let formatted_dest = if new_dest.contains(' ') && !new_dest.starts_with('<') {
                        format!("<{new_dest}>")
                    } else {
                        new_dest
                    };
                    let _ = write!(out, "[{text}]({formatted_dest}{title_suffix})");
                    count += 1;
                }
                LinkRewriteAction::Unlink => {
                    out.push_str(&text);
                    count += 1;
                }
            }
            i = next_i;
            continue;
        }

        out.push(chars[i]);
        i += 1;
    }

    (out, count)
}

fn parse_inline_link_raw(chars: &[char], start: usize) -> Option<(String, String, usize)> {
    let mut i = start + 1;
    let mut depth = 1;
    let text_start = i;
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 1,
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
        i += 1;
    }
    if depth != 0 || i >= chars.len() {
        return None;
    }
    let text: String = chars[text_start..i].iter().collect();

    let mut j = i + 1;
    if j >= chars.len() || chars[j] != '(' {
        return None;
    }
    j += 1;
    let dest_start = j;
    let mut paren = 1;
    while j < chars.len() {
        match chars[j] {
            '\\' => j += 1,
            '(' => paren += 1,
            ')' => {
                paren -= 1;
                if paren == 0 {
                    break;
                }
            }
            _ => {}
        }
        j += 1;
    }
    if paren != 0 || j >= chars.len() {
        return None;
    }
    let dest: String = chars[dest_start..j].iter().collect();
    Some((text, dest, j + 1))
}

const fn is_escaped(chars: &[char], index: usize) -> bool {
    let mut backslashes = 0;
    let mut i = index;
    while i > 0 && chars[i - 1] == '\\' {
        backslashes += 1;
        i -= 1;
    }
    backslashes % 2 == 1
}

fn clean_destination(dest: &str) -> String {
    let d = dest.trim();
    if let Some(rest) = d.strip_prefix('<')
        && let Some(end) = rest.find('>')
    {
        return rest[..end].to_string();
    }
    strip_title(d)
}

fn strip_title(dest: &str) -> String {
    let d = dest.trim();
    if let Some(idx) = d.find([' ', '\t']) {
        let (url, rest) = d.split_at(idx);
        let rest = rest.trim_start();
        if rest.starts_with('"') || rest.starts_with('\'') {
            return url.to_string();
        }
    }
    d.to_string()
}

fn extract_title_suffix(dest: &str) -> String {
    let d = dest.trim();
    let after_dest = if let Some(rest) = d.strip_prefix('<')
        && let Some(end) = rest.find('>')
    {
        &rest[end + 1..]
    } else if let Some(idx) = d.find([' ', '\t']) {
        &d[idx..]
    } else {
        ""
    };
    let trimmed_suffix = after_dest.trim();
    if trimmed_suffix.starts_with('"') || trimmed_suffix.starts_with('\'') {
        format!(" {trimmed_suffix}")
    } else {
        String::new()
    }
}

/// Appends or prepends a structured entry to `log.md` under today's date.
///
/// # Errors
///
/// Returns an [`io::Error`] if `log.md` cannot be read or written.
pub fn append_log_entry(
    bundle_root: &Path,
    date: Date,
    kind: &str,
    text: &str,
) -> io::Result<PathBuf> {
    let log_path = bundle_root.join("log.md");
    let mut log = if log_path.exists() {
        let content = fs::read_to_string(&log_path)?;
        Log::parse(&content)
    } else {
        Log {
            title: Some("Update Log".to_string()),
            ..Default::default()
        }
    };

    let date_str = date.to_string();
    let new_entry = LogEntry {
        kind: Some(kind.to_string()),
        text: text.to_string(),
    };

    if let Some(first_day) = log.days.first_mut()
        && first_day.date == date_str
    {
        first_day.entries.push(new_entry);
    } else {
        log.days.insert(
            0,
            LogDay {
                date: date_str,
                entries: vec![new_entry],
            },
        );
    }

    fs::write(&log_path, log.to_markdown())?;
    Ok(log_path)
}

/// Derives a standard Markdown anchor slug from a heading text.
///
/// Example: `"## Pricing Tiers"` -> `"pricing-tiers"`
#[must_use]
pub fn heading_slug(text: &str) -> String {
    let clean = text.trim().trim_start_matches('#').trim();
    let mut slug = String::with_capacity(clean.len());
    for c in clean.chars() {
        if c.is_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if (c.is_whitespace() || c == '-' || c == '_') && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.trim_matches('-').to_string()
}

/// Renames a section heading within a concept and updates all internal and external anchor links.
///
/// # Errors
///
/// Returns [`RefactorError::ConceptNotFound`] if `concept_id` is not in the bundle,
/// [`RefactorError::SectionNotFound`] if the section is not found in the body, or
/// [`RefactorError::Io`] on filesystem error.
#[allow(clippy::too_many_lines)]
pub fn rename_section(
    bundle: &Bundle,
    concept_id: &ConceptId,
    old_section: &str,
    new_section: &str,
    options: &RenameSectionOptions,
) -> Result<RenameSectionReport, RefactorError> {
    let concept = bundle
        .get(concept_id)
        .ok_or_else(|| RefactorError::ConceptNotFound(concept_id.clone()))?;

    let clean_old = old_section.trim().trim_start_matches('#').trim();
    let clean_new = new_section.trim().trim_start_matches('#').trim();
    let old_slug = heading_slug(clean_old);
    let new_slug = heading_slug(clean_new);

    let mut doc = concept.document.clone();
    let mut found_heading = false;
    let mut matched_title = String::new();
    let mut new_lines = Vec::new();

    let mut fence: Option<char> = None;

    for line in doc.body.lines() {
        let trimmed = line.trim();
        let trimmed_start = line.trim_start();

        if let Some(f) = fence {
            if trimmed_start.starts_with(&f.to_string().repeat(3)) {
                fence = None;
            }
            new_lines.push(line.to_string());
            continue;
        }
        if trimmed_start.starts_with("```") {
            fence = Some('`');
            new_lines.push(line.to_string());
            continue;
        }
        if trimmed_start.starts_with("~~~") {
            fence = Some('~');
            new_lines.push(line.to_string());
            continue;
        }

        if !found_heading && trimmed.starts_with('#') {
            let hashes = trimmed.chars().take_while(|c| *c == '#').count();
            let title = trimmed[hashes..].trim();
            if title.eq_ignore_ascii_case(clean_old)
                || heading_slug(title) == old_slug
                || title
                    .replace(['-', '_'], " ")
                    .eq_ignore_ascii_case(&clean_old.replace(['-', '_'], " "))
            {
                found_heading = true;
                matched_title = title.to_string();
                let hash_prefix = &trimmed[..hashes];
                new_lines.push(format!("{hash_prefix} {clean_new}"));
                continue;
            }
        }
        new_lines.push(line.to_string());
    }

    if !found_heading {
        return Err(RefactorError::SectionNotFound {
            concept: concept_id.clone(),
            section: old_section.to_string(),
        });
    }

    let updated_body_text = if doc.body.ends_with('\n') {
        format!("{}\n", new_lines.join("\n"))
    } else {
        new_lines.join("\n")
    };

    let matched_slug = heading_slug(&matched_title);

    // 1. Rewrite in-document anchors inside the same concept
    let (rewritten_doc_body, internal_count) =
        rewrite_markdown_links(&updated_body_text, |link, _| {
            if link.kind == LinkKind::Anchor {
                let anchor_text = link.target.trim_start_matches('#');
                if anchor_text == old_slug
                    || anchor_text == matched_slug
                    || heading_slug(anchor_text) == old_slug
                    || heading_slug(anchor_text) == matched_slug
                {
                    return LinkRewriteAction::Rewrite(format!("#{new_slug}"));
                }
            }
            LinkRewriteAction::Keep
        });
    doc.body = rewritten_doc_body;

    let concept_path = concept_id.to_path(bundle.root());
    let mut affected_files = vec![concept_path.clone()];
    let mut external_count = 0;
    let mut updated_other_docs: Vec<(PathBuf, Document)> = Vec::new();

    // 2. Rewrite backlinks with #anchor pointing to this concept across the bundle
    for other_concept in bundle.concepts() {
        if other_concept.id == *concept_id {
            continue;
        }

        let mut other_doc = other_concept.document.clone();
        let other_id = &other_concept.id;

        let (new_other_body, count) = rewrite_markdown_links(&other_doc.body, |link, _| {
            if let Some(resolved_id) = link.resolve(other_id)
                && resolved_id == *concept_id
                && let Some(anchor_idx) = link.target.find('#')
            {
                let anchor_text = &link.target[anchor_idx + 1..];
                if anchor_text == old_slug
                    || anchor_text == matched_slug
                    || heading_slug(anchor_text) == old_slug
                    || heading_slug(anchor_text) == matched_slug
                {
                    let path_part = &link.target[..anchor_idx];
                    return LinkRewriteAction::Rewrite(format!("{path_part}#{new_slug}"));
                }
            }
            LinkRewriteAction::Keep
        });

        if count > 0 {
            other_doc.body = new_other_body;
            external_count += count;
            let path = other_id.to_path(bundle.root());
            affected_files.push(path.clone());
            updated_other_docs.push((path, other_doc));
        }
    }

    if !options.dry_run {
        fs::write(&concept_path, doc.serialize())?;
        for (path, other_d) in updated_other_docs {
            fs::write(&path, other_d.serialize())?;
        }

        if options.update_log {
            let today = Date::today_utc().unwrap_or(Date {
                year: 2026,
                month: 8,
                day: 24,
            });
            let author_suffix = options
                .author
                .as_ref()
                .map_or(String::new(), |a| format!(" (by {a})"));
            let log_msg = format!(
                "Renamed section `{clean_old}` to `{clean_new}` in concept `{concept_id}` (updated {internal_count} internal link(s), {external_count} external backlink(s)){author_suffix}."
            );
            let _ = append_log_entry(bundle.root(), today, "Update", &log_msg);
        }
    }

    Ok(RenameSectionReport {
        concept: concept_id.clone(),
        old_section: clean_old.to_string(),
        new_section: clean_new.to_string(),
        old_slug,
        new_slug,
        internal_links_updated: internal_count,
        external_links_updated: external_count,
        affected_files,
        dry_run: options.dry_run,
    })
}

/// Moves or renames a concept, rewriting all incoming links and rebasing outgoing links.
///
/// # Errors
///
/// Returns [`RefactorError::SameSourceAndTarget`] if `source` equals `target`,
/// [`RefactorError::ConceptNotFound`] if `source` does not exist,
/// [`RefactorError::ConceptAlreadyExists`] if `target` exists and `force` is false, or
/// [`RefactorError::Io`] on filesystem error.
#[allow(clippy::too_many_lines)]
pub fn move_concept(
    bundle: &Bundle,
    source: &ConceptId,
    target: &ConceptId,
    options: &MoveOptions,
) -> Result<MoveReport, RefactorError> {
    if source == target {
        return Err(RefactorError::SameSourceAndTarget(source.clone()));
    }

    let source_concept = bundle
        .get(source)
        .ok_or_else(|| RefactorError::ConceptNotFound(source.clone()))?;

    let target_path = target.to_path(bundle.root());
    let source_path = source.to_path(bundle.root());

    if !options.force && bundle.get(target).is_some() {
        return Err(RefactorError::ConceptAlreadyExists(target.clone()));
    }

    let source_dir: Vec<String> = source
        .parent()
        .map(|p| p.segments().to_vec())
        .unwrap_or_default();
    let target_dir: Vec<String> = target
        .parent()
        .map(|p| p.segments().to_vec())
        .unwrap_or_default();

    // 1. Rebase outgoing links inside the moved document
    let mut moved_doc = source_concept.document.clone();
    let (rebased_body, rebased_outgoing_count) =
        rewrite_markdown_links(&moved_doc.body, |link, _| {
            match link.kind {
                LinkKind::Relative => {
                    // Try resolving to a concept id
                    link.resolve(source).map_or_else(
                        || {
                            // Relative file path (non-concept)
                            let new_rel =
                                rebase_relative_path(&source_dir, &target_dir, &link.target);
                            LinkRewriteAction::Rewrite(new_rel)
                        },
                        |dest_concept| {
                            let new_rel = compute_relative_path(target, &dest_concept);
                            let anchor =
                                link.target.find('#').map_or("", |idx| &link.target[idx..]);
                            LinkRewriteAction::Rewrite(format!("{new_rel}{anchor}"))
                        },
                    )
                }
                _ => LinkRewriteAction::Keep,
            }
        });
    moved_doc.body = rebased_body;

    // 2. Rebase relative frontmatter paths in the moved document
    let mut rebased_fm_count = 0;
    rebase_frontmatter_paths(
        &mut moved_doc.frontmatter,
        &source_dir,
        &target_dir,
        &mut rebased_fm_count,
    );

    let mut affected_files = Vec::new();
    let mut rewritten_incoming_total = 0;

    // 3. Rewrite incoming links in all other documents in the bundle
    let mut updated_other_docs: Vec<(PathBuf, Document)> = Vec::new();

    for other_concept in bundle.concepts() {
        if other_concept.id == *source {
            continue;
        }

        let mut other_doc = other_concept.document.clone();
        let other_id = &other_concept.id;
        let mut modified = false;

        let (new_body, rewritten_count) = rewrite_markdown_links(&other_doc.body, |link, _| {
            if let Some(resolved_id) = link.resolve(other_id)
                && resolved_id == *source
            {
                let anchor = link.target.find('#').map_or("", |idx| &link.target[idx..]);
                match link.kind {
                    LinkKind::Absolute => {
                        LinkRewriteAction::Rewrite(format!("/{target}.md{anchor}"))
                    }
                    LinkKind::Relative => {
                        let new_rel = compute_relative_path(other_id, target);
                        LinkRewriteAction::Rewrite(format!("{new_rel}{anchor}"))
                    }
                    _ => LinkRewriteAction::Keep,
                }
            } else {
                LinkRewriteAction::Keep
            }
        });

        if rewritten_count > 0 {
            other_doc.body = new_body;
            rewritten_incoming_total += rewritten_count;
            modified = true;
        }

        // Check frontmatter sources and path references in other documents
        if rewrite_frontmatter_concept_references(
            &mut other_doc.frontmatter,
            other_id,
            source,
            target,
        ) {
            modified = true;
        }

        if modified {
            let path = other_id.to_path(bundle.root());
            affected_files.push(path.clone());
            updated_other_docs.push((path, other_doc));
        }
    }

    affected_files.push(source_path.clone());
    affected_files.push(target_path.clone());

    if !options.dry_run {
        // Create destination directory if needed
        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Write moved document to new location
        fs::write(&target_path, moved_doc.serialize())?;

        // Remove source document
        if source_path.exists() && source_path != target_path {
            fs::remove_file(&source_path)?;
        }

        // Write updated other documents
        for (path, doc) in updated_other_docs {
            fs::write(&path, doc.serialize())?;
        }

        // Regenerate indexes
        if options.update_index {
            let _ = regenerate_indexes(bundle.root());
        }

        // Update log.md
        if options.update_log {
            let today = Date::today_utc().unwrap_or(Date {
                year: 2026,
                month: 8,
                day: 24,
            });
            let author_suffix = options
                .author
                .as_ref()
                .map_or(String::new(), |a| format!(" (by {a})"));
            let log_msg = format!(
                "Renamed concept `{source}` to `{target}` (rewrote {rewritten_incoming_total} incoming links, rebased {rebased_outgoing_count} outgoing links){author_suffix}."
            );
            let _ = append_log_entry(bundle.root(), today, "Update", &log_msg);
        }
    }

    Ok(MoveReport {
        source: source.clone(),
        target: target.clone(),
        source_path,
        target_path,
        rewritten_incoming_links: rewritten_incoming_total,
        rebased_outgoing_links: rebased_outgoing_count,
        rebased_frontmatter_paths: rebased_fm_count,
        affected_files,
        dry_run: options.dry_run,
    })
}

/// Safely removes a concept from the bundle.
///
/// # Errors
///
/// Returns [`RefactorError::ConceptNotFound`] if `target` is not in the bundle,
/// [`RefactorError::HasInboundLinks`] if other concepts link to `target` and `force`/`redirect_to`/`unlink`
/// were not given, or [`RefactorError::Io`] on filesystem error.
#[allow(clippy::too_many_lines)]
pub fn remove_concept(
    bundle: &Bundle,
    target: &ConceptId,
    options: &RemoveOptions,
) -> Result<RemoveReport, RefactorError> {
    if bundle.get(target).is_none() {
        return Err(RefactorError::ConceptNotFound(target.clone()));
    }

    let inbound = bundle.backlinks(target);
    let has_inbound = !inbound.is_empty();

    if has_inbound && !options.force && options.redirect_to.is_none() && !options.unlink {
        return Err(RefactorError::HasInboundLinks {
            target: target.clone(),
            inbound_count: inbound.len(),
            inbound_concepts: inbound.to_vec(),
        });
    }

    let target_path = target.to_path(bundle.root());
    let mut affected_files = vec![target_path.clone()];
    let mut redirected_count = 0;
    let mut unlinked_count = 0;
    let mut updated_other_docs: Vec<(PathBuf, Document)> = Vec::new();

    if options.redirect_to.is_some() || options.unlink {
        for other_concept in bundle.concepts() {
            if other_concept.id == *target {
                continue;
            }

            let mut other_doc = other_concept.document.clone();
            let other_id = &other_concept.id;
            let mut modified = false;

            let (new_body, count) = rewrite_markdown_links(&other_doc.body, |link, _| {
                if let Some(resolved_id) = link.resolve(other_id)
                    && resolved_id == *target
                {
                    options.redirect_to.as_ref().map_or_else(
                        || {
                            if options.unlink {
                                LinkRewriteAction::Unlink
                            } else {
                                LinkRewriteAction::Keep
                            }
                        },
                        |redirect_id| {
                            let anchor =
                                link.target.find('#').map_or("", |idx| &link.target[idx..]);
                            match link.kind {
                                LinkKind::Absolute => {
                                    LinkRewriteAction::Rewrite(format!("/{redirect_id}.md{anchor}"))
                                }
                                LinkKind::Relative => {
                                    let new_rel = compute_relative_path(other_id, redirect_id);
                                    LinkRewriteAction::Rewrite(format!("{new_rel}{anchor}"))
                                }
                                _ => LinkRewriteAction::Keep,
                            }
                        },
                    )
                } else {
                    LinkRewriteAction::Keep
                }
            });

            if count > 0 {
                if options.redirect_to.is_some() {
                    redirected_count += count;
                } else {
                    unlinked_count += count;
                }
                other_doc.body = new_body;
                modified = true;
            }

            // Also check frontmatter sources
            if let Some(redirect_id) = &options.redirect_to
                && rewrite_frontmatter_concept_references(
                    &mut other_doc.frontmatter,
                    other_id,
                    target,
                    redirect_id,
                )
            {
                modified = true;
            }

            if modified {
                let path = other_id.to_path(bundle.root());
                affected_files.push(path.clone());
                updated_other_docs.push((path, other_doc));
            }
        }
    }

    if !options.dry_run {
        // Delete target file
        if target_path.exists() {
            fs::remove_file(&target_path)?;
        }

        // Write updated docs
        for (path, doc) in updated_other_docs {
            fs::write(&path, doc.serialize())?;
        }

        // Regenerate indexes
        if options.update_index {
            let _ = regenerate_indexes(bundle.root());
        }

        // Update log.md
        if options.update_log {
            let today = Date::today_utc().unwrap_or(Date {
                year: 2026,
                month: 8,
                day: 24,
            });
            let log_msg = options.redirect_to.as_ref().map_or_else(
                || {
                    if options.unlink {
                        format!("Removed concept `{target}` (unlinked {unlinked_count} inbound links).")
                    } else {
                        format!("Removed concept `{target}`.")
                    }
                },
                |redirect_id| {
                    format!("Removed concept `{target}` (redirected {redirected_count} inbound links to `{redirect_id}`).")
                },
            );
            let _ = append_log_entry(bundle.root(), today, "Update", &log_msg);
        }
    }

    Ok(RemoveReport {
        target: target.clone(),
        removed_path: target_path,
        redirected_to: options.redirect_to.clone(),
        redirected_count,
        unlinked_count,
        affected_files,
        dry_run: options.dry_run,
    })
}

/// Splits a section from an existing concept into a new concept.
///
/// # Errors
///
/// Returns [`RefactorError::SameSourceAndTarget`] if `source` equals `target`,
/// [`RefactorError::ConceptNotFound`] if `source` is not found,
/// [`RefactorError::ConceptAlreadyExists`] if `target` exists and `force` is false,
/// [`RefactorError::SectionNotFound`] if the section is not found in `source`, or
/// [`RefactorError::Io`] on filesystem error.
#[allow(clippy::too_many_lines)]
pub fn split_concept(
    bundle: &Bundle,
    source: &ConceptId,
    target: &ConceptId,
    options: &SplitOptions,
) -> Result<SplitReport, RefactorError> {
    if source == target {
        return Err(RefactorError::SameSourceAndTarget(source.clone()));
    }

    let source_concept = bundle
        .get(source)
        .ok_or_else(|| RefactorError::ConceptNotFound(source.clone()))?;

    if !options.force && bundle.get(target).is_some() {
        return Err(RefactorError::ConceptAlreadyExists(target.clone()));
    }

    let target_path = target.to_path(bundle.root());
    let source_path = source.to_path(bundle.root());

    let (extracted_heading, extracted_lines, new_source_body) = extract_section_from_body(
        &source_concept.document.body,
        &options.section,
        source,
        target,
        options.link_text.as_deref().or(options.title.as_deref()),
    )
    .ok_or_else(|| RefactorError::SectionNotFound {
        concept: source.clone(),
        section: options.section.clone(),
    })?;

    let target_title = options
        .title
        .clone()
        .unwrap_or_else(|| extracted_heading.clone());

    // Identify footnotes in extracted lines vs remaining source lines
    let extracted_text = extracted_lines.join("\n");
    let extracted_refs: HashSet<String> = crate::footnotes::extract_refs(&extracted_text)
        .into_iter()
        .map(|r| r.label)
        .collect();

    let remaining_refs: HashSet<String> = crate::footnotes::extract_refs(&new_source_body)
        .into_iter()
        .map(|r| r.label)
        .collect();

    let all_defs = source_concept.document.footnote_definitions();
    let mut target_defs = Vec::new();

    for def in all_defs {
        let in_extracted = extracted_refs.contains(&def.label);
        if in_extracted {
            target_defs.push(def.clone());
        }
    }

    // Build new target document
    let mut target_fm = Frontmatter::new();
    target_fm.set(
        "type",
        Value::String(
            options
                .type_
                .clone()
                .unwrap_or_else(|| "Concept".to_string()),
        ),
    );
    target_fm.set("title", Value::String(target_title.clone()));
    target_fm.set(
        "status",
        Value::String(source_concept.document.frontmatter.status().to_string()),
    );

    let author = options.author.clone().unwrap_or_else(default_author);
    let mut gen_map = crate::yaml::Mapping::new();
    gen_map.insert("by", Value::String(author));
    gen_map.insert("at", Value::String(current_iso_timestamp()));
    target_fm.set("generated", Value::Mapping(gen_map));

    // Copy relevant sources to target frontmatter
    let all_sources = source_concept.document.frontmatter.sources();
    let mut target_sources = Vec::new();
    let mut kept_sources = Vec::new();

    for src in all_sources {
        let src_id = src.id.as_deref().unwrap_or("");
        let in_extracted = extracted_refs.contains(src_id);
        let in_remaining = remaining_refs.contains(src_id);
        if in_extracted {
            target_sources.push(src.clone());
        }
        if in_remaining {
            kept_sources.push(src);
        }
    }

    if !target_sources.is_empty() {
        let seq = target_sources.iter().map(Source::to_yaml_value).collect();
        target_fm.set("sources", Value::Sequence(seq));
    }

    let mut target_body = format!("# {target_title}\n\n{}\n", extracted_text.trim());
    if !target_defs.is_empty() {
        target_body.push('\n');
        for def in &target_defs {
            let _ = writeln!(target_body, "[^{}]: {}", def.label, def.text);
        }
    }

    let target_doc = Document::new(target_fm, target_body);

    // Update source document
    let mut new_source_doc = source_concept.document.clone();
    new_source_doc.body = new_source_body;
    if kept_sources.is_empty() {
        new_source_doc.frontmatter.remove("sources");
    } else {
        let seq = kept_sources.iter().map(Source::to_yaml_value).collect();
        new_source_doc
            .frontmatter
            .set("sources", Value::Sequence(seq));
    }

    let affected_files = vec![source_path.clone(), target_path.clone()];

    if !options.dry_run {
        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&target_path, target_doc.serialize())?;
        fs::write(&source_path, new_source_doc.serialize())?;

        if options.update_index {
            let _ = regenerate_indexes(bundle.root());
        }

        if options.update_log {
            let today = Date::today_utc().unwrap_or(Date {
                year: 2026,
                month: 8,
                day: 24,
            });
            let author_suffix = options
                .author
                .as_ref()
                .map_or(String::new(), |a| format!(" (by {a})"));
            let log_msg = format!(
                "Extracted section `{}` from `{source}` into new concept `{target}`{author_suffix}.",
                options.section
            );
            let _ = append_log_entry(bundle.root(), today, "Creation", &log_msg);
        }
    }

    Ok(SplitReport {
        source: source.clone(),
        target: target.clone(),
        section: options.section.clone(),
        target_title,
        target_path,
        extracted_lines_count: extracted_lines.len(),
        moved_sources_count: target_sources.len(),
        affected_files,
        dry_run: options.dry_run,
    })
}

/// Merges `source` concept into `target` concept and deletes `source`.
///
/// # Errors
///
/// Returns [`RefactorError::SameSourceAndTarget`] if `source` equals `target`,
/// [`RefactorError::ConceptNotFound`] if either `source` or `target` is not found, or
/// [`RefactorError::Io`] on filesystem error.
#[allow(clippy::too_many_lines)]
pub fn merge_concepts(
    bundle: &Bundle,
    source: &ConceptId,
    target: &ConceptId,
    options: &MergeOptions,
) -> Result<MergeReport, RefactorError> {
    if source == target {
        return Err(RefactorError::SameSourceAndTarget(source.clone()));
    }

    let source_concept = bundle
        .get(source)
        .ok_or_else(|| RefactorError::ConceptNotFound(source.clone()))?;
    let target_concept = bundle
        .get(target)
        .ok_or_else(|| RefactorError::ConceptNotFound(target.clone()))?;

    let source_path = source.to_path(bundle.root());
    let target_path = target.to_path(bundle.root());

    let source_dir: Vec<String> = source
        .parent()
        .map(|p| p.segments().to_vec())
        .unwrap_or_default();
    let target_dir: Vec<String> = target
        .parent()
        .map(|p| p.segments().to_vec())
        .unwrap_or_default();

    // 1. Rebase outgoing links in source body to target directory
    let (rebased_source_body, _) =
        rewrite_markdown_links(&source_concept.document.body, |link, _| match link.kind {
            LinkKind::Relative => link.resolve(source).map_or_else(
                || {
                    let new_rel = rebase_relative_path(&source_dir, &target_dir, &link.target);
                    LinkRewriteAction::Rewrite(new_rel)
                },
                |dest_concept| {
                    if dest_concept == *target {
                        LinkRewriteAction::Keep
                    } else {
                        let new_rel = compute_relative_path(target, &dest_concept);
                        let anchor = link.target.find('#').map_or("", |idx| &link.target[idx..]);
                        LinkRewriteAction::Rewrite(format!("{new_rel}{anchor}"))
                    }
                },
            ),
            _ => LinkRewriteAction::Keep,
        });

    // 2. Merge sources and resolve ID collisions
    let mut target_doc = target_concept.document.clone();
    let mut target_sources = target_doc.frontmatter.sources();
    let target_defs = target_doc.footnote_definitions();
    let mut existing_ids: HashSet<String> = target_sources
        .iter()
        .filter_map(|s| s.id.clone())
        .chain(target_defs.iter().map(|d| d.label.clone()))
        .collect();

    let mut merged_sources_count = 0;
    let mut source_body_final = rebased_source_body;

    for mut src in source_concept.document.frontmatter.sources() {
        if let Some(orig_id) = src.id.clone() {
            if existing_ids.contains(&orig_id) {
                // Check if identical source already exists
                let is_identical = target_sources
                    .iter()
                    .any(|s| s.id.as_deref() == Some(&orig_id) && s.resource == src.resource);
                if !is_identical {
                    // Remap id
                    let new_id = format!("{}_{orig_id}", source.name());
                    source_body_final = source_body_final
                        .replace(&format!("[^{orig_id}]"), &format!("[^{new_id}]"));
                    source_body_final = source_body_final
                        .replace(&format!("[^{orig_id}]:"), &format!("[^{new_id}]:"));
                    src.id = Some(new_id.clone());
                    existing_ids.insert(new_id);
                    target_sources.push(src);
                    merged_sources_count += 1;
                }
            } else {
                existing_ids.insert(orig_id);
                target_sources.push(src);
                merged_sources_count += 1;
            }
        } else {
            target_sources.push(src);
            merged_sources_count += 1;
        }
    }

    if !target_sources.is_empty() {
        let seq = target_sources.iter().map(Source::to_yaml_value).collect();
        target_doc.frontmatter.set("sources", Value::Sequence(seq));
    }

    // 3. Merge verified events
    let mut target_verified = target_doc.frontmatter.verified();
    for v in source_concept.document.frontmatter.verified() {
        if !target_verified.contains(&v) {
            target_verified.push(v);
        }
    }
    if !target_verified.is_empty() {
        let seq = target_verified
            .iter()
            .map(|v| {
                let mut map = crate::yaml::Mapping::new();
                if let Some(by) = &v.by {
                    map.insert("by", Value::String(by.as_str().to_string()));
                }
                if let Some(at) = &v.at {
                    map.insert("at", Value::String(at.raw.clone()));
                }
                Value::Mapping(map)
            })
            .collect();
        target_doc.frontmatter.set("verified", Value::Sequence(seq));
    }

    // 4. Append body under heading
    let heading = options.heading.clone().unwrap_or_else(|| {
        let source_title = source_concept
            .document
            .frontmatter
            .title()
            .map_or_else(|| source.name().to_string(), std::borrow::Cow::into_owned);
        format!("## {source_title}")
    });

    let body_to_append = {
        let trimmed = source_body_final.trim();
        trimmed.strip_prefix('#').map_or(trimmed, |rest| {
            let first_line = rest.lines().next().unwrap_or("");
            if first_line.starts_with('#') {
                trimmed
            } else {
                rest[first_line.len()..].trim_start()
            }
        })
    };

    target_doc.body = format!(
        "{}\n\n{heading}\n\n{body_to_append}\n",
        target_doc.body.trim_end()
    );

    // 5. Rewrite incoming backlinks across the bundle
    let mut affected_files = vec![source_path.clone(), target_path.clone()];
    let mut updated_other_docs: Vec<(PathBuf, Document)> = Vec::new();
    let mut rewritten_links_total = 0;

    for other_concept in bundle.concepts() {
        if other_concept.id == *source || other_concept.id == *target {
            continue;
        }

        let mut other_doc = other_concept.document.clone();
        let other_id = &other_concept.id;
        let mut modified = false;

        let (new_body, count) = rewrite_markdown_links(&other_doc.body, |link, _| {
            if let Some(resolved_id) = link.resolve(other_id)
                && resolved_id == *source
            {
                let anchor = link.target.find('#').map_or("", |idx| &link.target[idx..]);
                match link.kind {
                    LinkKind::Absolute => {
                        LinkRewriteAction::Rewrite(format!("/{target}.md{anchor}"))
                    }
                    LinkKind::Relative => {
                        let new_rel = compute_relative_path(other_id, target);
                        LinkRewriteAction::Rewrite(format!("{new_rel}{anchor}"))
                    }
                    _ => LinkRewriteAction::Keep,
                }
            } else {
                LinkRewriteAction::Keep
            }
        });

        if count > 0 {
            other_doc.body = new_body;
            rewritten_links_total += count;
            modified = true;
        }

        if rewrite_frontmatter_concept_references(
            &mut other_doc.frontmatter,
            other_id,
            source,
            target,
        ) {
            modified = true;
        }

        if modified {
            let path = other_id.to_path(bundle.root());
            affected_files.push(path.clone());
            updated_other_docs.push((path, other_doc));
        }
    }

    if !options.dry_run {
        // Write updated target
        fs::write(&target_path, target_doc.serialize())?;

        // Remove source file
        if source_path.exists() {
            fs::remove_file(&source_path)?;
        }

        // Write updated other docs
        for (path, doc) in updated_other_docs {
            fs::write(&path, doc.serialize())?;
        }

        // Regenerate indexes
        if options.update_index {
            let _ = regenerate_indexes(bundle.root());
        }

        // Update log.md
        if options.update_log {
            let today = Date::today_utc().unwrap_or(Date {
                year: 2026,
                month: 8,
                day: 24,
            });
            let author_suffix = options
                .author
                .as_ref()
                .map_or(String::new(), |a| format!(" (by {a})"));
            let log_msg = format!(
                "Merged concept `{source}` into `{target}` (rewrote {rewritten_links_total} inbound links){author_suffix}."
            );
            let _ = append_log_entry(bundle.root(), today, "Update", &log_msg);
        }
    }

    Ok(MergeReport {
        source: source.clone(),
        target: target.clone(),
        removed_path: source_path,
        updated_path: target_path,
        rewritten_links_count: rewritten_links_total,
        merged_sources_count,
        affected_files,
        dry_run: options.dry_run,
    })
}

fn rebase_frontmatter_paths(
    fm: &mut Frontmatter,
    old_dir: &[String],
    new_dir: &[String],
    count: &mut usize,
) {
    if let Some(Value::String(s)) = fm.get("computation")
        && !s.starts_with('/')
        && links::Link::classify(s) == LinkKind::Relative
    {
        fm.set(
            "computation",
            Value::String(rebase_relative_path(old_dir, new_dir, s)),
        );
        *count += 1;
    }

    if let Some(Value::Mapping(map)) = fm.get_mut("executor")
        && let Some(Value::String(res)) = map.get("resource")
        && !res.starts_with('/')
        && links::Link::classify(res) == LinkKind::Relative
    {
        map.insert(
            "resource",
            Value::String(rebase_relative_path(old_dir, new_dir, res)),
        );
        *count += 1;
    }

    if let Some(Value::Mapping(map)) = fm.get_mut("attester")
        && let Some(Value::String(res)) = map.get("resource")
        && !res.starts_with('/')
        && links::Link::classify(res) == LinkKind::Relative
    {
        map.insert(
            "resource",
            Value::String(rebase_relative_path(old_dir, new_dir, res)),
        );
        *count += 1;
    }

    if let Some(Value::Sequence(sources)) = fm.get_mut("sources") {
        for src_val in sources {
            if let Value::Mapping(src_map) = src_val
                && let Some(Value::String(res)) = src_map.get("resource")
                && !res.starts_with('/')
                && links::Link::classify(res) == LinkKind::Relative
            {
                src_map.insert(
                    "resource",
                    Value::String(rebase_relative_path(old_dir, new_dir, res)),
                );
                *count += 1;
            }
        }
    }
}

fn rewrite_frontmatter_concept_references(
    fm: &mut Frontmatter,
    from_id: &ConceptId,
    old_target: &ConceptId,
    new_target: &ConceptId,
) -> bool {
    let mut modified = false;

    if let Some(Value::Sequence(sources)) = fm.get_mut("sources") {
        for src_val in sources {
            if let Value::Mapping(src_map) = src_val
                && let Some(Value::String(res)) = src_map.get("resource")
            {
                let link = Link {
                    text: String::new(),
                    kind: Link::classify(res),
                    target: res.clone(),
                };
                if let Some(resolved) = link.resolve(from_id)
                    && resolved == *old_target
                {
                    let new_res = match link.kind {
                        LinkKind::Absolute => format!("/{new_target}.md"),
                        LinkKind::Relative => compute_relative_path(from_id, new_target),
                        _ => continue,
                    };
                    src_map.insert("resource", Value::String(new_res));
                    modified = true;
                }
            }
        }
    }

    modified
}

/// Extracts a section matching `section_query` from body lines and generates replacement.
fn extract_section_from_body(
    body: &str,
    section_query: &str,
    source: &ConceptId,
    target: &ConceptId,
    custom_link_text: Option<&str>,
) -> Option<(String, Vec<String>, String)> {
    let clean_query = section_query.trim().trim_start_matches('#').trim();
    let query_slug = heading_slug(clean_query);
    let lines: Vec<&str> = body.lines().collect();

    let mut match_idx = None;
    let mut match_level = 1;
    let mut match_title = String::new();
    let mut fence: Option<char> = None;

    for (i, line) in lines.iter().enumerate() {
        let trimmed_start = line.trim_start();
        if let Some(f) = fence {
            if trimmed_start.starts_with(&f.to_string().repeat(3)) {
                fence = None;
            }
            continue;
        }
        if trimmed_start.starts_with("```") {
            fence = Some('`');
            continue;
        }
        if trimmed_start.starts_with("~~~") {
            fence = Some('~');
            continue;
        }

        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            let hashes = trimmed.chars().take_while(|c| *c == '#').count();
            let title = trimmed[hashes..].trim();
            if title.eq_ignore_ascii_case(clean_query)
                || heading_slug(title) == query_slug
                || title
                    .replace(['-', '_'], " ")
                    .eq_ignore_ascii_case(&clean_query.replace(['-', '_'], " "))
            {
                match_idx = Some(i);
                match_level = hashes;
                match_title = title.to_string();
                break;
            }
        }
    }

    let start_idx = match_idx?;
    let mut end_idx = lines.len();
    fence = None;

    for (i, line) in lines.iter().enumerate().skip(start_idx + 1) {
        let trimmed_start = line.trim_start();
        if let Some(f) = fence {
            if trimmed_start.starts_with(&f.to_string().repeat(3)) {
                fence = None;
            }
            continue;
        }
        if trimmed_start.starts_with("```") {
            fence = Some('`');
            continue;
        }
        if trimmed_start.starts_with("~~~") {
            fence = Some('~');
            continue;
        }

        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            let hashes = trimmed.chars().take_while(|c| *c == '#').count();
            if hashes <= match_level {
                end_idx = i;
                break;
            }
        }
    }

    let heading_line = lines[start_idx];
    let extracted_content: Vec<String> = lines[start_idx + 1..end_idx]
        .iter()
        .map(ToString::to_string)
        .collect();

    let rel_link = compute_relative_path(source, target);
    let lt = custom_link_text.unwrap_or(&match_title);
    let replacement = format!("{heading_line}\n\nSee [{lt}]({rel_link}).\n");

    let mut remaining_lines: Vec<String> = Vec::new();
    for line in &lines[..start_idx] {
        remaining_lines.push(line.to_string());
    }
    remaining_lines.push(replacement);
    for line in &lines[end_idx..] {
        remaining_lines.push(line.to_string());
    }

    let mut new_body = remaining_lines.join("\n");
    if body.ends_with('\n') {
        new_body.push('\n');
    }

    Some((match_title, extracted_content, new_body))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_relative_path() {
        let from = ConceptId::parse("auth/tokens/jwt").unwrap();
        let to_same = ConceptId::parse("auth/tokens/refresh").unwrap();
        assert_eq!(compute_relative_path(&from, &to_same), "refresh.md");

        let to_up = ConceptId::parse("auth/user").unwrap();
        assert_eq!(compute_relative_path(&from, &to_up), "../user.md");

        let to_deep = ConceptId::parse("billing/invoicing/pdf").unwrap();
        assert_eq!(
            compute_relative_path(&from, &to_deep),
            "../../billing/invoicing/pdf.md"
        );

        let root = ConceptId::parse("overview").unwrap();
        assert_eq!(compute_relative_path(&root, &from), "auth/tokens/jwt.md");
        assert_eq!(compute_relative_path(&from, &root), "../../overview.md");
    }

    #[test]
    fn test_rebase_relative_path() {
        let old_dir = vec!["auth".to_string(), "tokens".to_string()];
        let new_dir = vec!["security".to_string()];

        let rebased = rebase_relative_path(&old_dir, &new_dir, "../scripts/calc.py");
        assert_eq!(rebased, "../auth/scripts/calc.py");

        let rebased_anchor = rebase_relative_path(&old_dir, &new_dir, "../user.md#profile");
        assert_eq!(rebased_anchor, "../auth/user.md#profile");
    }

    #[test]
    fn test_rewrite_markdown_links() {
        let body = "\
# Title

See [User Guide](../guides/user.md) and [Profile](/users/profile.md#info).
Also `[Code Link](../not/a/link.md)` should not change.

```python
# [Python Link](../ignored.md)
pass
```
";

        let (rewritten, count) = rewrite_markdown_links(body, |link, _| {
            if link.target.starts_with("../guides/user.md") {
                LinkRewriteAction::Rewrite("../../docs/user.md".to_string())
            } else if link.target.starts_with("/users/profile.md") {
                LinkRewriteAction::Unlink
            } else {
                LinkRewriteAction::Keep
            }
        });

        assert_eq!(count, 2);
        assert!(rewritten.contains("[User Guide](../../docs/user.md)"));
        assert!(rewritten.contains("and Profile."));
        assert!(rewritten.contains("`[Code Link](../not/a/link.md)`"));
        assert!(rewritten.contains("# [Python Link](../ignored.md)"));
    }
}
