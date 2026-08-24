//! Trust and lifecycle frontmatter: `generated`, `verified`, `status`, and
//! `stale_after`.
//!
//! These four keys let a consumer answer "how much should I trust this" and "is
//! it still current" from frontmatter alone. All are optional, and their
//! *absence* is meaningful rather than invalid: a concept with no trust
//! frontmatter is [`TrustTier::Unverified`] and `status: stable`, and must
//! never be rejected.
//!
//! Two rules from the spec are encoded here rather than left to callers:
//!
//! - A bare `verified: { by, at }` mapping **must** be read as a one-element
//!   list ([`Verification::list_from_value`]).
//! - The trust tier is *derived* from `verified`, never stored
//!   ([`TrustTier::derive`]).

use crate::actor::Actor;
use crate::date::{Date, DateTime, DateTimeField};
use crate::yaml::Value;
use std::fmt;

/// How the current content was produced: `generated: { by, at }`.
///
/// Distinct from [`Verification`] on purpose: who *wrote* a concept need not be
/// who *confirmed* it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Generated {
    /// The producing actor. REQUIRED within `generated`; `None` marks a
    /// malformed block rather than a fatal error.
    pub by: Option<Actor>,
    /// When the content last meaningfully changed.
    pub at: Option<DateTimeField>,
}

impl Generated {
    /// Reads a `generated` value. Returns `None` when the value is not a
    /// mapping.
    pub fn from_value(value: &Value) -> Option<Self> {
        let map = value.as_mapping()?;
        Some(Self {
            by: map
                .get("by")
                .and_then(Value::as_display_string)
                .map(Actor::parse),
            at: map
                .get("at")
                .and_then(Value::as_display_string)
                .map(DateTimeField::new),
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
/// nightly process, say. "How recently" is the latest [`Verification::at`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verification {
    /// The verifying actor.
    pub by: Option<Actor>,
    /// When the verification happened.
    pub at: Option<DateTimeField>,
}

impl Verification {
    /// Reads one `{ by, at }` mapping. Returns `None` when the value is not a
    /// mapping.
    pub fn from_value(value: &Value) -> Option<Self> {
        let map = value.as_mapping()?;
        Some(Self {
            by: map
                .get("by")
                .and_then(Value::as_display_string)
                .map(Actor::parse),
            at: map
                .get("at")
                .and_then(Value::as_display_string)
                .map(DateTimeField::new),
        })
    }

    /// `true` when this event has the required actor and a parseable timestamp
    /// that includes a time of day and an explicit UTC offset.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.by
            .as_ref()
            .is_some_and(|by| !by.as_str().trim().is_empty())
            && self.at.as_ref().is_some_and(DateTimeField::is_valid)
    }

    /// Reads a whole `verified` value into a list of events.
    ///
    /// Consumers **MUST** treat a bare `{ by, at }` mapping as a one-element
    /// list, so that shape is
    /// accepted here alongside the list form. Any other shape yields an empty
    /// list.
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
#[must_use]
pub fn latest_verification(events: &[Verification]) -> Option<&Verification> {
    events
        .iter()
        .filter(|v| v.is_valid())
        .filter_map(|v| v.at.as_ref().and_then(|at| at.datetime).map(|dt| (v, dt)))
        .max_by_key(|(_, dt)| *dt)
        .map(|(v, _)| v)
}

/// A concept's trust tier, derived from `verified`.
///
/// Ordering is by increasing trust, so tiers can be compared directly
/// (`tier >= TrustTier::MachineConfirmed`). Tiers are advisory signals, not
/// access control.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TrustTier {
    /// No `verified` key.
    Unverified,
    /// `verified` by non-`human:` actors only.
    MachineConfirmed,
    /// `verified` by at least one `human:<id>` actor.
    HumanReviewed,
}

impl TrustTier {
    /// Derives the tier from a concept's verification events.
    #[must_use]
    pub fn derive(events: &[Verification]) -> Self {
        let mut valid_events = events.iter().filter(|event| event.is_valid());
        if valid_events.clone().next().is_none() {
            Self::Unverified
        } else if valid_events.any(|v| v.by.as_ref().is_some_and(Actor::is_human)) {
            Self::HumanReviewed
        } else {
            Self::MachineConfirmed
        }
    }
}

impl fmt::Display for TrustTier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unverified => "unverified",
            Self::MachineConfirmed => "machine-confirmed",
            Self::HumanReviewed => "human-reviewed",
        })
    }
}

