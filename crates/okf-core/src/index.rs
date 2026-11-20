//! Generation of `index.md` directory listings.
//!
//! Index files support **progressive disclosure**: they let a human or agent
//! see what a directory holds before opening individual documents. Grouping is
//! by concept `type`, which is also how an `Attested Computation` becomes
//! discoverable from an index.
//!
//! An index lists the concepts of its directory and its subdirectories.
//! Markdown files that are not concepts (no frontmatter, or no `type`) and
//! anything whose name starts with `.` are left out, as they are not part of
//! the knowledge the bundle carries.
//!
//! A bundle-root `index.md` may open with a frontmatter block of bundle
//! metadata (`okf_version`, `title`, `description`, `license`, and extension
//! keys). Regeneration keeps that block exactly as written. An index in a
//! subdirectory has no frontmatter.
//!
//! This is a port of the reference `bundle/index.py`'s `regenerate_indexes` and
//! `_build_index_text`. The reference synthesizes subdirectory descriptions
//! with an LLM; since OKF tooling must not require any particular model or
//! network access, the description synthesizer here is a pluggable closure with
//! a deterministic default ([`default_synthesize`]). Ported to
//! Rust and modified from the original Apache-2.0 Python source; see the NOTICE
//! file.

use crate::bundle::Bundle;
use crate::document::Document;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const INDEX_FILE: &str = "index.md";

/// One row in a generated index, mirroring the reference's
/// `(type, title, relative_link, description)` tuple.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexEntry {
    /// The concept type, or `"Subdirectories"` for a child directory.
    pub type_: String,
    /// Display title.
    pub title: String,
    /// Relative link target.
    pub link: String,
    /// One-line description (may be empty).
    pub description: String,
}

/// Builds the markdown text of an `index.md` from a set of entries: entries are
/// grouped by type under `#`-headings (types sorted ascending), and within each
/// group sorted by title (case-insensitive).
#[must_use]
pub fn build_index_text(entries: &[IndexEntry]) -> String {
    build_index_text_impl(entries, encode_link_destination)
}

/// Builds index text when entry links are already percent-encoded. The index
/// generator uses this for filenames obtained as raw `OsStr` bytes; the public
/// [`build_index_text`] function continues to accept ordinary path strings.
fn build_index_text_with_encoded_links(entries: &[IndexEntry]) -> String {
    build_index_text_impl(entries, str::to_owned)
}

fn build_index_text_impl<F>(entries: &[IndexEntry], encode_link: F) -> String
where
    F: Fn(&str) -> String,
{
    let mut grouped: BTreeMap<String, Vec<(&str, &str, &str)>> = BTreeMap::new();
    for e in entries {
        let key = if e.type_.is_empty() {
            "Other".to_string()
        } else {
            e.type_.clone()
        };
        grouped
            .entry(key)
            .or_default()
            .push((&e.title, &e.link, &e.description));
    }

    let mut sections: Vec<String> = Vec::new();
    for (typ, mut items) in grouped {
        items.sort_by_key(|a| a.0.to_lowercase());
        let mut lines = vec![format!("# {}", escape_markdown_text(&typ)), String::new()];
        for (title, link, desc) in items {
            let title = escape_markdown_text(title);
            let link = encode_link(link);
            let desc = escape_markdown_text(desc);
            let suffix = if desc.is_empty() {
                String::new()
            } else {
                format!(" - {desc}")
            };
            lines.push(format!("* [{title}]({link}){suffix}"));
        }
        sections.push(lines.join("\n"));
    }
    format!("{}\n", sections.join("\n\n"))
}

/// Escapes Markdown delimiters before placing arbitrary text in an index. This
/// keeps titles and descriptions from creating links, images, or raw HTML while
/// retaining ordinary punctuation such as `*` and `.` in the rendered text;
/// line breaks become spaces because index descriptions are one-line values.
fn escape_markdown_text(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\n' | '\r' => escaped.push(' '),
            '\\' | '[' | ']' | '<' | '>' | '&' => {
                escaped.push('\\');
                escaped.push(c);
            }
            c => escaped.push(c),
        }
    }
    escaped
}

/// Percent-encodes a relative Markdown destination. Keeping only unreserved
/// URI bytes and path separators makes brackets, parentheses, quotes, spaces,
/// `#`, and `%` unable to alter the link syntax while preserving the filename
/// that the destination addresses.
fn encode_link_destination(link: &str) -> String {
    percent_encode_path(link.as_bytes())
}

