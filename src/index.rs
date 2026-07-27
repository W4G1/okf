//! Generation of `index.md` directory listings (§8).
//!
//! Index files support **progressive disclosure**: they let a human or agent
//! see what a directory holds before opening individual documents. Grouping is
//! by concept `type`, which is also how an `Attested Computation` becomes
//! discoverable from an index (§10.5).
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
        items.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()));
        let mut lines = vec![format!("# {typ}"), String::new()];
        for (title, link, desc) in items {
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
            if name == INDEX_FILE {
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
                // An empty `title` falls back to the filename, as §4.1 permits
                // and the reference's `fm.get("title") or child.stem` does.
                let title = doc
                    .frontmatter
                    .title()
                    .filter(|t| !t.is_empty())
                    .unwrap_or(stem);
                let description = doc.frontmatter.description().unwrap_or_default();
                let type_ = doc.frontmatter.type_().unwrap_or_default();
                entries.push(IndexEntry {
                    type_,
                    title,
                    link: name,
                    description,
                });
            } else if child.is_dir() {
                let description = dir_descriptions.get(&child).cloned().unwrap_or_default();
                entries.push(IndexEntry {
                    type_: "Subdirectories".to_string(),
                    title: name.clone(),
                    link: format!("{name}/{INDEX_FILE}"),
                    description,
                });
            }
        }

        if entries.is_empty() {
            continue;
        }

        let index_path = directory.join(INDEX_FILE);
        let body = build_index_text(&entries);
        let text = match preserved_frontmatter(&index_path) {
            Some(fm) => format!("---\n{fm}---\n\n{body}"),
            None => body,
        };
        fs::write(&index_path, text)?;
        written.push(index_path);

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

/// The `okf_version` declaration to carry over when rewriting an `index.md`.
///
/// A bundle-root `index.md` is the one place frontmatter is permitted in an
/// index, and the only key it may hold is `okf_version` (§12). Regenerating the
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
    dir.strip_prefix(root)
        .map(|r| r.components().count())
        .unwrap_or(0)
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
