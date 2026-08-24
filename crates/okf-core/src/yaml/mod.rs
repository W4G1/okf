//! A small YAML *subset* parser used for OKF frontmatter.
//!
//! OKF frontmatter is, in practice, a flat-ish YAML mapping of scalars, lists,
//! and occasionally nested mappings (see the [specification][spec]). A
//! full YAML 1.2 engine would be overkill and would pull in dependencies, so
//! this module implements the pragmatic subset that real frontmatter uses:
//!
//! - block mappings (`key: value`), including nested/indented blocks;
//! - block sequences (`- item`);
//! - flow collections (`[a, b]`, `{a: 1, b: 2}`);
//! - plain, single-quoted, and double-quoted scalars;
//! - literal (`|`) and folded (`>`) block scalars;
//! - `#` comments and blank lines;
//! - the core scalar types: null, bool, int, float, string.
//!
//! Plain and quoted scalars may span lines, folding each break into a single
//! space, because `PyYAML`'s `safe_dump` wraps any value past its 80-column line
//! width and the reference implementation publishes bundles that way.
//!
//! It deliberately does **not** support anchors/aliases, explicit tags
//! (`!!str`), multiple documents, or complex (non-scalar) mapping keys. Those
//! never appear in well-formed OKF frontmatter; encountering them yields a
//! clear [`YamlError`] rather than silent misbehaviour.
//!
//! The guarantee that matters for OKF round-tripping is:
//! `parse(emit(parse(x))) == parse(x)`. Emitting and re-parsing preserves the
//! logical value and key order. This mirrors the reference implementation's
//! `OKFDocument` round-trip test.
//!
//! ## Timestamps are strings
//!
//! One deliberate divergence from `PyYAML`: YAML's implicit resolver types a bare
//! `2026-12-31` as a date and a bare `2026-06-30T14:00:00Z` as a datetime, while
//! this module keeps every scalar of either shape as a string. The OKF layer
//! loses nothing, since [`DateField`](crate::DateField) and
//! [`DateTimeField`](crate::DateTimeField) keep the text beside the parsed
//! value, and it means a malformed date can be reported rather than silently
//! dropped.
//!
//! The consequence shows up on the way out. A bare ISO datetime is not stable
//! even under the reference's own round-trip: `PyYAML` loads it into a `datetime`
//! and dumps it back as `2026-06-30 14:00:00+00:00`, losing the `T` and `Z`
//! separators the spec asks for. A quoted one survives byte-identical. So the
//! emitter quotes a datetime-valued string and leaves a bare `YYYY-MM-DD` plain,
//! which is how both the specification and the reference write `stale_after`,
//! `last_modified`, and `usage_window`.
//!
//! [spec]: https://github.com/GoogleCloudPlatform/open-knowledge-format/blob/main/SPEC.md

mod emitter;
mod parser;

use std::fmt;

pub use parser::YamlError;

/// An ordered YAML mapping (preserves insertion / source order, like the
/// reference implementation which dumps with `sort_keys=False`).
///
/// Keys are [`Value`]s for generality, but OKF frontmatter keys are always
/// strings; the [`get`](Mapping::get) / [`insert`](Mapping::insert) helpers
/// operate on string keys for convenience.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mapping {
    entries: Vec<(Value, Value)>,
}

impl Mapping {
    /// Creates an empty mapping.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Number of key/value pairs.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` if the mapping has no entries.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Looks up a value by string key.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.entries
            .iter()
            .find(|(k, _)| k.as_str() == Some(key))
            .map(|(_, v)| v)
    }

    /// Returns `true` if the mapping contains the given string key.
    #[must_use]
    pub fn contains_key(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// Inserts (or, if the string key already exists, replaces) a value,
    /// preserving the position of an existing key. Returns the previous value.
    pub fn insert(&mut self, key: impl Into<String>, value: Value) -> Option<Value> {
        let key = key.into();
        if let Some(slot) = self
            .entries
            .iter_mut()
            .find(|(k, _)| k.as_str() == Some(&key))
        {
            return Some(std::mem::replace(&mut slot.1, value));
        }
        self.entries.push((Value::String(key), value));
        None
    }

    /// Removes a value by string key, preserving order of the rest.
    pub fn remove(&mut self, key: &str) -> Option<Value> {
        let idx = self
            .entries
            .iter()
            .position(|(k, _)| k.as_str() == Some(key))?;
        Some(self.entries.remove(idx).1)
    }

    /// Pushes a raw key/value pair (used by the parser; keeps non-string keys).
    pub(crate) fn push_raw(&mut self, key: Value, value: Value) {
        self.entries.push((key, value));
    }

    /// Iterates over `(key, value)` pairs in order.
    pub fn iter(&self) -> impl Iterator<Item = (&Value, &Value)> {
        self.entries.iter().map(|(k, v)| (k, v))
    }

    /// Iterates over string keys (skipping any non-string keys).
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().filter_map(|(k, _)| k.as_str())
    }
}

