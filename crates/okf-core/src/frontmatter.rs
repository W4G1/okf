//! Typed, order-preserving access to a concept's YAML frontmatter.
//!
//! OKF frontmatter is an open mapping: a few well-known keys (defined by the
//! [spec]) plus arbitrary producer-defined extensions that consumers MUST
//! preserve when round-tripping. [`Frontmatter`] therefore stores the full
//! [`Mapping`] verbatim and layers typed accessors on top, rather than
//! deserializing into a fixed struct that would drop unknown keys.
//!
//! | Family                    | Keys                                                           |
//! |---------------------------|----------------------------------------------------------------|
//! | Core                      | `type`, `title`, `description`, `resource`, `tags`             |
//! | Provenance                | `sources`, `usage_window`                                      |
//! | Trust                     | `generated`, `verified`                                        |
//! | Lifecycle                 | `status`, `superseded_by`, `stale_after`                       |
//! | Computation               | `runtime`, `parameters`, `computation`, `executor`, `attester` |
//!
//! Absence is meaningful but never fatal: [`Frontmatter::status`] defaults to
//! `stable`, [`Frontmatter::trust_tier`] to `unverified`, and a concept
//! carrying nothing but `type` is fully conformant.
//!
//! ## Reading values that are absent, empty, or oddly shaped
//!
//! The accessors apply the specification's consumer rules, so callers do not
//! have to:
//!
//! - A null, an empty string, and an empty list are the same as an absent key.
//! - Where a field is a list (`tags`, `sources`, `verified`, `parameters`),
//!   one item is read as a list with one item. A scalar `tags` value is one
//!   tag and is never split at commas.
//! - An entry of a family that lacks its REQUIRED key is ignored: a `sources`
//!   entry without `resource`, a `verified` event without both `by` and `at`,
//!   a `parameters` entry without `name` and `type`. The validator reports
//!   them; read the raw [`Value`] with [`Frontmatter::get`] to see them.
//!
//! ## Rewriting without re-serializing
//!
//! A producer that modifies a file should not re-serialize the values it did
//! not change. A frontmatter block that was parsed from text remembers how
//! each top-level entry was written, and [`Frontmatter::to_yaml_preserving`]
//! writes untouched entries back byte for byte: quoting, flow or block style,
//! and comments all survive. Only entries whose value actually changed are
//! emitted afresh.
//!
//! [spec]: https://github.com/open-knowledge-format/open-knowledge-format/blob/main/SPEC.md

use crate::computation::{ATTESTED_COMPUTATION_TYPE, Attester, Executor, Parameter};
use crate::date::{Date, DateTime, DateTimeField};
use crate::error::DocumentError;
use crate::provenance::{Source, UsageWindow};
use crate::trust::{self, Freshness, Generated, Status, TrustTier, Verification};
use crate::yaml::{Layout, Mapping, Value};
use std::borrow::Cow;
use std::fmt;
use std::sync::Arc;

/// The only frontmatter key OKF always requires: a concept carrying
/// nothing but `type` is fully conformant.
///
/// This is what [`Document::validate`](crate::Document::validate) enforces, and
/// it matches the reference implementation's `REQUIRED_FRONTMATTER_KEYS`. v0.1
/// required four keys; v0.2 narrowed the requirement to this one and demoted
/// the rest to recommendations ([`RECOMMENDED_FRONTMATTER_KEYS`]). It is also
/// what makes a markdown file a concept at all: a file with no `type` is not
/// one.
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

/// The prefix the specification reserves for producer-defined keys: future
/// OKF versions will not define a key that starts with it. Producers should
/// name their own keys `x-<producer>-<name>`.
pub const EXTENSION_KEY_PREFIX: &str = "x-";

/// The keys a bundle-root `index.md` may carry as bundle metadata.
pub const BUNDLE_METADATA_KEYS: [&str; 4] = ["okf_version", "title", "description", "license"];