fn percent_encode_path(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(bytes.len());
    for &byte in bytes {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/') {
            encoded.push(byte as char);
        } else {
            encoded.push('%');
            encoded.push(HEX[(byte >> 4) as usize] as char);
            encoded.push(HEX[(byte & 0x0f) as usize] as char);
        }
    }
    encoded
}

/// Encodes one filesystem component without first converting it through a
/// lossy UTF-8 representation. This keeps an index destination usable even
/// when the filesystem permits a non-UTF-8 filename.
fn encoded_component(name: &OsStr) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        percent_encode_path(name.as_bytes())
    }
    #[cfg(not(unix))]
    {
        percent_encode_path(name.to_string_lossy().as_bytes())
    }
}

/// A synthesizer for subdirectory descriptions: given the directory's path
/// (relative to the bundle root) and its child `(title, description)` pairs,
/// returns a one-line description.
pub type Synthesize<'a> = dyn Fn(&str, &[(String, String)]) -> String + 'a;

/// The default, deterministic synthesizer: lists the child titles.
///
/// Used when no custom (for example LLM-backed) synthesizer is supplied. The
/// wording is the reference `synthesize_description`'s own `_fallback`, which is
/// what it writes when its model call fails, so an index generated here reads
/// the same as one generated there without a model.
#[must_use]
pub fn default_synthesize(_rel: &str, children: &[(String, String)]) -> String {
    if children.is_empty() {
        return String::new();
    }
    let titles: Vec<&str> = children
        .iter()
        .map(|(title, _)| title.as_str())
        .filter(|title| !title.is_empty())
        .collect();
    let titles = if titles.is_empty() {
        "no titled entries".to_string()
    } else {
        titles.join(", ")
    };
    format!("Contains {} entries: {titles}.", children.len())
}

/// Regenerates every `index.md` in the bundle using [`default_synthesize`].
///
/// # Errors
///
/// Returns the underlying [`io::Error`] from any directory walk or file write.
pub fn regenerate_indexes(bundle_root: impl AsRef<Path>) -> io::Result<Vec<PathBuf>> {
    regenerate_indexes_with(bundle_root, &default_synthesize)
}

/// Regenerates every `index.md` in the bundle, deriving each subdirectory's
/// description with the supplied synthesizer.
///
/// Directories are processed deepest-first so a parent index can reuse the
/// descriptions computed for its children. A directory with nothing to list is
/// skipped, and is only listed by its parent if it already has an `index.md`.
/// Returns the paths of the index files written.
///
/// The frontmatter of the bundle-root `index.md` is kept verbatim. A root
/// index that has none gains `okf_version`, unless a path field in the bundle
/// still depends on the root-relative reading of earlier OKF versions:
/// declaring the current version would stop such a field from resolving, so
/// the index is then written without a declaration.
///
/// # Errors
///
/// Returns the underlying [`io::Error`] from any directory walk or file write.
pub fn regenerate_indexes_with(
    bundle_root: impl AsRef<Path>,
    synthesize: &Synthesize,
) -> io::Result<Vec<PathBuf>> {
    let indexes = plan_indexes(bundle_root.as_ref(), synthesize)?;
    let mut written = Vec::with_capacity(indexes.len());
    for (path, text) in indexes {
        fs::write(&path, text)?;
        written.push(path);
    }
    Ok(written)
}