/// A concept's lifecycle `status`. An absent key means
/// [`Status::Stable`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// Not yet reviewed; possibly incomplete.
    Draft,
    /// The default: ready for consumption.
    Stable,
    /// Kept for links and history; no longer current.
    Deprecated,
    /// A producer-defined value outside the three the spec names. Consumers
    /// must tolerate it.
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
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Draft => f.write_str("draft"),
            Self::Stable => f.write_str("stable"),
            Self::Deprecated => f.write_str("deprecated"),
            Self::Other(s) => f.write_str(s),
        }
    }
}

/// Whether a concept with this `stale_after` timestamp is stale at `now`.
///
/// Staleness rule: "A concept is stale when `now >= stale_after`." An absent,
/// offset-less, or unparseable `stale_after` is never stale.
#[must_use]
pub fn is_stale_at(stale_after: Option<DateTime>, now: DateTime) -> bool {
    stale_after.is_some_and(|dt| dt.offset_minutes.is_some() && dt.has_time && now >= dt)
}

/// Whether a concept with this `stale_after` timestamp is stale on `today`.
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

    #[test]
    fn bare_verified_mapping_is_a_one_element_list() {
        let bare =
            Verification::list_from_value(&v("{ by: human:walter, at: 2026-06-25T09:00:00Z }"));
        assert_eq!(bare.len(), 1);
        assert!(bare[0].by.as_ref().unwrap().is_human());
        assert_eq!(TrustTier::derive(&bare), TrustTier::HumanReviewed);
    }

    #[test]
    fn trust_tiers_key_off_the_human_prefix() {
        assert_eq!(TrustTier::derive(&[]), TrustTier::Unverified);

        let machine = Verification::list_from_value(&v(
            "- { by: process:finance-nightly, at: 2026-06-26T02:00:00Z }",
        ));
        assert_eq!(TrustTier::derive(&machine), TrustTier::MachineConfirmed);

        let both =
            Verification::list_from_value(&v("- { by: human:walter, at: 2026-06-25T09:00:00Z }\n\
             - { by: process:finance-nightly, at: 2026-06-26T02:00:00Z }"));
        assert_eq!(both.len(), 2);
        assert_eq!(TrustTier::derive(&both), TrustTier::HumanReviewed);
        assert!(TrustTier::HumanReviewed > TrustTier::MachineConfirmed);

        // "How recently" is the latest `at`, not the last entry.
        let latest = latest_verification(&both).unwrap();
        assert_eq!(latest.at.as_ref().unwrap().raw, "2026-06-26T02:00:00Z");
    }

    #[test]
    fn malformed_verification_events_do_not_raise_trust() {
        let malformed =
            Verification::list_from_value(&v("- { by: human:walter, at: yesterday }\n\
             - { by: human:other, at: 2026-06-26 }\n\
             - { by: human:third }"));
        assert_eq!(malformed.len(), 3);
        assert!(malformed.iter().all(|event| !event.is_valid()));
        assert_eq!(TrustTier::derive(&malformed), TrustTier::Unverified);
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
    }

    #[test]
    fn staleness_is_an_instant_comparison() {
        let stale_after = DateTime::parse("2026-09-23T00:00:00Z");
        assert!(!is_stale_at(
            stale_after,
            DateTime::parse("2026-09-22T23:59:59Z").unwrap()
        ));
        assert!(is_stale_at(
            stale_after,
            DateTime::parse("2026-09-23T00:00:00Z").unwrap()
        ));
        assert!(is_stale_at(
            stale_after,
            DateTime::parse("2026-09-24T12:00:00Z").unwrap()
        ));
        assert!(!is_stale_at(
            None,
            DateTime::parse("2099-01-01T00:00:00Z").unwrap()
        ));
        // Date-only or offset-less values are ignored
        assert!(!is_stale_at(
            DateTime::parse("2026-09-23"),
            DateTime::parse("2026-09-24T00:00:00Z").unwrap()
        ));
        assert!(!is_stale_at(
            DateTime::parse("2026-09-23T00:00:00"),
            DateTime::parse("2026-09-24T00:00:00Z").unwrap()
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
    }
}
