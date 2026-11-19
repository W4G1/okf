//! Trust and lifecycle frontmatter: `generated`, `verified`, `status`,
//! `superseded_by`, and `stale_after`.
//!
//! These keys let a consumer answer "how much can I trust this" and "is it
//! still current" from frontmatter alone. All are optional, and their
//! *absence* is meaningful rather than invalid: a concept with no trust
//! frontmatter is [`TrustTier::Unverified`] and `status: stable`, and must
//! never be rejected.
//!
//! Four rules from the spec are encoded here rather than left to callers:
//!
//! - A bare `verified: { by, at }` mapping **must** be read as a one-element
//!   list ([`Verification::list_from_value`]).
//! - An event without both `by` and `at` is ignored
//!   ([`Verification::is_valid`]).
//! - The trust tier is *derived*, never stored, and only from **current**
//!   events: those at or after `generated.at` ([`TrustTier::derive`]). When an
//!   agent rewrites a concept after a human review, the human did not read the
//!   new text, so the concept is not human-reviewed until a human confirms it
//!   again.
//! - A `stale_after` that does not parse is never treated as fresh
//!   ([`Freshness::Malformed`]).

use crate::actor::Actor;
use crate::date::{Date, DateTime, DateTimeField};
use crate::yaml::Value;
use std::fmt;

/// Reads a scalar at `key`, treating a null or empty value as absent.
fn non_blank(map: &crate::yaml::Mapping, key: &str) -> Option<String> {
    map.get(key)
        .and_then(Value::as_display_string)
        .filter(|s| !s.trim().is_empty())
}

/// How the current content was produced: `generated: { by, at }`.
///
/// Distinct from [`Verification`] on purpose: who *wrote* a concept need not be
/// who *confirmed* it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Generated {
    /// The producing actor. REQUIRED within `generated`; `None` marks a
    /// malformed block rather than a fatal error.
    pub by: Option<Actor>,
    /// When the content last meaningfully changed. A trivial edit (format,
    /// whitespace, link repair) does not move it.
    pub at: Option<DateTimeField>,
}

impl Generated {
    /// Reads a `generated` value. Returns `None` when the value is not a
    /// mapping.
    pub fn from_value(value: &Value) -> Option<Self> {
        let map = value.as_mapping()?;
        Some(Self {
            by: non_blank(map, "by").map(Actor::parse),
            at: non_blank(map, "at").map(DateTimeField::new),
        })
    }
}

impl fmt::Display for Generated {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.by, &self.at) {
            (Some(by), Some(at)) => write!(f, "{by} at {at}"),
            (Some(by), None) => write!(f, "{by}"),
            (None, Some(at)) => write!(f, "(unknown) at {at}"),
            (None, None) => f.write_str("(unknown)"),
        }
    }
}

/// A single verification event: `{ by, at }`.
///
/// Multiple entries capture independent checks, a human sign-off plus a
/// nightly process, say. The most recent check is the event with the latest
/// [`Verification::at`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verification {
    /// The verifying actor. REQUIRED within an event.
    pub by: Option<Actor>,
    /// When the verification happened. REQUIRED within an event.
    pub at: Option<DateTimeField>,
}

impl Verification {
    /// Reads one `{ by, at }` mapping. Returns `None` when the value is not a
    /// mapping.
    pub fn from_value(value: &Value) -> Option<Self> {
        let map = value.as_mapping()?;
        Some(Self {
            by: non_blank(map, "by").map(Actor::parse),
            at: non_blank(map, "at").map(DateTimeField::new),
        })
    }

