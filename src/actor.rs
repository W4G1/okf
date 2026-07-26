//! The actor convention (§7): who or what performed an action.
//!
//! Fields that record an identity (`generated.by`, `verified[].by`, and
//! `sources[].author`, §5.1) share one convention:
//!
//! | Form                    | Meaning                | Example                          |
//! |-------------------------|------------------------|----------------------------------|
//! | `<producer>/<version>`  | an agent or tool       | `reference_agent/gemini-2.5-pro` |
//! | `human:<id>`            | a person               | `human:ahormati`                 |
//! | `process:<id>`          | an automated process   | `process:finance-nightly`        |
//!
//! The `human:` prefix is load-bearing: trust tiers (§5.3) are derived from it,
//! so [`Actor::is_human`] is the single place that decision is made.
//!
//! Anything else parses as [`ActorKind::Other`] rather than an error. The spec
//! itself writes `author: team:ga4-docs`, so other `<scheme>:<id>` forms do
//! occur in practice; a consumer must keep them, not reject them (§11).

use std::fmt;

/// The category an actor string falls into.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ActorKind {
    /// A person: `human:<id>`.
    Human,
    /// An automated process: `process:<id>`.
    Process,
    /// An agent or tool: `<producer>/<version>`.
    Agent,
    /// Any other identity string (for example the spec's `team:ga4-docs`).
    Other,
}

impl fmt::Display for ActorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ActorKind::Human => "human",
            ActorKind::Process => "process",
            ActorKind::Agent => "agent",
            ActorKind::Other => "other",
        })
    }
}

/// A parsed actor string (§7), retaining the text exactly as written.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Actor {
    raw: String,
    kind: ActorKind,
}

impl Actor {
    /// Classifies an actor string. Never fails: an unrecognized form becomes
    /// [`ActorKind::Other`] with the raw text preserved.
    pub fn parse(s: impl Into<String>) -> Actor {
        let raw = s.into();
        let t = raw.trim();
        let kind = if t.strip_prefix("human:").is_some_and(|id| !id.is_empty()) {
            ActorKind::Human
        } else if t.strip_prefix("process:").is_some_and(|id| !id.is_empty()) {
            ActorKind::Process
        } else if is_agent(t) {
            ActorKind::Agent
        } else {
            ActorKind::Other
        };
        Actor { raw, kind }
    }

    /// The actor string exactly as written.
    pub fn as_str(&self) -> &str {
        &self.raw
    }

    /// Which of the §7 forms this actor uses.
    pub fn kind(&self) -> ActorKind {
        self.kind
    }

    /// `true` for a `human:<id>` actor, the signal trust tiers key off (§5.3).
    pub fn is_human(&self) -> bool {
        self.kind == ActorKind::Human
    }

    /// The identifying part: the text after `human:` / `process:`, the producer
    /// of an agent, or the whole string otherwise.
    pub fn id(&self) -> &str {
        let t = self.raw.trim();
        match self.kind {
            ActorKind::Human => &t["human:".len()..],
            ActorKind::Process => &t["process:".len()..],
            ActorKind::Agent => self.producer().unwrap_or(t),
            ActorKind::Other => t,
        }
    }

    /// The `<producer>` half of an agent actor.
    pub fn producer(&self) -> Option<&str> {
        self.agent_halves().map(|(p, _)| p)
    }

    /// The `<version>` half of an agent actor.
    pub fn version(&self) -> Option<&str> {
        self.agent_halves().map(|(_, v)| v)
    }

    fn agent_halves(&self) -> Option<(&str, &str)> {
        if self.kind != ActorKind::Agent {
            return None;
        }
        self.raw.trim().split_once('/')
    }
}

impl fmt::Display for Actor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.raw)
    }
}

impl From<&str> for Actor {
    fn from(s: &str) -> Self {
        Actor::parse(s)
    }
}

/// `<producer>/<version>`: a single `/` with non-empty text on both sides, and
/// no scheme separator that would make it a URL.
fn is_agent(t: &str) -> bool {
    match t.split_once('/') {
        Some((producer, version)) => {
            !producer.is_empty() && !version.is_empty() && !version.contains('/')
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_the_three_conventional_forms() {
        let agent = Actor::parse("reference_agent/gemini-2.5-pro");
        assert_eq!(agent.kind(), ActorKind::Agent);
        assert_eq!(agent.producer(), Some("reference_agent"));
        assert_eq!(agent.version(), Some("gemini-2.5-pro"));
        assert!(!agent.is_human());

        let human = Actor::parse("human:ahormati");
        assert_eq!(human.kind(), ActorKind::Human);
        assert!(human.is_human());
        assert_eq!(human.id(), "ahormati");

        let process = Actor::parse("process:finance-nightly");
        assert_eq!(process.kind(), ActorKind::Process);
        assert!(!process.is_human());
        assert_eq!(process.id(), "finance-nightly");
    }

    #[test]
    fn other_forms_are_kept_not_rejected() {
        // The spec's own `sources[].author` example.
        let team = Actor::parse("team:ga4-docs");
        assert_eq!(team.kind(), ActorKind::Other);
        assert_eq!(team.as_str(), "team:ga4-docs");
        assert_eq!(team.id(), "team:ga4-docs");

        assert_eq!(Actor::parse("human:").kind(), ActorKind::Other);
        assert_eq!(Actor::parse("a/b/c").kind(), ActorKind::Other);
    }

    #[test]
    fn display_round_trips_the_raw_string() {
        for s in ["human:ahormati", "reference_agent/gemini-2.5-pro", "anything"] {
            assert_eq!(Actor::parse(s).to_string(), s);
        }
    }
}
