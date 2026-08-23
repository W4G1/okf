//! Typed, order-preserving access to a concept's YAML frontmatter.
//!
//! OKF frontmatter is an open mapping: a few well-known keys (§4.1 of the
//! [spec]) plus arbitrary producer-defined extensions that consumers MUST
//! preserve when round-tripping. [`Frontmatter`] therefore stores the full
//! [`Mapping`] verbatim and layers typed accessors on top, rather than
//! deserializing into a fixed struct that would drop unknown keys.
//!
//! v0.2 adds four families of well-known keys on top of the v0.1 core, all of
//! them optional:
//!
//! | Family                    | Keys                                                     | Section |
//! |---------------------------|----------------------------------------------------------|---------|
//! | Core                      | `type`, `title`, `description`, `resource`, `tags`         | §4.1    |
//! | Provenance                | `sources`, `usage_window`                                  | §5.1    |
//! | Trust                     | `generated`, `verified`                                    | §5.2    |
//! | Lifecycle                 | `status`, `stale_after`                                    | §5.4/5  |
//! | Computation               | `runtime`, `parameters`, `computation`, `executor`, `attester` | §10.2 |
//!
//! Absence is meaningful but never fatal: [`Frontmatter::status`] defaults to
//! `stable`, [`Frontmatter::trust_tier`] to `unverified`, and a concept
//! carrying nothing but `type` is fully conformant (§11).
//!
//! [spec]: https://github.com/GoogleCloudPlatform/open-knowledge-format/blob/main/SPEC.md

use crate::computation::{Attester, Executor, Parameter, ATTESTED_COMPUTATION_TYPE};
use crate::date::{Date, DateField, DateTimeField};
use crate::provenance::{Source, UsageWindow};
use crate::trust::{self, Generated, Status, TrustTier, Verification};
use crate::yaml::{Mapping, Value};
use std::borrow::Cow;

/// The only frontmatter key OKF always requires (§4.1): a concept carrying
/// nothing but `type` is fully conformant (§11).
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
/// `title` and `description` are §4.1 recommendations; `generated` is §5.2's
/// record of how the content was produced. §4.1 also recommends `resource` and
/// `tags`, which are deliberately left out here: `resource` is "absent for
/// concepts that describe abstract ideas rather than physical resources", so
/// flagging either would be noise rather than guidance.
///
/// Leaving any of these unset is never a conformance failure (§11).
pub const RECOMMENDED_FRONTMATTER_KEYS: [&str; 3] = ["title", "description", "generated"];

/// Keys v0.2 retired but consumers may still encounter in v0.1 documents
/// (§13.1). `timestamp` is superseded by `generated.at`.
pub const LEGACY_FRONTMATTER_KEYS: [&str; 1] = ["timestamp"];

/// Every frontmatter key the specification gives a meaning to, across all
/// families. Anything else is a producer extension (§4.1).
pub const KNOWN_FRONTMATTER_KEYS: [&str; 17] = [
    // Core (§4.1).
    "type",
    "title",
    "description",
    "resource",
    "tags",
    // Provenance (§5.1).
    "sources",
    "usage_window",
    // Trust (§5.2).
    "generated",
    "verified",
    // Lifecycle (§5.4, §5.5).
    "status",
    "stale_after",
    // Attested Computation (§10.2).
    "runtime",
    "parameters",
    "computation",
    "executor",
    "attester",
    // Legacy (§13.1).
    "timestamp",
];

/// The key order the reference implementation writes documents in (its
/// `_PREFERRED_KEY_ORDER`): identity first, then lifecycle, trust, and
/// provenance.
///
/// Presentational only. §4.1 gives frontmatter no required key order, and a
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

    /// The **required** `type` field (§4.1). `None` if absent or not a scalar.
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
    /// A bare `{ by, at }` mapping is returned as a one-element list, as §5.2
    /// requires.
    pub fn verified(&self) -> Vec<Verification> {
        self.map
            .get("verified")
            .map(Verification::list_from_value)
            .unwrap_or_default()
    }

    /// The verification with the latest parseable `at` (§5.2).
    #[must_use]
    pub fn latest_verification(&self) -> Option<Verification> {
        let events = self.verified();
        trust::latest_verification(&events).cloned()
    }

    /// The trust tier derived from `verified` (§5.3).
    #[must_use]
    pub fn trust_tier(&self) -> TrustTier {
        TrustTier::derive(&self.verified())
    }

    /// When the content last meaningfully changed: `generated.at` (§5.2),
    /// falling back to a legacy v0.1 `timestamp` when `generated` is absent, as
    /// §13.1 permits.
    #[must_use]
    pub fn content_changed_at(&self) -> Option<DateTimeField> {
        self.generated()
            .and_then(|g| g.at)
            .or_else(|| self.timestamp().map(|s| DateTimeField::new(s.into_owned())))
    }

    /// The legacy v0.1 `timestamp` field, superseded by `generated.at` (§13.1).
    ///
    /// Prefer [`Frontmatter::content_changed_at`], which reads `generated.at`
    /// first and falls back to this.
    #[must_use]
    pub fn timestamp(&self) -> Option<Cow<'_, str>> {
        self.display_str("timestamp")
    }

    /// The lifecycle `status`. An absent key is [`Status::Stable`] (§5.4).
    #[must_use]
    pub fn status(&self) -> Status {
        Status::parse(self.display_str("status").as_deref())
    }

    /// The `stale_after` date, on and after which the content is stale (§5.5).
    #[must_use]
    pub fn stale_after(&self) -> Option<DateField> {
        self.display_str("stale_after")
            .map(|s| DateField::new(s.into_owned()))
    }

    /// Whether the concept is stale on `today`: `today >= stale_after` (§5.5).
    /// A concept with no (or an unreadable) `stale_after` is never stale.
    ///
    /// A datetime-valued `stale_after` is compared on its date part, as
    /// [`DateField::effective_date`] explains.
    #[must_use]
    pub fn is_stale_on(&self, today: Date) -> bool {
        let stale_after = self.stale_after().and_then(|d| d.effective_date());
        trust::is_stale_on(stale_after, today)
    }

    /// `true` when `type` is `Attested Computation` (§10.1).
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
    /// a body block (§10.3).
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

    /// The path-valued frontmatter fields present (§6.2), as
    /// `(field name, raw value)`.
    ///
    /// `sources[].resource` is deliberately excluded: it may be a scope
    /// descriptor rather than a path (§5.1). Use
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
    /// producer-defined extension keys consumers must preserve (§4.1).
    #[must_use]
    pub fn extension_keys(&self) -> Vec<&str> {
        self.map
            .keys()
            .filter(|k| !KNOWN_FRONTMATTER_KEYS.contains(k))
            .collect()
    }

    /// Returns the legacy v0.1 keys present that v0.2 supersedes (§13.1).
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
