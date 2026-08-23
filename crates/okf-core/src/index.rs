//! Generation of `index.md` directory listings.
//!
//! Index files support **progressive disclosure**: they let a human or agent
//! see what a directory holds before opening individual documents. Grouping is
//! by concept `type`, which is also how an `Attested Computation` becomes
//! discoverable from an index.
//!
//! This is a port of the reference `bundle/index.py`'s `regenerate_indexes` and
//! `_build_index_text`. The reference synthesizes subdirectory descriptions
//! with an LLM; since OKF tooling must not require any particular model or
//! network access, the description synthesizer here is a pluggable closure with
//! a deterministic, dependency-free default ([`default_synthesize`]). Ported to
//! Rust and modified from the original Apache-2.0 Python source; see the NOTICE
//! file.

use crate::document::Document;
use crate::yaml::Value;
use std::collections::{BTreeMap, HashMap};
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
/// descriptions computed for its children. Empty directories are skipped.
/// Returns the paths of the index files written.
///
/// # Errors
///
/// Returns the underlying [`io::Error`] from any directory walk or file write.
pub fn regenerate_indexes_with(
    bundle_root: impl AsRef<Path>,
    synthesize: &Synthesize,
) -> io::Result<Vec<PathBuf>> {
    let bundle_root = bundle_root.as_ref();
    let mut written = Vec::new();
    if !bundle_root.exists() {
        return Ok(written);
    }

    let mut directories = directories_to_index(bundle_root)?;
    // Deepest-first; ties broken by path for determinism.
    directories.sort_by(|a, b| {
        let da = depth(bundle_root, a);
        let db = depth(bundle_root, b);
        db.cmp(&da).then_with(|| a.cmp(b))
    });

    let mut dir_descriptions: HashMap<PathBuf, String> = HashMap::new();

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
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            if crate::bundle::RESERVED_FILENAMES.contains(&name.as_str()) {
                continue;
            }
            if child.is_file() && child.extension().is_some_and(|e| e == "md") {
                let Some(doc) = load_doc(&child) else {
                    continue;
                };
                let stem = child
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                // An empty `title` falls back to the filename, as the spec permits
                // and the reference's `fm.get("title") or child.stem` does.
                let title = doc
                    .frontmatter
                    .title()
                    .filter(|t| !t.is_empty())
                    .map_or(stem, std::borrow::Cow::into_owned);
                let description = doc
                    .frontmatter
                    .description()
                    .map(std::borrow::Cow::into_owned)
                    .unwrap_or_default();
                let type_ = doc
                    .frontmatter
                    .type_()
                    .map(std::borrow::Cow::into_owned)
                    .unwrap_or_default();
                entries.push(IndexEntry {
                    type_,
                    title,
                    link: encoded_component(child.file_name().unwrap_or_default()),
                    description,
                });
            } else if child.is_dir() {
                let description = dir_descriptions.get(&child).cloned().unwrap_or_default();
                let encoded_name = encoded_component(child.file_name().unwrap_or_default());
                entries.push(IndexEntry {
                    type_: "Subdirectories".to_string(),
                    title: name.clone(),
                    link: format!("{encoded_name}/{INDEX_FILE}"),
                    description,
                });
            }
        }

        if entries.is_empty() {
            continue;
        }

        written.push(write_index(directory, bundle_root, &entries)?);

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

    Ok(written)
}

fn load_doc(path: &Path) -> Option<Document> {
    let text = fs::read_to_string(path).ok()?;
    Document::parse(&text).ok()
}

fn write_index(
    directory: &Path,
    bundle_root: &Path,
    entries: &[IndexEntry],
) -> io::Result<PathBuf> {
    let index_path = directory.join(INDEX_FILE);
    let body = build_index_text_with_encoded_links(entries);
    let text = if directory == bundle_root {
        match preserved_frontmatter(&index_path) {
            Some(fm) => format!("---\n{fm}---\n\n{body}"),
            None => body,
        }
    } else {
        body
    };
    fs::write(&index_path, text)?;
    Ok(index_path)
}

/// The `okf_version` declaration to carry over when rewriting an `index.md`.
///
/// A bundle-root `index.md` is the one place frontmatter is permitted in an
/// index, and the only key it may hold is `okf_version`. Regenerating the
/// listing must not silently drop the bundle's declared version, so the key is
/// read back and re-emitted; anything else in the block is discarded, since it
/// does not belong there.
fn preserved_frontmatter(index_path: &Path) -> Option<String> {
    let doc = load_doc(index_path)?;
    let version = doc.frontmatter.get("okf_version")?;
    let mut kept = crate::yaml::Mapping::new();
    kept.insert("okf_version", version.clone());
    Some(Value::Mapping(kept).to_yaml_string())
}

fn depth(root: &Path, dir: &Path) -> usize {
    dir.strip_prefix(root).map_or(0, |r| r.components().count())
}

/// All directories that contain at least one `.md` file at any depth, including
/// the bundle root (matching the reference `_directories_to_index`).
fn directories_to_index(bundle_root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut md_files = Vec::new();
    collect_markdown(bundle_root, &mut md_files)?;

    let mut dirs: std::collections::BTreeSet<PathBuf> = std::collections::BTreeSet::new();
    let root_parent = bundle_root.parent();
    for md in &md_files {
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
    Ok(dirs.into_iter().collect())
}

fn collect_markdown(dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            collect_markdown(&path, out)?;
        } else if path.extension().is_some_and(|e| e == "md") {
            out.push(path);
        }
    }
    Ok(())
}
