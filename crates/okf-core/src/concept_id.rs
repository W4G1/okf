//! Concept identifiers and their mapping to/from file paths.
//!
//! A *concept id* is the path of a concept's file within the bundle with the
//! `.md` suffix removed, e.g. `tables/users.md` has id `tables/users`.
//! This module ports the reference `bundle/paths.py`. Its ASCII segment rule is
//! kept as [`is_portable_segment`], a guidance check, rather than as a parse
//! error: see [`validate_segment`]. Ported to Rust and modified from the
//! original Apache-2.0 Python source; see the NOTICE file.

use std::fmt;
use std::path::{Component, Path, PathBuf};

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
    ///
    /// # Errors
    ///
    /// Returns [`ConceptIdError`] if `segments` is empty or any segment fails
    /// [`validate_segment`].
    pub fn new(segments: Vec<String>) -> Result<Self, ConceptIdError> {
        if segments.is_empty() {
            return Err(ConceptIdError(
                "concept_id must have at least one segment".into(),
            ));
        }
        for seg in &segments {
            validate_segment(seg)?;
        }
        Ok(Self { segments })
    }

    /// Parses a concept id from a `/`-separated string. Empty segments are
    /// dropped (so leading/trailing/duplicate slashes are tolerated), matching
    /// the reference `parse_concept_id`.
    ///
    /// # Errors
    ///
    /// Returns [`ConceptIdError`] if `s` resolves to no segments or any segment
    /// fails [`validate_segment`].
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
        Ok(Self { segments })
    }

    /// The id's segments.
    #[must_use]
    pub fn segments(&self) -> &[String] {
        &self.segments
    }

    /// The final segment (the concept's own name, without directories).
    pub fn name(&self) -> &str {
        self.segments.last().map_or("", String::as_str)
    }

    /// The id of the directory that contains this concept, if any.
    #[must_use]
    pub fn parent(&self) -> Option<Self> {
        if self.segments.len() <= 1 {
            None
        } else {
            Some(Self {
                segments: self.segments[..self.segments.len() - 1].to_vec(),
            })
        }
    }

    /// Resolves this id to a file path under `bundle_root` (appending `.md`).
    ///
    /// # Panics
    ///
    /// Never panics in practice: the constructor rejects empty segment lists,
    /// so [`ConceptId::segments`] always has at least one element.
    #[must_use]
    pub fn to_path(&self, bundle_root: &Path) -> PathBuf {
        let mut path = bundle_root.to_path_buf();
        let (name, dirs) = self
            .segments
            .split_last()
            .expect("ConceptId is constructed non-empty");
        for d in dirs {
            path.push(d);
        }
        path.push(format!("{name}.md"));
        path
    }

    /// Derives a concept id from a file path relative to `bundle_root`,
    /// stripping the `.md` suffix.
    ///
    /// The path must be a normalized, UTF-8 `.md` path whose segments can be
    /// represented by a [`ConceptId`]. A file already on disk is a concept
    /// whatever its portable spelling, and conformance is a question of
    /// frontmatter, not filenames, so names such as `my notes.md` remain valid.
    /// Rejecting non-UTF-8 names rather than replacing them is important: a
    /// replacement character would produce an id that does not point back to
    /// the original file.
    ///
    /// # Errors
    ///
    /// Returns [`ConceptIdError`] if `path` is not under `bundle_root` or
    /// resolves to no segments, is not a `.md` file, contains a non-normal path
    /// component, or cannot be represented as UTF-8 without loss.
    pub fn from_path(bundle_root: &Path, path: &Path) -> Result<Self, ConceptIdError> {
        let rel = path
            .strip_prefix(bundle_root)
            .map_err(|_| ConceptIdError(format!("{} is not under bundle root", path.display())))?;
        let mut segments = Vec::new();
        for component in rel.components() {
            let Component::Normal(segment) = component else {
                return Err(ConceptIdError(format!(
                    "{} contains a non-normal path component",
                    path.display()
                )));
            };
            let segment = segment.to_str().ok_or_else(|| {
                ConceptIdError(format!(
                    "{} contains a path segment that is not valid UTF-8",
                    path.display()
                ))
            })?;
            segments.push(segment.to_string());
        }

        let Some(last) = segments.last_mut() else {
            return Err(ConceptIdError(
                "concept_id must have at least one segment".into(),
            ));
        };
        let Some(stripped) = last.strip_suffix(".md") else {
            return Err(ConceptIdError(format!(
                "{} does not name a markdown concept",
                path.display()
            )));
        };
        *last = stripped.to_string();

        // Use the validating constructor so a path-derived id has the same
        // segment invariants as one parsed from a string.
        Self::new(segments)
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
        Self::parse(s)
    }
}

