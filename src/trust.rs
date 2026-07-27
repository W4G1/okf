//! Trust and lifecycle frontmatter: `generated`, `verified`, `status`, and
//! `stale_after` (§5.2 to §5.5).
//!
//! These four keys let a consumer answer "how much should I trust this" and "is
//! it still current" from frontmatter alone. All are optional, and their
//! *absence* is meaningful rather than invalid: a concept with no trust
//! frontmatter is [`TrustTier::Unverified`] and `status: stable`, and must
//! never be rejected (§11).
//!
//! Two rules from the spec are encoded here rather than left to callers:
//!
//! - A bare `verified: { by, at }` mapping **must** be read as a one-element
//!   list ([`Verification::list_from_value`], §5.2).
//! - The trust tier is *derived* from `verified`, never stored
//!   ([`TrustTier::derive`], §5.3).

use crate::actor::Actor;
use crate::date::{Date, DateTimeField};
use crate::yaml::Value;
use std::fmt;

/// How the current content was produced (§5.2): `generated: { by, at }`.
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

/// A single verification event (§5.2): `{ by, at }`.
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

    /// Reads a whole `verified` value into a list of events.
    ///
    /// Consumers **MUST** treat a bare `{ by, at }` mapping as a one-element
    /// list (§5.2, restated as a conformance rule in §11), so that shape is
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

/// Returns the verification with the latest parseable `at` (§5.2).
///
/// Events whose `at` is missing or unparseable cannot be ordered and are
/// skipped; `None` means no event carries a usable timestamp.
#[must_use]
pub fn latest_verification(events: &[Verification]) -> Option<&Verification> {
    events
        .iter()
        .filter_map(|v| v.at.as_ref().and_then(|a| a.datetime).map(|dt| (v, dt)))
        .max_by_key(|(_, dt)| *dt)
        .map(|(v, _)| v)
}

/// A concept's trust tier, derived from `verified` (§5.3).
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
    /// Derives the tier from a concept's verification events (§5.3).
    #[must_use]
    pub fn derive(events: &[Verification]) -> Self {
        if events.is_empty() {
            Self::Unverified
        } else if events
            .iter()
            .any(|v| v.by.as_ref().is_some_and(Actor::is_human))
        {
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

/// A concept's lifecycle `status` (§5.4). An absent key means
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
    /// must tolerate it (§11).
    Other(String),
}

/// The `status` values §5.4 defines.
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

    /// `true` for one of the three values §5.4 defines.
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

/// Whether a concept with this `stale_after` date is stale on `today`.
///
/// §5.5: "A concept is stale when `today >= stale_after`." An absent or
/// unparseable `stale_after` is never stale.
#[must_use]
pub fn is_stale_on(stale_after: Option<Date>, today: Date) -> bool {
    stale_after.is_some_and(|d| today >= d)
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
            Verification::list_from_value(&v("{ by: human:ahormati, at: 2026-06-25T09:00:00Z }"));
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

        let both = Verification::list_from_value(&v(
            "- { by: human:ahormati, at: 2026-06-25T09:00:00Z }\n\
             - { by: process:finance-nightly, at: 2026-06-26T02:00:00Z }",
        ));
        assert_eq!(both.len(), 2);
        assert_eq!(TrustTier::derive(&both), TrustTier::HumanReviewed);
        assert!(TrustTier::HumanReviewed > TrustTier::MachineConfirmed);

        // "How recently" is the latest `at`, not the last entry.
        let latest = latest_verification(&both).unwrap();
        assert_eq!(latest.at.as_ref().unwrap().raw, "2026-06-26T02:00:00Z");
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
    fn staleness_is_a_plain_date_comparison() {
        let stale_after = Date::new(2026, 9, 23);
        assert!(!is_stale_on(stale_after, Date::new(2026, 9, 22).unwrap()));
        assert!(
            is_stale_on(stale_after, Date::new(2026, 9, 23).unwrap()),
            "stale on the day itself"
        );
        assert!(is_stale_on(stale_after, Date::new(2026, 9, 24).unwrap()));
        assert!(!is_stale_on(None, Date::new(2099, 1, 1).unwrap()));
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