    /// `true` when this event has both required keys, whatever their values.
    /// Consumers ignore an event that does not.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        self.by.is_some() && self.at.is_some()
    }

    /// `true` when this event has the required actor and a timestamp that
    /// parses. Only such an event can raise a trust tier.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.by.is_some() && self.at.as_ref().is_some_and(DateTimeField::is_valid)
    }

    /// The instant of the verification, when the event is valid.
    #[must_use]
    pub fn instant(&self) -> Option<DateTime> {
        self.by
            .as_ref()
            .and(self.at.as_ref())
            .and_then(DateTimeField::instant)
    }

    /// Whether this event is **current**: its `at` is equal to or later than
    /// the time the content last changed.
    ///
    /// `changed_at` is `generated.at`. With no such timestamp every valid
    /// event is current. When it is present but does not parse, no event is
    /// current: the comparison cannot be made, and the trust tier may only
    /// err downward.
    #[must_use]
    pub fn is_current(&self, changed_at: Option<&DateTimeField>) -> bool {
        let Some(at) = self.instant() else {
            return false;
        };
        changed_at.is_none_or(|changed| changed.instant().is_some_and(|changed| at >= changed))
    }

    /// Reads a whole `verified` value into a list of events.
    ///
    /// Consumers **MUST** treat a bare `{ by, at }` mapping as a one-element
    /// list, so that shape is accepted here alongside the list form. Any other
    /// shape yields an empty list. Every mapping-shaped entry is returned,
    /// including incomplete ones, so a validator can report them;
    /// [`Frontmatter::verified`](crate::Frontmatter::verified) drops the ones
    /// a consumer ignores.
    pub fn list_from_value(value: &Value) -> Vec<Self> {
        match value {
            Value::Sequence(items) => items.iter().filter_map(Self::from_value).collect(),
            Value::Mapping(_) => Self::from_value(value).into_iter().collect(),
            _ => Vec::new(),
        }
    }
}

impl fmt::Display for Verification {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.by, &self.at) {
            (Some(by), Some(at)) => write!(f, "{by} at {at}"),
            (Some(by), None) => write!(f, "{by}"),
            (None, Some(at)) => write!(f, "(unknown) at {at}"),
            (None, None) => f.write_str("(unknown)"),
        }
    }
}

/// Returns the verification with the latest parseable `at`.
///
/// Events missing a verifier or a valid, parseable `at` cannot be
/// ordered and are skipped; `None` means no event carries a usable timestamp.
/// History counts here: the latest event need not be a current one.
#[must_use]
pub fn latest_verification(events: &[Verification]) -> Option<&Verification> {
    events
        .iter()
        .filter_map(|v| v.instant().map(|at| (v, at)))
        .max_by_key(|(_, at)| *at)
        .map(|(v, _)| v)
}

/// The events that are **current** with respect to `changed_at`
/// (`generated.at`), in their original order.
///
/// See [`Verification::is_current`] for the rule. Earlier events stay in the
/// file as history; a consumer may show them as "verified before the last
/// change", but must not count them toward the trust tier.
#[must_use]
pub fn current_verifications<'a>(
    events: &'a [Verification],
    changed_at: Option<&DateTimeField>,
) -> Vec<&'a Verification> {
    events
        .iter()
        .filter(|event| event.is_current(changed_at))
        .collect()
}

/// A concept's trust tier, derived from its current `verified` events.
///
/// Ordering is by increasing trust, so tiers can be compared directly
/// (`tier >= TrustTier::MachineConfirmed`). Tiers are advisory signals, not
/// access control.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TrustTier {
    /// No current verification event.
    #[default]
    Unverified,
    /// Current events from non-`human:` actors only. That includes `team:`,
    /// `process:`, other schemes, and `<producer>/<version>` agents.
    MachineConfirmed,
    /// At least one current event from a `human:<id>` actor.
    HumanReviewed,
}

impl TrustTier {
    /// Derives the tier from a concept's verification events and the time its
    /// content last changed (`generated.at`).
    ///
    /// Only **current** events count: those at or after `changed_at`, or all
    /// valid events when there is no `changed_at`. Pass `None` only when the
    /// concept really has no `generated.at`; leaving it out for a concept that
    /// has one reports a review of text that has since been rewritten.
    #[must_use]
    pub fn derive(events: &[Verification], changed_at: Option<&DateTimeField>) -> Self {
        Self::from_current(current_verifications(events, changed_at))
    }

    /// Derives the tier from events already known to be current.
    #[must_use]
    pub fn from_current<'a>(current: impl IntoIterator<Item = &'a Verification>) -> Self {
        let mut tier = Self::Unverified;
        for event in current {
            if !event.is_valid() {
                continue;
            }
            if event.by.as_ref().is_some_and(Actor::is_human) {
                return Self::HumanReviewed;
            }
            tier = Self::MachineConfirmed;
        }
        tier
    }

    /// The string representation of this trust tier.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Unverified => "unverified",
            Self::MachineConfirmed => "machine-confirmed",
            Self::HumanReviewed => "human-reviewed",
        }
    }
}

impl AsRef<str> for TrustTier {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

/// Error returned when a string cannot be parsed into a [`TrustTier`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseTrustTierError(pub String);

impl fmt::Display for ParseTrustTierError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown trust tier: {:?}", self.0)
    }
}