impl TryFrom<&str> for ConceptId {
    type Error = ConceptIdError;
    fn try_from(s: &str) -> Result<Self, Self::Error> {
        Self::parse(s)
    }
}

impl TryFrom<String> for ConceptId {
    type Error = ConceptIdError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::parse(&s)
    }
}

impl From<ConceptId> for String {
    fn from(id: ConceptId) -> Self {
        id.to_string()
    }
}

impl AsRef<[String]> for ConceptId {
    fn as_ref(&self) -> &[String] {
        self.segments()
    }
}

impl std::ops::Deref for ConceptId {
    type Target = [String];
    fn deref(&self) -> &[String] {
        self.segments()
    }
}

/// Validates a single path segment, rejecting only what cannot be a concept id.
///
/// The reference `bundle/paths.py` restricts segments to
/// `[A-Za-z0-9_][A-Za-z0-9_.\-]*`, but that rule is an artifact of the reference
/// implementation rather than a requirement: the specification states no
/// character constraint on filenames, and conformance is a question of
/// frontmatter. [`ConceptId::from_path`] accordingly accepts non-portable
/// UTF-8 names, so applying the ASCII rule here only meant that ids the loader
/// had just produced could not be parsed back, and that links to those
/// concepts vanished from the graph without even being reported as broken.
///
/// What stays rejected is the set that cannot round-trip through the
/// `/`-joined string form or through [`ConceptId::to_path`]: an empty segment,
/// the traversal names `.` and `..`, the path separators `/` and `\`, and
/// control characters. Spaces, emoji, and other Unicode are accepted.
///
/// The ASCII convention is still worth following, so `validate_bundle` reports
/// segments outside it as a warning instead of refusing to parse them.
///
/// # Errors
///
/// Returns [`ConceptIdError`] for the small set of segments that cannot
/// round-trip through the `/`-joined string form or [`ConceptId::to_path`]:
/// empty, `.` and `..`, path separators, and control characters.
pub fn validate_segment(seg: &str) -> Result<(), ConceptIdError> {
    let reject = |reason: &str| {
        Err(ConceptIdError(format!(
            "Invalid concept id segment: {seg:?} ({reason})"
        )))
    };
    if seg.is_empty() {
        return reject("empty");
    }
    if seg == "." || seg == ".." {
        return reject("`.` and `..` cannot name a concept");
    }
    for c in seg.chars() {
        // `\` is a separator on Windows, so allowing it would let one segment
        // silently become two in `to_path`.
        if c == '/' || c == '\\' {
            return reject("contains a path separator");
        }
        if c.is_control() {
            return reject("contains a control character");
        }
    }
    Ok(())
}

/// Whether a segment is within the reference implementation's
/// `[A-Za-z0-9_][A-Za-z0-9_.\-]*` convention.
///
/// [`validate_segment`] no longer enforces this, because the spec does not, but
/// a name outside it needs `<...>` or percent-encoding to be linked from
/// markdown and is not guaranteed to survive every filesystem unchanged. It is
/// reported as guidance, never as an error.
#[must_use]
pub fn is_portable_segment(seg: &str) -> bool {
    let mut chars = seg.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphanumeric() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-')
}