/// Every frontmatter key the specification gives a meaning to, across all
/// families. Anything else is a producer extension.
pub const KNOWN_FRONTMATTER_KEYS: [&str; 18] = [
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
    "superseded_by",
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

/// The key order the reference implementation writes documents in.
///
/// That is its `_PREFERRED_KEY_ORDER`: identity first, then lifecycle, trust,
/// and provenance. `superseded_by` is placed beside the `status` it qualifies.
///
/// Presentational only. Frontmatter has no required key order, and a
/// consumer must not depend on one; see [`Frontmatter::reorder_preferred`].
pub const PREFERRED_KEY_ORDER: [&str; 12] = [
    "type",
    "resource",
    "title",
    "description",
    "tags",
    "status",
    "superseded_by",
    "generated",
    "verified",
    "stale_after",
    "sources",
    "usage_window",
];

/// A concept's frontmatter: an ordered key/value mapping with typed accessors
/// for the well-known OKF fields.
///
/// Two frontmatter blocks are equal when their mappings are equal; how the
/// text was originally laid out is not part of the comparison.
#[derive(Clone, Default)]
pub struct Frontmatter {
    map: Mapping,
    /// How the block was written, when it was parsed from text. See
    /// [`Frontmatter::to_yaml_preserving`].
    source: Option<Arc<SourceText>>,
    /// Document formatting, not part of the mapping or its equality.
    pub(crate) document_layout: Option<DocumentLayout>,
}

#[derive(Clone, Copy)]
pub(crate) struct DocumentLayout {
    pub blank_separator: bool,
    pub crlf: bool,
}

/// The original text of a parsed frontmatter block, entry by entry, with the
/// values those entries parsed to.
#[derive(Debug)]
struct SourceText {
    layout: Layout,
    values: Vec<(Value, Value)>,
}

impl PartialEq for Frontmatter {
    fn eq(&self, other: &Self) -> bool {
        self.map == other.map
    }
}

impl fmt::Debug for Frontmatter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The remembered source text is how the block was laid out, not what
        // it says, so it is left out.
        f.debug_struct("Frontmatter")
            .field("map", &self.map)
            .finish_non_exhaustive()
    }
}