/// Computes the generated text without writing it. Refactors use the same
/// generator before and after editing to distinguish generated listings from
/// hand-written indexes.
fn plan_indexes(bundle_root: &Path, synthesize: &Synthesize) -> io::Result<Vec<(PathBuf, String)>> {
    let mut planned = Vec::new();
    if !bundle_root.exists() {
        return Ok(planned);
    }

    // The markdown files that are part of the bundle. A dot-path, or a
    // symbolic link that leads out of the root, is not, and is not listed.
    let bundle_files: HashSet<PathBuf> = crate::bundle::markdown_files(bundle_root)?
        .into_iter()
        .collect();
    let mut directories = directories_to_index(&bundle_files, bundle_root);
    // Deepest-first; ties broken by path for determinism.
    directories.sort_by(|a, b| {
        let da = depth(bundle_root, a);
        let db = depth(bundle_root, b);
        db.cmp(&da).then_with(|| a.cmp(b))
    });

    let mut dir_descriptions: HashMap<PathBuf, String> = HashMap::new();
    // Directories that have an `index.md` a parent can link to.
    let mut indexed: HashSet<PathBuf> = HashSet::new();

    for directory in &directories {
        let mut entries: Vec<IndexEntry> = Vec::new();

        let mut children: Vec<PathBuf> = fs::read_dir(directory)?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .collect();
        children.sort();

        for child in children {
            let name = child
                .file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().to_string());
            // Dot-paths are not part of the bundle, and reserved files are
            // never concepts.
            if name.starts_with('.') || crate::bundle::RESERVED_FILENAMES.contains(&name.as_str()) {
                continue;
            }
            if bundle_files.contains(&child) {
                // A file without `type` is not a concept, and is not listed.
                let Some(doc) = load_doc(&child).filter(Document::is_concept) else {
                    continue;
                };
                let stem = child
                    .file_stem()
                    .map_or_else(String::new, |s| s.to_string_lossy().to_string());
                // An empty `title` falls back to the filename, as the spec permits
                // and the reference's `fm.get("title") or child.stem` does.
                let title = doc
                    .frontmatter
                    .title()
                    .map_or(stem, std::borrow::Cow::into_owned);
                let description = doc
                    .frontmatter
                    .description()
                    .map_or_else(String::new, std::borrow::Cow::into_owned);
                let type_ = doc
                    .frontmatter
                    .type_()
                    .map_or_else(String::new, std::borrow::Cow::into_owned);
                entries.push(IndexEntry {
                    type_,
                    title,
                    link: encoded_component(child.file_name().unwrap_or_default()),
                    description,
                });
            } else if child.is_dir()
                && !child.is_symlink()
                && (indexed.contains(&child) || child.join(INDEX_FILE).is_file())
            {
                let description = dir_descriptions.get(&child).cloned().unwrap_or_default();
                let encoded_name = encoded_component(child.file_name().unwrap_or_default());
                entries.push(IndexEntry {
                    type_: "Subdirectories".to_string(),
                    title: name,
                    link: format!("{encoded_name}/{INDEX_FILE}"),
                    description,
                });
            }
        }

        if entries.is_empty() {
            continue;
        }

        // An `index.md` that is a symbolic link out of the bundle root is not
        // part of the bundle, and must not be written through.
        if crate::bundle::leads_outside(bundle_root, &directory.join(INDEX_FILE)) {
            continue;
        }
        planned.push((
            directory.join(INDEX_FILE),
            index_text(directory, bundle_root, &entries),
        ));
        indexed.insert(directory.clone());

        if directory == bundle_root {
            continue;
        }

        let pairs: Vec<(String, String)> = entries
            .iter()
            .map(|e| (e.title.clone(), e.description.clone()))
            .collect();
        let desc = if pairs.len() == 1 && !pairs[0].1.is_empty() {
            pairs[0].1.clone()
        } else {
            let rel = directory
                .strip_prefix(bundle_root)
                .unwrap_or(directory)
                .to_string_lossy()
                .to_string();
            synthesize(&rel, &pairs)
        };
        dir_descriptions.insert(directory.clone(), desc);
    }

    Ok(planned)
}

/// Index ownership captured before a refactor. Only exact generated bodies
/// are considered safe to replace; ambiguous or customized listings are kept.
/// New indexes may be created, and unchanged generated indexes are not written.
pub(crate) struct RefactorIndexes {
    protected: HashSet<PathBuf>,
    generated: HashSet<PathBuf>,
}

impl RefactorIndexes {
    pub(crate) fn capture(bundle_root: &Path) -> io::Result<Self> {
        let expected: HashMap<PathBuf, String> = plan_indexes(bundle_root, &default_synthesize)?
            .into_iter()
            .collect();
        let mut protected = HashSet::new();
        let mut generated = HashSet::new();
        let root_index = bundle_root.join(INDEX_FILE);
        for path in crate::bundle::markdown_files(bundle_root)? {
            if path.file_name() != Some(OsStr::new(INDEX_FILE)) {
                continue;
            }
            let existing = fs::read_to_string(&path)?;
            let expected_text = expected.get(&path).map_or("\n", String::as_str);
            // Only a root index may carry frontmatter. Treat a subdirectory
            // block as custom too, even if its body looks generated: removing
            // invalid metadata is not part of a concept refactor.
            let matches = if path == root_index {
                index_body(&existing) == index_body(expected_text)
            } else {
                existing == expected_text
            };
            if matches {
                generated.insert(path);
            } else {
                protected.insert(path);
            }
        }
        Ok(Self {
            protected,
            generated,
        })
    }