impl std::error::Error for ParseTrustTierError {}

impl std::str::FromStr for TrustTier {
    type Err = ParseTrustTierError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim() {
            "unverified" => Ok(Self::Unverified),
            "machine-confirmed" | "machine_confirmed" => Ok(Self::MachineConfirmed),
            "human-reviewed" | "human_reviewed" => Ok(Self::HumanReviewed),
            other => Err(ParseTrustTierError(other.to_string())),
        }
    }
}

impl fmt::Display for TrustTier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A concept's lifecycle `status`. An absent key means
/// [`Status::Stable`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Status {
    /// Not yet reviewed; possibly incomplete.
    Draft,
    /// The default: ready for consumption.
    #[default]
    Stable,
    /// Kept for links and history; no longer current.
    Deprecated,
    /// A value outside the three the spec names. Consumers keep it and show
    /// it, but it is not `stable`: a filter to current knowledge excludes the
    /// concept.
    Other(String),
}

/// The `status` values the spec defines.
pub const STATUS_VALUES: [&str; 3] = ["draft", "stable", "deprecated"];

impl Status {
    /// Parses a `status` scalar. `None` (an absent key) is [`Status::Stable`].
    #[must_use]
    pub fn parse(value: Option<&str>) -> Self {
        value.map_or(Self::Stable, |s| match s.trim() {
            "draft" => Self::Draft,
            "stable" | "" => Self::Stable,
            "deprecated" => Self::Deprecated,
            other => Self::Other(other.to_string()),
        })
    }

    /// Returns the string representation of this status.
    #[must_use]
    pub const fn as_str(&self) -> &str {
        match self {
            Self::Draft => "draft",
            Self::Stable => "stable",
            Self::Deprecated => "deprecated",
            Self::Other(s) => s.as_str(),
        }
    }

    /// `true` for one of the three values the spec defines.
    #[must_use]
    pub const fn is_known(&self) -> bool {
        !matches!(self, Self::Other(_))
    }

    /// `true` for [`Status::Deprecated`], kept for links and history, but no
    /// longer current.
    #[must_use]
    pub fn is_deprecated(&self) -> bool {
        *self == Self::Deprecated
    }

    /// `true` when the concept counts as current knowledge: `stable` or
    /// `draft`.
    ///
    /// A deprecated concept is not current, and neither is one with a status
    /// outside the defined vocabulary. Consumers that filter to current
    /// knowledge exclude both.
    #[must_use]
    pub const fn is_current(&self) -> bool {
        matches!(self, Self::Draft | Self::Stable)
    }
}

impl AsRef<str> for Status {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl std::str::FromStr for Status {
    type Err = std::convert::Infallible;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::parse(Some(s)))
    }
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Whether a concept's content is still within its review window.
///
/// Stale means that a person or a process must check the content again. It
/// does not mean that the content is false. Freshness and trust are
/// independent: a human-reviewed concept can be stale.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Freshness {
    /// No `stale_after`: no review date is known.
    Undated,
    /// `now < stale_after`.
    Fresh,
    /// `now >= stale_after`.
    Stale,
    /// `stale_after` is present but does not parse. A consumer must not treat
    /// such a concept as fresh, and should show the value as malformed.
    Malformed,
}

impl Freshness {
    /// `true` when the concept must not be presented as fresh: it is past its
    /// `stale_after`, or that value is malformed.
    #[must_use]
    pub const fn is_stale(self) -> bool {
        matches!(self, Self::Stale | Self::Malformed)
    }

    /// The string representation of this state.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Undated => "undated",
            Self::Fresh => "fresh",
            Self::Stale => "stale",
            Self::Malformed => "malformed",
        }
    }
}

impl fmt::Display for Freshness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Evaluates a `stale_after` field at `now`.
///
/// An absent field is [`Freshness::Undated`]. A field that does not parse is
/// [`Freshness::Malformed`], which counts as stale.
#[must_use]
pub fn freshness_at(stale_after: Option<&DateTimeField>, now: DateTime) -> Freshness {
    stale_after.map_or(Freshness::Undated, |field| match field.instant() {
        None => Freshness::Malformed,
        Some(instant) if now >= instant => Freshness::Stale,
        Some(_) => Freshness::Fresh,
    })
}