impl Frontmatter {
    /// Creates an empty frontmatter block.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            map: Mapping::new(),
            source: None,
            document_layout: None,
        }
    }

    /// Wraps an existing mapping.
    #[must_use]
    pub const fn from_mapping(map: Mapping) -> Self {
        Self {
            map,
            source: None,
            document_layout: None,
        }
    }

    /// Parses the text of a frontmatter block (without its `---` delimiters),
    /// remembering how each top-level entry was written so that
    /// [`Frontmatter::to_yaml_preserving`] can write unchanged entries back
    /// verbatim.
    ///
    /// An empty block parses to an empty frontmatter.
    ///
    /// # Errors
    ///
    /// Returns [`DocumentError::InvalidYaml`] if the text is not valid YAML,
    /// and [`DocumentError::FrontmatterNotMapping`] if it parses to a scalar
    /// or sequence rather than a mapping.
    pub fn parse(text: &str) -> Result<Self, DocumentError> {
        let (value, layout) = crate::yaml::parse_with_layout(text)?;
        match value {
            Value::Null => Ok(Self::new()),
            Value::Mapping(map) => {
                let source = layout.map(|layout| {
                    Arc::new(SourceText {
                        layout,
                        values: map.entries().to_vec(),
                    })
                });
                Ok(Self {
                    map,
                    source,
                    document_layout: None,
                })
            }
            _ => Err(DocumentError::FrontmatterNotMapping),
        }
    }

    /// Serializes the block as YAML, writing every top-level entry whose value
    /// is unchanged since parsing exactly as it was written.
    ///
    /// Quoting, flow or block style, and comments of untouched entries
    /// survive. An entry that was added, or whose value changed, is emitted in
    /// the canonical block style; the comment lines that preceded a changed
    /// entry are kept, and those of a removed entry go with it. Entries follow
    /// the current key order. If moving or removing an entry makes adjacent
    /// blank lines change a preserved value, only that entry is re-emitted
    /// canonically to keep its value intact.
    ///
    /// A block that was built in code rather than parsed has no original
    /// text, and this is then the same as [`Frontmatter::to_yaml_canonical`].
    #[must_use]
    pub fn to_yaml_preserving(&self) -> String {
        let Some(source) = &self.source else {
            return self.to_yaml_canonical();
        };
        let mut used = vec![false; source.values.len()];
        let mut entries: Vec<(&str, Cow<'_, str>)> = Vec::with_capacity(self.map.len());
        for (key, value) in self.map.entries() {
            let original = source
                .values
                .iter()
                .enumerate()
                .find(|(i, (k, _))| !used[*i] && k == key);
            let entry = match original {
                Some((i, (_, original_value))) => {
                    used[i] = true;
                    let entry = &source.layout.entries[i];
                    let text = if original_value == value {
                        Cow::Borrowed(entry.text.as_str())
                    } else {
                        Cow::Owned(Value::entry_to_yaml_string(key, value))
                    };
                    (entry.leading.as_str(), text)
                }
                None => ("", Cow::Owned(Value::entry_to_yaml_string(key, value))),
            };
            entries.push(entry);
        }

        let mut out = String::new();
        for (i, (leading, text)) in entries.iter().enumerate() {
            let (key, value) = &self.map.entries()[i];
            let following = entries
                .get(i + 1)
                .map_or(source.layout.trailing.as_str(), |(leading, _)| *leading);
            // A moved `|+` scalar can absorb blanks that originally followed
            // another entry. Check its new boundary before reusing raw text;
            // quoting only the affected entry leaves other styles intact.
            let keeps_value = following.is_empty()
                || matches!(
                    Value::parse(&format!("{text}{following}")),
                    Ok(Value::Mapping(map)) if map.len() == 1
                        && map.entries()[0].0 == *key && map.entries()[0].1 == *value
                );
            out.push_str(leading);
            if keeps_value {
                out.push_str(text);
            } else {
                out.push_str(&Value::entry_to_yaml_string(key, value));
            }
        }
        out.push_str(&source.layout.trailing);
        out
    }

    /// Serializes the block as YAML in the canonical block style, ignoring how
    /// it was originally written. This is what `okf fmt` produces.
    #[must_use]
    pub fn to_yaml_canonical(&self) -> String {
        if self.map.is_empty() {
            return String::new();
        }
        Value::Mapping(self.map.clone()).to_yaml_string()
    }

    /// `true` when [`Frontmatter::to_yaml_preserving`] would reproduce text
    /// that differs from the canonical style, i.e. the block is not laid out
    /// the way `okf fmt` would write it.
    #[must_use]
    pub fn is_canonical(&self) -> bool {
        self.to_yaml_preserving() == self.to_yaml_canonical()
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

    /// Number of frontmatter keys.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.map.len()
    }

    /// `true` if there are no keys.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// `true` if frontmatter contains the given key.
    #[must_use]
    pub fn contains_key(&self, key: &str) -> bool {
        self.map.contains_key(key)
    }

    /// Iterates over frontmatter `(key, value)` pairs in order.
    pub fn iter(&self) -> impl Iterator<Item = (&Value, &Value)> {
        self.map.iter()
    }

    /// Iterates over frontmatter string keys in order.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.map.keys()
    }

    /// Iterates over frontmatter values in order.
    pub fn values(&self) -> impl Iterator<Item = &Value> {
        self.map.values()
    }

    /// Raw value for an arbitrary key (including producer extensions).
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.map.get(key)
    }

    /// Looks up a mutable value by string key.
    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        self.map.get_mut(key)
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
        for preferred in PREFERRED_KEY_ORDER {
            for (key, value) in self.map.iter() {
                if key.as_str() == Some(preferred) {
                    ordered.push_raw(key.clone(), value.clone());
                }
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
    /// display form, and empty ones are dropped.
    ///
    /// A scalar `tags` value is one tag. It is not split at commas, so
    /// `tags: sales, orders` reads as the single tag `sales, orders`; the
    /// validator reports that shape so the producer can write a list.
    pub fn tags(&self) -> Vec<String> {
        self.map.get("tags").map_or_else(Vec::new, |value| {
            value
                .as_list()
                .into_iter()
                .filter_map(Value::as_display_string)
                .filter(|tag| !tag.trim().is_empty())
                .collect()
        })
    }

    /// The `sources` entries: the materials this concept derives from.
    ///
    /// An entry without the required `resource` is ignored, as the
    /// specification asks of consumers. Use [`Source::list_from_value`] on the
    /// raw value to see such entries too.
    pub fn sources(&self) -> Vec<Source> {
        self.map
            .get("sources")
            .map_or_else(Vec::new, Source::list_from_value)
            .into_iter()
            .filter(Source::has_resource)
            .collect()
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
    /// spec requires. An event without both `by` and `at` is ignored. Events
    /// older than `generated.at` are included: they are history a consumer may
    /// show. Use [`Frontmatter::current_verifications`] for the ones that
    /// count toward the trust tier, and [`Verification::list_from_value`] on
    /// the raw value to see incomplete events too.
    pub fn verified(&self) -> Vec<Verification> {
        self.map
            .get("verified")
            .map_or_else(Vec::new, Verification::list_from_value)
            .into_iter()
            .filter(Verification::is_complete)
            .collect()
    }

    /// The verification events that are **current**: at or after
    /// `generated.at`, or all valid events when the concept records no such
    /// time.
    #[must_use]
    pub fn current_verifications(&self) -> Vec<Verification> {
        let changed_at = self.trust_baseline();
        self.verified()
            .into_iter()
            .filter(|event| event.is_current(changed_at.as_ref()))
            .collect()
    }

    /// `true` when the concept has valid verification events but none of them
    /// is current, so its content changed after the last check.
    ///
    /// A consumer may show such a concept as "verified before the last
    /// change". Its trust tier is `unverified`.
    #[must_use]
    pub fn verified_before_change(&self) -> bool {
        let changed_at = self.trust_baseline();
        let events = self.verified();
        events.iter().any(Verification::is_valid)
            && !events
                .iter()
                .any(|event| event.is_current(changed_at.as_ref()))
    }

    /// The verification with the latest parseable `at`, current or not.
    #[must_use]
    pub fn latest_verification(&self) -> Option<Verification> {
        let events = self.verified();
        trust::latest_verification(&events).cloned()
    }

    /// The trust tier, derived from the current verification events.
    #[must_use]
    pub fn trust_tier(&self) -> TrustTier {
        TrustTier::derive(&self.verified(), self.trust_baseline().as_ref())
    }

    /// The timestamp verification events are compared against to decide
    /// whether they are current.
    ///
    /// That is `generated.at`. When `generated` is absent, a legacy v0.1
    /// `timestamp` stands in for it if it parses; one that does not is not
    /// used, since the fallback is optional and should not cost a concept its
    /// tier.
    fn trust_baseline(&self) -> Option<DateTimeField> {
        if let Some(generated) = self.generated() {
            return generated.at;
        }
        self.timestamp()
            .map(|raw| DateTimeField::new(raw.into_owned()))
            .filter(DateTimeField::is_valid)
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
        Status::parse(self.scalar_or_shape("status").as_deref())
    }

    /// `true` when the concept counts as current knowledge: its `status` is
    /// `stable` (or absent) or `draft`. See [`Status::is_current`].
    #[must_use]
    pub fn is_current(&self) -> bool {
        self.status().is_current()
    }

    /// The `superseded_by` path: the concept that replaces this one.
    ///
    /// It has a meaning only with `status: deprecated`, and this returns the
    /// value whatever the status so that a validator can report the mismatch.
    /// Resolve it with
    /// [`Bundle::superseded_by`](crate::Bundle::superseded_by), which applies
    /// that condition.
    #[must_use]
    pub fn superseded_by(&self) -> Option<Cow<'_, str>> {
        self.display_str("superseded_by")
    }

    /// The `stale_after` timestamp, on and after which the content is stale.
    /// A null or empty value is the same as an absent key.
    #[must_use]
    pub fn stale_after(&self) -> Option<DateTimeField> {
        self.scalar_or_shape("stale_after")
            .map(|s| DateTimeField::new(s.into_owned()))
    }

    /// Whether the concept is within its review window at `now`.
    ///
    /// A `stale_after` that does not parse is [`Freshness::Malformed`], which
    /// a consumer must not treat as fresh.
    #[must_use]
    pub fn freshness_at(&self, now: DateTime) -> Freshness {
        trust::freshness_at(self.stale_after().as_ref(), now)
    }

    /// Whether the concept must not be presented as fresh at `now`:
    /// `now >= stale_after`, or `stale_after` is malformed. A concept with no
    /// `stale_after` is never stale.
    #[must_use]
    pub fn is_stale_at(&self, now: DateTime) -> bool {
        self.freshness_at(now).is_stale()
    }

    /// Whether the concept is stale on `today`, evaluated at midnight UTC.
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
    ///
    /// An entry without both `name` and `type` is ignored. Use
    /// [`Parameter::list_from_value`] on the raw value to see such entries.
    pub fn parameters(&self) -> Vec<Parameter> {
        self.map
            .get("parameters")
            .map_or_else(Vec::new, Parameter::list_from_value)
            .into_iter()
            .filter(Parameter::is_complete)
            .collect()
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

    /// The path fields present, as `(field name, raw value)`.
    ///
    /// These are the fields whose value is a path or a URI: `resource`,
    /// `computation`, `executor.resource`, `attester.resource`,
    /// `superseded_by`, and each `sources[N].resource` that is not a scope
    /// descriptor. They all resolve with the same rules as a link; see
    /// [`Bundle::resolve_path_field`](crate::Bundle::resolve_path_field).
    #[must_use]
    pub fn path_fields(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let mut push = |name: &str, value: Option<String>| {
            if let Some(v) = value.filter(|v| !v.trim().is_empty()) {
                out.push((name.to_string(), v));
            }
        };
        push("resource", self.resource().map(Cow::into_owned));
        push("computation", self.computation().map(Cow::into_owned));
        push(
            "executor.resource",
            self.executor().and_then(|e| e.resource),
        );
        push(
            "attester.resource",
            self.attester().and_then(|a| a.resource),
        );
        push("superseded_by", self.superseded_by().map(Cow::into_owned));
        if let Some(value) = self.map.get("sources") {
            // `N` is the position in the list as written, so that it names
            // the same entry a reader of the file would count to.
            for (i, item) in value.as_list().into_iter().enumerate() {
                if let Some(source) = Source::from_value(item)
                    && source.resource_kind() == crate::provenance::ResourceKind::Path
                {
                    push(&format!("sources[{i}].resource"), source.resource);
                }
            }
        }
        out
    }

    /// Sets the value of a path field named as [`Frontmatter::path_fields`]
    /// names it (`resource`, `computation`, `superseded_by`,
    /// `executor.resource`, `attester.resource`, or `sources[N].resource`).
    ///
    /// Only the one scalar is replaced: every other key of the surrounding
    /// mapping, extension keys included, is kept. Returns `false` when the
    /// field is not present, in which case nothing changes.
    pub fn set_path_field(&mut self, field: &str, value: &str) -> bool {
        let new = Value::String(value.to_string());
        match field {
            "resource" | "computation" | "superseded_by" => {
                if self.map.contains_key(field) {
                    self.map.insert(field, new);
                    return true;
                }
                false
            }
            "executor.resource" | "attester.resource" => {
                let key = field.trim_end_matches(".resource");
                match self.map.get_mut(key).and_then(Value::as_mapping_mut) {
                    Some(map) if map.contains_key("resource") => {
                        map.insert("resource", new);
                        true
                    }
                    _ => false,
                }
            }
            _ => {
                let Some(index) = field
                    .strip_prefix("sources[")
                    .and_then(|rest| rest.strip_suffix("].resource"))
                    .and_then(|n| n.parse::<usize>().ok())
                else {
                    return false;
                };
                let entry = match self.map.get_mut("sources") {
                    Some(Value::Sequence(items)) => items.get_mut(index),
                    Some(single @ Value::Mapping(_)) if index == 0 => Some(single),
                    _ => None,
                };
                match entry.and_then(Value::as_mapping_mut) {
                    Some(map) if map.contains_key("resource") => {
                        map.insert("resource", new);
                        true
                    }
                    _ => false,
                }
            }
        }
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

    /// Returns the extension keys that do not start with the reserved `x-`
    /// prefix, and so could collide with a key a future OKF version defines.
    #[must_use]
    pub fn unprefixed_extension_keys(&self) -> Vec<&str> {
        self.extension_keys()
            .into_iter()
            .filter(|key| !key.starts_with(EXTENSION_KEY_PREFIX))
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

    /// As [`Frontmatter::display_str`], but a value of the wrong shape (a list
    /// or a mapping where a scalar belongs) is returned as its YAML text
    /// instead of being read as absent.
    ///
    /// For `status` and `stale_after`, absence has a meaning of its own
    /// (stable, and no review date). A value that is present but unreadable
    /// must not be mistaken for that: it is an unknown status, and a
    /// `stale_after` that does not parse.
    fn scalar_or_shape(&self, key: &str) -> Option<Cow<'_, str>> {
        let value = self.map.get(key).filter(|value| !value.is_blank())?;
        value.as_display_str().map_or_else(
            || {
                let text = value.to_yaml_string();
                Some(Cow::Owned(
                    text.split_whitespace().collect::<Vec<_>>().join(" "),
                ))
            },
            |text| (!text.trim().is_empty()).then_some(text),
        )
    }

    /// Borrows the scalar at `key` as a display string, coercing non-string
    /// scalars (a `type: 42` deviation yields `Some("42")`) the way the
    /// reference's `str(fm.get(...))` does. Returns `None` for absent keys,
    /// non-scalar values, and an empty string, which reads as an absent key.
    /// The common YAML-string case borrows without allocation; only the
    /// coerced case owns.
    fn display_str(&self, key: &str) -> Option<Cow<'_, str>> {
        self.map
            .get(key)
            .and_then(Value::as_display_str)
            .filter(|s| !s.trim().is_empty())
    }
}

impl From<Mapping> for Frontmatter {
    fn from(map: Mapping) -> Self {
        Self::from_mapping(map)
    }
}

impl From<Frontmatter> for Mapping {
    fn from(fm: Frontmatter) -> Self {
        fm.into_mapping()
    }
}

impl fmt::Display for Frontmatter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&Value::Mapping(self.map.clone()).to_yaml_string())
    }
}