    pub(crate) fn apply(self, bundle_root: &Path) -> io::Result<()> {
        let mut planned = plan_indexes(bundle_root, &default_synthesize)?;
        let planned_paths: HashSet<PathBuf> =
            planned.iter().map(|(path, _)| path.clone()).collect();
        // The normal generator skips empty directories. A generated listing
        // that just lost its last concept must instead have stale rows cleared.
        for path in self.generated.difference(&planned_paths) {
            let directory = path.parent().unwrap_or(bundle_root);
            if directory.is_dir() && !crate::bundle::leads_outside(bundle_root, path) {
                planned.push((path.clone(), index_text(directory, bundle_root, &[])));
            }
        }
        for (path, text) in planned {
            if !self.protected.contains(&path) {
                match fs::read_to_string(&path) {
                    Ok(existing) if existing == text => continue,
                    Ok(_) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
                fs::write(path, text)?;
            }
        }
        Ok(())
    }
}

/// The body after an optional frontmatter block and its blank separator.
fn index_body(text: &str) -> &str {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.split_inclusive('\n');
    if !lines.next().is_some_and(crate::document::is_delimiter) {
        return text;
    }
    let mut offset = text.find('\n').map_or(text.len(), |i| i + 1);
    for line in lines {
        offset += line.len();
        if crate::document::is_delimiter(line) {
            let body = &text[offset..];
            return body
                .strip_prefix("\r\n")
                .or_else(|| body.strip_prefix('\n'))
                .unwrap_or(body);
        }
    }
    text
}

fn load_doc(path: &Path) -> Option<Document> {
    let text = fs::read_to_string(path).ok()?;
    Document::parse(&text).ok()
}

fn index_text(directory: &Path, bundle_root: &Path, entries: &[IndexEntry]) -> String {
    let body = build_index_text_with_encoded_links(entries);
    if directory == bundle_root {
        format!(
            "{}{body}",
            root_frontmatter(bundle_root, &directory.join(INDEX_FILE))
        )
    } else {
        body
    }
}

/// The frontmatter block to open the bundle-root `index.md` with, delimiters
/// and trailing blank line included, or an empty string for none.
///
/// A producer that regenerates `index.md` must keep the existing frontmatter
/// of the bundle-root index, so an existing block is carried over as its
/// source text rather than re-serialized: the bundle metadata, its quoting,
/// and its comments all survive.
fn root_frontmatter(bundle_root: &Path, index_path: &Path) -> String {
    if !crate::bundle::leads_outside(bundle_root, index_path)
        && let Ok(text) = fs::read_to_string(index_path)
        && let Some(block) = frontmatter_block(&text)
    {
        return block;
    }
    // No declaration yet. Declaring the current version switches off the
    // fallback that lets older bundles resolve root-relative path fields, so
    // only declare it when nothing in the bundle relies on that fallback.
    let depends_on_fallback =
        Bundle::load(bundle_root).is_ok_and(|bundle| !bundle.legacy_path_fields().is_empty());
    if depends_on_fallback {
        String::new()
    } else {
        format!("---\nokf_version: \"{}\"\n---\n\n", crate::OKF_VERSION)
    }
}

/// The frontmatter block at the start of `text`, exactly as written, from the
/// opening `---` to the closing one, followed by a blank line. `None` when the
/// text has no closed frontmatter block.
fn frontmatter_block(text: &str) -> Option<String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut lines = text.lines();
    if !crate::document::is_delimiter(lines.next()?) {
        return None;
    }
    let mut block = String::from("---\n");
    for line in lines {
        if crate::document::is_delimiter(line) {
            block.push_str("---\n\n");
            return Some(block);
        }
        block.push_str(line);
        block.push('\n');
    }
    None
}

fn depth(root: &Path, dir: &Path) -> usize {
    dir.strip_prefix(root).map_or(0, |r| r.components().count())
}

/// All directories that contain at least one markdown file of the bundle at
/// any depth, including the bundle root (matching the reference
/// `_directories_to_index`).
fn directories_to_index(md_files: &HashSet<PathBuf>, bundle_root: &Path) -> Vec<PathBuf> {
    let mut dirs: std::collections::BTreeSet<PathBuf> = std::collections::BTreeSet::new();
    let root_parent = bundle_root.parent();
    for md in md_files {
        let mut cur = md.parent();
        while let Some(dir) = cur {
            if Some(dir) == root_parent {
                break;
            }
            dirs.insert(dir.to_path_buf());
            if dir == bundle_root {
                break;
            }
            cur = dir.parent();
        }
    }
    dirs.into_iter().collect()
}
