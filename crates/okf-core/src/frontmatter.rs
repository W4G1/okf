//! Typed, order-preserving access to a concept's YAML frontmatter.
//!
//! OKF frontmatter is an open mapping: a few well-known keys (defined by the
//! [spec]) plus arbitrary producer-defined extensions that consumers MUST
//! preserve when round-tripping. [`Frontmatter`] therefore stores the full
//! [`Mapping`] verbatim and layers typed accessors on top, rather than
//! deserializing into a fixed struct that would drop unknown keys.
//!
//! v0.2 adds four families of well-known keys on top of the v0.1 core, all of
//! them optional:
//!
//! | Family                    | Keys                                                     |
//! |---------------------------|----------------------------------------------------------|
//! | Core                      | `type`, `title`, `description`, `resource`, `tags`         |
//! | Provenance                | `sources`, `usage_window`                                  |
//! | Trust                     | `generated`, `verified`                                    |
//! | Lifecycle                 | `status`, `stale_after`                                    |
//! | Computation               | `runtime`, `parameters`, `computation`, `executor`, `attester` |
//!
//! Absence is meaningful but never fatal: [`Frontmatter::status`] defaults to
//! `stable`, [`Frontmatter::trust_tier`] to `unverified`, and a concept
//! carrying nothing but `type` is fully conformant.
//!
//! [spec]: https://github.com/GoogleCloudPlatform/open-knowledge-format/blob/main/SPEC.md

use crate::computation::{ATTESTED_COMPUTATION_TYPE, Attester, Executor, Parameter};
use crate::date::{Date, DateTime, DateTimeField};
use crate::provenance::{Source, UsageWindow};
use crate::trust::{self, Generated, Status, TrustTier, Verification};
use crate::yaml::{Mapping, Value};
use std::borrow::Cow;

/// The only frontmatter key OKF always requires: a concept carrying
/// nothing but `type` is fully conformant.
///
/// This is what [`Document::validate`](crate::Document::validate) enforces, and
/// it matches the reference implementation's `REQUIRED_FRONTMATTER_KEYS`. v0.1
/// required four keys; v0.2 narrowed the requirement to this one and demoted
/// the rest to recommendations ([`RECOMMENDED_FRONTMATTER_KEYS`]).
pub const REQUIRED_FRONTMATTER_KEYS: [&str; 1] = ["type"];

/// Keys a producer should fill in before publishing, in the order
/// [`Document::missing_recommended`](crate::Document::missing_recommended)
/// reports them.
///
/// `title` and `description` are recommended fields; `generated` is the
/// record of how the content was produced. The spec also recommends `resource` and
/// `tags`, which are deliberately left out here: `resource` is "absent for
/// concepts that describe abstract ideas rather than physical resources", so
/// flagging either would be noise rather than guidance.
///
/// Leaving any of these unset is never a conformance failure.
pub const RECOMMENDED_FRONTMATTER_KEYS: [&str; 3] = ["title", "description", "generated"];

/// Keys v0.2 retired but consumers may still encounter in v0.1 documents.
/// `timestamp` is superseded by `generated.at`.
pub const LEGACY_FRONTMATTER_KEYS: [&str; 1] = ["timestamp"];

/// Every frontmatter key the specification gives a meaning to, across all
/// families. Anything else is a producer extension.
pub const KNOWN_FRONTMATTER_KEYS: [&str; 17] = [
    // Core.
    "type",
    "title",
    "description",
    "resource",
    "tags",
    // Provenance.
    "sources",
    "usage_window",
    // Trust.
    "generated",
    "verified",
    // Lifecycle.
    "status",
    "stale_after",
    // Attested Computation.
    "runtime",
    "parameters",
    "computation",
    "executor",
    "attester",
    // Legacy.
    "timestamp",
];

/// The key order the reference implementation writes documents in (its
/// `_PREFERRED_KEY_ORDER`): identity first, then lifecycle, trust, and
/// provenance.
///
/// Presentational only. Frontmatter has no required key order, and a
/// consumer must not depend on one; see [`Frontmatter::reorder_preferred`].
pub const PREFERRED_KEY_ORDER: [&str; 11] = [
    "type",
    "resource",
    "title",
    "description",
    "tags",
    "status",
    "generated",
    "verified",
    "stale_after",
    "sources",
    "usage_window",
];

/// A concept's frontmatter: an ordered key/value mapping with typed accessors
/// for the well-known OKF fields.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Frontmatter {
    map: Mapping,
}