impl std::str::FromStr for Frontmatter {
    type Err = DocumentError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl FromIterator<(String, Value)> for Frontmatter {
    fn from_iter<T: IntoIterator<Item = (String, Value)>>(iter: T) -> Self {
        Self::from_mapping(Mapping::from_iter(iter))
    }
}

impl<'a> FromIterator<(&'a str, Value)> for Frontmatter {
    fn from_iter<T: IntoIterator<Item = (&'a str, Value)>>(iter: T) -> Self {
        Self::from_mapping(Mapping::from_iter(iter))
    }
}

impl FromIterator<(Value, Value)> for Frontmatter {
    fn from_iter<T: IntoIterator<Item = (Value, Value)>>(iter: T) -> Self {
        Self::from_mapping(Mapping::from_iter(iter))
    }
}

impl Extend<(String, Value)> for Frontmatter {
    fn extend<T: IntoIterator<Item = (String, Value)>>(&mut self, iter: T) {
        self.map.extend(iter);
    }
}

impl<'a> Extend<(&'a str, Value)> for Frontmatter {
    fn extend<T: IntoIterator<Item = (&'a str, Value)>>(&mut self, iter: T) {
        self.map.extend(iter);
    }
}

impl Extend<(Value, Value)> for Frontmatter {
    fn extend<T: IntoIterator<Item = (Value, Value)>>(&mut self, iter: T) {
        self.map.extend(iter);
    }
}