/// A parsed YAML value.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// `null`, `~`, or an empty value.
    Null,
    /// `true` / `false`.
    Bool(bool),
    /// An integer scalar.
    Int(i64),
    /// A floating-point scalar.
    Float(f64),
    /// A string scalar.
    String(String),
    /// A sequence (`[...]` or block `- ...`).
    Sequence(Vec<Self>),
    /// A mapping (`{...}` or block `key: value`).
    Mapping(Mapping),
}

impl Value {
    /// Parses a single YAML value from text (the OKF frontmatter subset).
    ///
    /// # Errors
    ///
    /// Returns [`YamlError`] for any input outside the supported subset
    /// (anchors, tags, multiple documents, or syntactically malformed YAML).
    pub fn parse(text: &str) -> Result<Self, YamlError> {
        parser::parse(text)
    }

    /// Emits this value as YAML text using block style, preserving key order.
    #[must_use]
    pub fn to_yaml_string(&self) -> String {
        emitter::emit(self)
    }

    /// Returns the string contents if this is a [`Value::String`].
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s),
            _ => None,
        }
    }

    /// Returns the boolean if this is a [`Value::Bool`].
    #[must_use]
    pub const fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Returns the integer if this is a [`Value::Int`].
    #[must_use]
    pub const fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(i) => Some(*i),
            _ => None,
        }
    }

    /// Returns the sequence elements if this is a [`Value::Sequence`].
    #[must_use]
    pub fn as_sequence(&self) -> Option<&[Self]> {
        match self {
            Self::Sequence(s) => Some(s),
            _ => None,
        }
    }

    /// Returns the mapping if this is a [`Value::Mapping`].
    #[must_use]
    pub const fn as_mapping(&self) -> Option<&Mapping> {
        match self {
            Self::Mapping(m) => Some(m),
            _ => None,
        }
    }

    /// True for `Null`, an empty string, an empty sequence, or an empty
    /// mapping. Mirrors Python's "falsy" check used by the reference
    /// implementation's `validate()` (`not frontmatter.get(k)`).
    #[must_use]
    pub const fn is_empty_value(&self) -> bool {
        match self {
            Self::Null | Self::Bool(false) | Self::Int(0) => true,
            Self::String(s) => s.is_empty(),
            Self::Sequence(s) => s.is_empty(),
            Self::Mapping(m) => m.is_empty(),
            _ => false,
        }
    }

    /// Renders a scalar as a plain display string (used for typed frontmatter
    /// accessors that coerce scalars to text, matching the reference's
    /// `str(fm.get(...))`).
    #[must_use]
    pub fn as_display_string(&self) -> Option<String> {
        match self {
            Self::String(s) => Some(s.clone()),
            Self::Bool(b) => Some(b.to_string()),
            Self::Int(i) => Some(i.to_string()),
            Self::Float(f) => Some(format!("{f}")),
            _ => None,
        }
    }

    /// The borrowing form of [`as_display_string`](Self::as_display_string):
    /// returns a [`std::borrow::Cow`] borrowing the [`String`](Self::String) case and
    /// owning the coerced form for [`Bool`](Self::Bool)/[`Int`](Self::Int)/
    /// [`Float`](Self::Float). `None` for non-scalar variants.
    ///
    /// Frontmatter accessors use this so the common case (a YAML string) is
    /// allocation-free, while the deviation case (e.g. `type: 42`) still
    /// coerces to text the way the reference's `str(fm.get(...))` does, rather
    /// than silently reading as `None`.
    #[must_use]
    pub fn as_display_str(&self) -> Option<std::borrow::Cow<'_, str>> {
        match self {
            Self::String(s) => Some(std::borrow::Cow::Borrowed(s)),
            Self::Bool(b) => Some(std::borrow::Cow::Owned(b.to_string())),
            Self::Int(i) => Some(std::borrow::Cow::Owned(i.to_string())),
            Self::Float(f) => Some(std::borrow::Cow::Owned(format!("{f}"))),
            _ => None,
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_yaml_string())
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Self::String(s.to_string())
    }
}

impl From<String> for Value {
    fn from(s: String) -> Self {
        Self::String(s)
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Self::Bool(b)
    }
}

impl From<i64> for Value {
    fn from(i: i64) -> Self {
        Self::Int(i)
    }
}

impl<T: Into<Self>> From<Vec<T>> for Value {
    fn from(v: Vec<T>) -> Self {
        Self::Sequence(v.into_iter().map(Into::into).collect())
    }
}

impl From<Mapping> for Value {
    fn from(m: Mapping) -> Self {
        Self::Mapping(m)
    }
}