/// Whether a concept that goes stale at the instant `stale_after` is stale at
/// `now`.
///
/// Staleness rule: "A concept is stale when `now >= stale_after`." This is the
/// plain comparison for a value that is already known to be a valid instant;
/// `None` (no `stale_after`) is never stale. Use [`freshness_at`] for a raw
/// field, so that a malformed value is not mistaken for a fresh one.
#[must_use]
pub fn is_stale_at(stale_after: Option<DateTime>, now: DateTime) -> bool {
    stale_after.is_some_and(|instant| now >= instant)
}

/// Whether a concept that goes stale at the instant `stale_after` is stale on
/// `today`.
///
/// Evaluates staleness at midnight UTC on `today`.
#[must_use]
pub fn is_stale_on(stale_after: Option<DateTime>, today: Date) -> bool {
    is_stale_at(stale_after, today.to_utc_datetime())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::yaml::Value;

    fn v(yaml: &str) -> Value {
        Value::parse(yaml).unwrap()
    }

    fn at(raw: &str) -> DateTimeField {
        DateTimeField::new(raw)
    }

    #[test]
    fn bare_verified_mapping_is_a_one_element_list() {
        let bare =
            Verification::list_from_value(&v("{ by: human:walter, at: 2026-06-25T09:00:00Z }"));
        assert_eq!(bare.len(), 1);
        assert!(bare[0].by.as_ref().unwrap().is_human());
        assert_eq!(TrustTier::derive(&bare, None), TrustTier::HumanReviewed);
    }

    #[test]
    fn trust_tiers_key_off_the_human_prefix() {
        assert_eq!(TrustTier::derive(&[], None), TrustTier::Unverified);

        let machine = Verification::list_from_value(&v(
            "- { by: process:finance-nightly, at: 2026-06-26T02:00:00Z }",
        ));
        assert_eq!(
            TrustTier::derive(&machine, None),
            TrustTier::MachineConfirmed
        );

        // A team name does not identify the person who did the check.
        let team = Verification::list_from_value(&v("- { by: team:finance-fpa, at: 2026-06-26 }"));
        assert_eq!(TrustTier::derive(&team, None), TrustTier::MachineConfirmed);

        let both =
            Verification::list_from_value(&v("- { by: human:walter, at: 2026-06-25T09:00:00Z }\n\
             - { by: process:finance-nightly, at: 2026-06-26T02:00:00Z }"));
        assert_eq!(both.len(), 2);
        assert_eq!(TrustTier::derive(&both, None), TrustTier::HumanReviewed);
        assert!(TrustTier::HumanReviewed > TrustTier::MachineConfirmed);

        // "How recently" is the latest `at`, not the last entry.
        let latest = latest_verification(&both).unwrap();
        assert_eq!(latest.at.as_ref().unwrap().raw, "2026-06-26T02:00:00Z");
    }

    #[test]
    fn only_current_events_give_the_tier() {
        let generated = at("2026-06-20T14:00:00Z");
        let human_before = "{ by: human:walter, at: 2026-06-01T09:00:00Z }";
        let human_same = "{ by: human:walter, at: 2026-06-20T14:00:00Z }";
        let process_after = "{ by: process:nightly, at: 2026-06-21T02:00:00+00:00 }";

        // A review before the last change is history, not a current event.
        let before = Verification::list_from_value(&v(human_before));
        assert_eq!(
            TrustTier::derive(&before, Some(&generated)),
            TrustTier::Unverified
        );
        assert_eq!(
            current_verifications(&before, Some(&generated)),
            Vec::<&Verification>::new()
        );
        // The event is still there to show.
        assert!(latest_verification(&before).is_some());

        let mixed =
            Verification::list_from_value(&v(&format!("- {human_before}\n- {process_after}")));
        assert_eq!(
            TrustTier::derive(&mixed, Some(&generated)),
            TrustTier::MachineConfirmed
        );
        assert_eq!(current_verifications(&mixed, Some(&generated)).len(), 1);

        // Equal instants are current.
        let same = Verification::list_from_value(&v(human_same));
        assert_eq!(
            TrustTier::derive(&same, Some(&generated)),
            TrustTier::HumanReviewed
        );

        // A date only compares as the start of that day in UTC.
        let day = Verification::list_from_value(&v("{ by: human:a, at: 2026-06-20 }"));
        assert_eq!(
            TrustTier::derive(&day, Some(&at("2026-06-20"))),
            TrustTier::HumanReviewed
        );
        assert_eq!(
            TrustTier::derive(&day, Some(&at("2026-06-20T00:00:01Z"))),
            TrustTier::Unverified
        );

        // A `generated.at` that does not parse cannot make an event current.
        assert_eq!(
            TrustTier::derive(&same, Some(&at("soon"))),
            TrustTier::Unverified
        );
    }

    #[test]
    fn malformed_verification_events_do_not_raise_trust() {
        let malformed =
            Verification::list_from_value(&v("- { by: human:walter, at: yesterday }\n\
             - { by: human:other, at: 2026-06-26T09:00:00 }\n\
             - { by: human:third }\n\
             - { by: \"\", at: 2026-06-26T09:00:00Z }"));
        assert_eq!(malformed.len(), 4);
        assert!(malformed.iter().all(|event| !event.is_valid()));
        assert!(malformed[0].is_complete());
        assert!(!malformed[2].is_complete());
        assert!(!malformed[3].is_complete());
        assert_eq!(TrustTier::derive(&malformed, None), TrustTier::Unverified);
        assert_eq!(latest_verification(&malformed), None);
    }

    #[test]
    fn status_defaults_to_stable_and_keeps_unknown_values() {
        assert_eq!(Status::parse(None), Status::Stable);
        assert_eq!(Status::parse(Some("draft")), Status::Draft);
        assert_eq!(Status::parse(Some("deprecated")), Status::Deprecated);
        let other = Status::parse(Some("experimental"));
        assert!(!other.is_known());
        assert_eq!(other.to_string(), "experimental");

        // An unknown status is kept, but it is not current.
        assert!(Status::Stable.is_current());
        assert!(Status::Draft.is_current());
        assert!(!Status::Deprecated.is_current());
        assert!(!other.is_current());
    }

    #[test]
    fn staleness_is_an_instant_comparison() {
        let stale_after = at("2026-09-23T00:00:00Z");
        let now = |raw: &str| DateTime::parse_rfc3339(raw).unwrap();

        assert_eq!(
            freshness_at(Some(&stale_after), now("2026-09-22T23:59:59Z")),
            Freshness::Fresh
        );
        assert_eq!(
            freshness_at(Some(&stale_after), now("2026-09-23T00:00:00Z")),
            Freshness::Stale
        );
        assert_eq!(
            freshness_at(
                Some(&at("2026-09-23T14:00:00+02:00")),
                now("2026-09-23T12:00:00Z")
            ),
            Freshness::Stale
        );
        assert_eq!(
            freshness_at(None, now("2099-01-01T00:00:00Z")),
            Freshness::Undated
        );
        assert!(!Freshness::Undated.is_stale());

        // A date only is the start of that day in UTC.
        assert_eq!(
            freshness_at(Some(&at("2026-09-23")), now("2026-09-23T12:00:00Z")),
            Freshness::Stale
        );
        assert_eq!(
            freshness_at(Some(&at("2026-09-24")), now("2026-09-23T12:00:00Z")),
            Freshness::Fresh
        );

        // A value that does not parse is never fresh.
        for raw in [
            "not-a-date",
            "2026-09-23T00:00:00",
            "2026-09-23 00:00:00Z",
            "2026-02-30",
        ] {
            let state = freshness_at(Some(&at(raw)), now("2020-01-01T00:00:00Z"));
            assert_eq!(state, Freshness::Malformed, "{raw}");
            assert!(state.is_stale(), "{raw}");
        }

        assert!(is_stale_at(
            stale_after.instant(),
            now("2026-09-24T12:00:00Z")
        ));
        assert!(!is_stale_at(None, now("2099-01-01T00:00:00Z")));
        assert!(is_stale_on(
            stale_after.instant(),
            Date::new(2026, 9, 23).unwrap()
        ));
    }

    #[test]
    fn generated_reads_actor_and_datetime() {
        let g = Generated::from_value(&v(
            "{ by: reference_agent/gemini-2.5-pro, at: 2026-06-20T22:53:05Z }",
        ))
        .unwrap();
        assert_eq!(g.by.as_ref().unwrap().producer(), Some("reference_agent"));
        assert!(g.at.as_ref().unwrap().is_valid());
        assert!(Generated::from_value(&v("just a string")).is_none());

        // Null and empty values are the same as an absent key.
        let blank = Generated::from_value(&v("{ by: \"\", at: null }")).unwrap();
        assert!(blank.by.is_none());
        assert!(blank.at.is_none());
    }
}