impl Frontmatter {
    /// Creates an empty frontmatter block.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            map: Mapping::new(),
        }
    }

    /// Wraps an existing mapping.
    #[must_use]
    pub const fn from_mapping(map: Mapping) -> Self {
        Self { map }
    }

    /// Borrows the underlying ordered mapping.
    #[must_use]
    pub const fn as_mapping(&self) -> &Mapping {
        &self.map
    }

    /// Mutably borrows the underlying ordered mapping.
    pub const fn as_mapping_mut(&mut self) -> &mut Mapping {
        &mut self.map
    }

    /// Consumes the wrapper, returning the underlying mapping.
    #[must_use]
    pub fn into_mapping(self) -> Mapping {
        self.map
    }

    /// `true` if there are no keys.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Raw value for an arbitrary key (including producer extensions).
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.map.get(key)
    }

    /// Sets a raw value for a key, preserving position if it already exists.
    pub fn set(&mut self, key: impl Into<String>, value: Value) {
        self.map.insert(key, value);
    }

    /// Removes a key from frontmatter, returning the removed value if present.
    pub fn remove(&mut self, key: &str) -> Option<Value> {
        self.map.remove(key)
    }

    /// Reorders the keys into [`PREFERRED_KEY_ORDER`], leaving every other key
    /// after them in its current relative order.
    ///
    /// A port of the reference implementation's `_reorder_frontmatter`, which it
    /// applies whenever it writes a concept document; call this before
    /// [`Document::serialize`](crate::Document::serialize) to produce
    /// frontmatter laid out the same way. No key is added, dropped, or
    /// rewritten, so only the serialized order changes.
    pub fn reorder_preferred(&mut self) {
        let mut ordered = Mapping::new();
        for key in PREFERRED_KEY_ORDER {
            if let Some(value) = self.map.get(key) {
                ordered.insert(key, value.clone());
            }
        }
        for (key, value) in self.map.iter() {
            let already_placed = key
                .as_str()
                .is_some_and(|k| PREFERRED_KEY_ORDER.contains(&k));
            if !already_placed {
                ordered.push_raw(key.clone(), value.clone());
            }
        }
        self.map = ordered;
    }

    /// The **required** `type` field. `None` if absent or not a scalar.
    ///
    /// Non-string scalars (`type: 42`) are coerced to their display form, the
    /// way the reference's `str(fm.get("type"))` does, rather than read as
    /// `None`: the spec calls `type` "a short string", so a non-string value
    /// is a producer deviation, but a consumer still gets *something* to
    /// route on rather than treating the concept as typeless.
    #[must_use]
    pub fn type_(&self) -> Option<Cow<'_, str>> {
        self.display_str("type")
    }

    /// The optional `title` field.
    #[must_use]
    pub fn title(&self) -> Option<Cow<'_, str>> {
        self.display_str("title")
    }

    /// The optional one-line `description`.
    #[must_use]
    pub fn description(&self) -> Option<Cow<'_, str>> {
        self.display_str("description")
    }

    /// The optional `resource` URI for the underlying asset.
    #[must_use]
    pub fn resource(&self) -> Option<Cow<'_, str>> {
        self.display_str("resource")
    }

    /// The optional `tags` list. Non-string elements are coerced to their
    /// display form; a non-sequence `tags` value yields an empty vector.
    pub fn tags(&self) -> Vec<String> {
        match self.map.get("tags") {
            Some(Value::Sequence(items)) => {
                items.iter().filter_map(Value::as_display_string).collect()
            }
            _ => Vec::new(),
        }
    }

    /// The `sources` entries: the materials this concept derives from.
    pub fn sources(&self) -> Vec<Source> {
        self.map
            .get("sources")
            .map(Source::list_from_value)
            .unwrap_or_default()
    }

    /// The shared `usage_window` that frames every `sources[].usage_count`.
    pub fn usage_window(&self) -> Option<UsageWindow> {
        self.map
            .get("usage_window")
            .and_then(UsageWindow::from_value)
    }

    /// The `generated` block: how the current content was produced.
    pub fn generated(&self) -> Option<Generated> {
        self.map.get("generated").and_then(Generated::from_value)
    }

    /// The `verified` events: who or what has confirmed this content.
    ///
    /// A bare `{ by, at }` mapping is returned as a one-element list, as the
    /// spec requires.
    pub fn verified(&self) -> Vec<Verification> {
        self.map
            .get("verified")
            .map(Verification::list_from_value)
            .unwrap_or_default()
    }

    /// The verification with the latest parseable `at`.
    #[must_use]
    pub fn latest_verification(&self) -> Option<Verification> {
        let events = self.verified();
        trust::latest_verification(&events).cloned()
    }

    /// The trust tier derived from `verified`.
    #[must_use]
    pub fn trust_tier(&self) -> TrustTier {
        TrustTier::derive(&self.verified())
    }

    /// When the content last meaningfully changed: `generated.at`,
    /// falling back to a legacy v0.1 `timestamp` when `generated` is absent, as
    /// permitted.
    #[must_use]
    pub fn content_changed_at(&self) -> Option<DateTimeField> {
        self.generated()
            .and_then(|g| g.at)
            .or_else(|| self.timestamp().map(|s| DateTimeField::new(s.into_owned())))
    }

    /// The legacy v0.1 `timestamp` field, superseded by `generated.at`.
    ///
    /// Prefer [`Frontmatter::content_changed_at`], which reads `generated.at`
    /// first and falls back to this.
    #[must_use]
    pub fn timestamp(&self) -> Option<Cow<'_, str>> {
        self.display_str("timestamp")
    }

    /// The lifecycle `status`. An absent key is [`Status::Stable`].
    #[must_use]
    pub fn status(&self) -> Status {
        Status::parse(self.display_str("status").as_deref())
    }

    /// The `stale_after` timestamp, on and after which the content is stale.
    #[must_use]
    pub fn stale_after(&self) -> Option<DateTimeField> {
        self.display_str("stale_after")
            .map(|s| DateTimeField::new(s.into_owned()))
    }

    /// Whether the concept is stale at `now`: `now >= stale_after`.
    /// A concept with no (or an unreadable / offset-less) `stale_after` is never stale.
    #[must_use]
    pub fn is_stale_at(&self, now: DateTime) -> bool {
        let Some(stale_after) = self.stale_after() else {
            return false;
        };
        if !stale_after.is_valid() {
            return false;
        }
        trust::is_stale_at(stale_after.datetime, now)
    }

    /// Whether the concept is stale on `today`: `today >= stale_after`.
    /// Evaluates staleness at midnight UTC on `today`.
    #[must_use]
    pub fn is_stale_on(&self, today: Date) -> bool {
        self.is_stale_at(today.to_utc_datetime())
    }

    /// `true` when `type` is `Attested Computation`.
    #[must_use]
    pub fn is_attested_computation(&self) -> bool {
        self.type_().as_deref() == Some(ATTESTED_COMPUTATION_TYPE)
    }

    /// The `runtime`: how to run the computation, and so what `parameters`
    /// mean. REQUIRED on an Attested Computation concept.
    #[must_use]
    pub fn runtime(&self) -> Option<Cow<'_, str>> {
        self.display_str("runtime")
    }

    /// The declared `parameters` an agent may fill.
    pub fn parameters(&self) -> Vec<Parameter> {
        self.map
            .get("parameters")
            .map(Parameter::list_from_value)
            .unwrap_or_default()
    }

    /// The `computation` path, when the computation lives in a file rather than
    /// a body block.
    #[must_use]
    pub fn computation(&self) -> Option<Cow<'_, str>> {
        self.display_str("computation")
    }

    /// The `executor`: how the computation is run, and what a receipt carries.
    pub fn executor(&self) -> Option<Executor> {
        self.map.get("executor").and_then(Executor::from_value)
    }

    /// The `attester`: deterministic code that turns a receipt into a verdict.
    pub fn attester(&self) -> Option<Attester> {
        self.map.get("attester").and_then(Attester::from_value)
    }

    /// The path-valued frontmatter fields present, as
    /// `(field name, raw value)`.
    ///
    /// `sources[].resource` is deliberately excluded: it may be a scope
    /// descriptor rather than a path. Use
    /// [`Source::resource_kind`](crate::provenance::Source::resource_kind) to
    /// filter those yourself.
    #[must_use]
    pub fn path_fields(&self) -> Vec<(&'static str, String)> {
        let mut out = Vec::new();
        let mut push = |name: &'static str, value: Option<String>| {
            if let Some(v) = value.filter(|v| !v.trim().is_empty()) {
                out.push((name, v));
            }
        };
        push(
            "resource",
            self.resource().map(std::borrow::Cow::into_owned),
        );
        push(
            "computation",
            self.computation().map(std::borrow::Cow::into_owned),
        );
        push(
            "executor.resource",
            self.executor().and_then(|e| e.resource),
        );
        push(
            "attester.resource",
            self.attester().and_then(|a| a.resource),
        );
        out
    }

    /// Returns the keys present that are not well-known OKF fields, i.e. the
    /// producer-defined extension keys consumers must preserve.
    #[must_use]
    pub fn extension_keys(&self) -> Vec<&str> {
        self.map
            .keys()
            .filter(|k| !KNOWN_FRONTMATTER_KEYS.contains(k))
            .collect()
    }

    /// Returns the legacy v0.1 keys present that v0.2 supersedes.
    #[must_use]
    pub fn legacy_keys(&self) -> Vec<&str> {
        self.map
            .keys()
            .filter(|k| LEGACY_FRONTMATTER_KEYS.contains(k))
            .collect()
    }

    /// Borrows the scalar at `key` as a display string, coercing non-string
    /// scalars (a `type: 42` deviation yields `Some("42")`) the way the
    /// reference's `str(fm.get(...))` does. Returns `None` for absent keys
    /// and non-scalar values. The common YAML-string case borrows without
    /// allocation; only the coerced case owns.
    fn display_str(&self, key: &str) -> Option<Cow<'_, str>> {
        self.map.get(key).and_then(Value::as_display_str)
    }
}

impl From<Mapping> for Frontmatter {
    fn from(map: Mapping) -> Self {
        Self { map }
    }
}
