//! Concept identifiers and their mapping to/from file paths.
//!
//! A *concept id* is the path of a concept's file within the bundle with the
//! `.md` suffix removed, e.g. `tables/users.md` has id `tables/users` (§2).
//! This module ports the reference `bundle/paths.py`, including its segment
//! validation rule. Ported to Rust and modified from the original Apache-2.0
//! Python source; see the NOTICE file.

use std::fmt;
use std::path::{Path, PathBuf};

/// Error returned when a concept-id segment is malformed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConceptIdError(pub String);

impl fmt::Display for ConceptIdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ConceptIdError {}

/// A concept identifier: an ordered list of path segments (e.g.
/// `["tables", "users"]` for `tables/users`).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConceptId {
    segments: Vec<String>,
}

impl ConceptId {
    /// Builds a concept id from segments, validating each.
    pub fn new(segments: Vec<String>) -> Result<Self, ConceptIdError> {
        if segments.is_empty() {
            return Err(ConceptIdError(
                "concept_id must have at least one segment".into(),
            ));
        }
        for seg in &segments {
            validate_segment(seg)?;
        }
        Ok(ConceptId { segments })
    }

    /// Parses a concept id from a `/`-separated string. Empty segments are
    /// dropped (so leading/trailing/duplicate slashes are tolerated), matching
    /// the reference `parse_concept_id`.
    pub fn parse(s: &str) -> Result<Self, ConceptIdError> {
        let segments: Vec<String> = s
            .split('/')
            .filter(|p| !p.is_empty())
            .map(String::from)
            .collect();
        if segments.is_empty() {
            return Err(ConceptIdError(format!("Empty concept id: {s:?}")));
        }
        for seg in &segments {
            validate_segment(seg)?;
        }
        Ok(ConceptId { segments })
    }

    /// The id's segments.
    pub fn segments(&self) -> &[String] {
        &self.segments
    }

    /// The final segment (the concept's own name, without directories).
    pub fn name(&self) -> &str {
        self.segments.last().map(String::as_str).unwrap_or("")
    }

    /// The id of the directory that contains this concept, if any.
    pub fn parent(&self) -> Option<ConceptId> {
        if self.segments.len() <= 1 {
            None
        } else {
            Some(ConceptId {
                segments: self.segments[..self.segments.len() - 1].to_vec(),
            })
        }
    }

    /// Resolves this id to a file path under `bundle_root` (appending `.md`).
    pub fn to_path(&self, bundle_root: &Path) -> PathBuf {
        let mut path = bundle_root.to_path_buf();
        let (name, dirs) = self.segments.split_last().expect("non-empty");
        for d in dirs {
            path.push(d);
        }
        path.push(format!("{name}.md"));
        path
    }

    /// Derives a concept id from a file path relative to `bundle_root`,
    /// stripping the `.md` suffix.
    ///
    /// Segments are **not** validated here, matching the reference
    /// `path_to_concept_id` (only `parse_concept_id` validates). A file already
    /// on disk is a concept whatever it is called, and §11 makes conformance a
    /// question of frontmatter, not filenames, so a name such as
    /// `my notes.md` must not turn a readable document into an error. The
    /// consequence is that [`ConceptId::parse`] may reject the string form of an
    /// id that this returns.
    pub fn from_path(bundle_root: &Path, path: &Path) -> Result<Self, ConceptIdError> {
        let rel = path
            .strip_prefix(bundle_root)
            .map_err(|_| ConceptIdError(format!("{} is not under bundle root", path.display())))?;
        let mut segments: Vec<String> = rel
            .components()
            .map(|comp| comp.as_os_str().to_string_lossy().to_string())
            .collect();
        if let Some(last) = segments.last_mut() {
            if let Some(stripped) = last.strip_suffix(".md") {
                *last = stripped.to_string();
            }
        }
        if segments.is_empty() {
            return Err(ConceptIdError(
                "concept_id must have at least one segment".into(),
            ));
        }
        Ok(ConceptId { segments })
    }
}

impl fmt::Display for ConceptId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.segments.join("/"))
    }
}

impl std::str::FromStr for ConceptId {
    type Err = ConceptIdError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        ConceptId::parse(s)
    }
}

/// Validates a single path segment against the reference rule
/// `[A-Za-z0-9_][A-Za-z0-9_.\-]*`.
pub fn validate_segment(seg: &str) -> Result<(), ConceptIdError> {
    let mut chars = seg.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphanumeric() || c == '_' => {}
        _ => {
            return Err(ConceptIdError(format!(
                "Invalid concept id segment: {seg:?}"
            )))
        }
    }
    for c in chars {
        if !(c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-') {
            return Err(ConceptIdError(format!(
                "Invalid concept id segment: {seg:?}"
            )));
        }
    }
    Ok(())
}
