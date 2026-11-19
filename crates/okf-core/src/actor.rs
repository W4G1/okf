//! The actor convention: who or what performed an action.
//!
//! Fields that record an identity (`generated.by`, `verified[].by`, and
//! `sources[].author`) share one convention. An actor has one of two forms:
//!
//! | Form                    | Meaning                | Example                          |
//! |-------------------------|------------------------|----------------------------------|
//! | `<producer>/<version>`  | an agent or tool       | `reference_agent/gemini-2.5-pro` |
//! | `<scheme>:<id>`         | any other actor        | `human:walter`                   |
//!
//! The `id` is an opaque string. OKF defines three schemes:
//!
//! | Scheme     | Meaning              | Example                   |
//! |------------|----------------------|---------------------------|
//! | `human:`   | a person             | `human:walter`            |
//! | `team:`    | a group of people    | `team:finance-fpa`        |
//! | `process:` | an automated process | `process:finance-nightly` |
//!
//! Producers may use other schemes, and a consumer accepts them and treats
//! them as non-human. Only `human:` gives the human-reviewed trust tier, so
//! [`Actor::is_human`] is the single place that decision is made. A `team:`
//! actor does not: a team name does not identify the person who did the check.
//!
//! A string in neither form still parses, as [`ActorKind::Other`] with
//! [`Actor::is_conventional`] returning `false`, rather than as an error.

use std::fmt;

/// The category an actor string falls into.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ActorKind {
    /// A person: `human:<id>`.
    Human,
    /// A group of people: `team:<id>`.
    Team,
    /// An automated process: `process:<id>`.
    Process,
    /// An agent or tool: `<producer>/<version>`.
    Agent,
    /// Any other identity: a `<scheme>:<id>` with a scheme OKF does not
    /// define, or a string in neither actor form. Always non-human.
    Other,
}

impl ActorKind {
    /// String representation of this actor kind.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Team => "team",
            Self::Process => "process",
            Self::Agent => "agent",
            Self::Other => "other",
        }
    }
}

impl AsRef<str> for ActorKind {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

/// Error returned when a string cannot be parsed into an [`ActorKind`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseActorKindError(pub String);

impl fmt::Display for ParseActorKindError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown actor kind: {:?}", self.0)
    }
}

impl std::error::Error for ParseActorKindError {}

impl std::str::FromStr for ActorKind {
    type Err = ParseActorKindError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "human" => Ok(Self::Human),
            "team" => Ok(Self::Team),
            "process" => Ok(Self::Process),
            "agent" => Ok(Self::Agent),
            "other" => Ok(Self::Other),
            other => Err(ParseActorKindError(other.to_string())),
        }
    }
}

impl fmt::Display for ActorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A parsed actor string, retaining the text exactly as written.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Actor {
    raw: String,
    kind: ActorKind,
    /// Byte offset of the separator in the trimmed text: the `/` of an agent,
    /// or the `:` of a `<scheme>:<id>`. `None` for a string in neither form.
    separator: Option<usize>,
    /// Whether the string is written the way the convention shows it, with
    /// no whitespace in it.
    conventional: bool,
}

impl Actor {
    /// Classifies an actor string. Never fails: an unrecognized form becomes
    /// [`ActorKind::Other`] with the raw text preserved.
    pub fn parse(s: impl Into<String>) -> Self {
        let raw = s.into();
        let t = raw.trim();
        let (kind, separator) = agent_separator(t).map_or_else(
            || {
                scheme_separator(t).map_or((ActorKind::Other, None), |colon| {
                    let kind = match &t[..colon] {
                        "human" => ActorKind::Human,
                        "team" => ActorKind::Team,
                        "process" => ActorKind::Process,
                        _ => ActorKind::Other,
                    };
                    (kind, Some(colon))
                })
            },
            |slash| (ActorKind::Agent, Some(slash)),
        );
        let conventional = separator.is_some() && !t.contains(char::is_whitespace);
        Self {
            raw,
            kind,
            separator,
            conventional,
        }
    }

    /// The actor string exactly as written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.raw
    }

    /// Which actor form this uses.
    #[must_use]
    pub const fn kind(&self) -> ActorKind {
        self.kind
    }

    /// `true` for a `human:<id>` actor, the signal trust tiers key off.
    #[must_use]
    pub fn is_human(&self) -> bool {
        self.kind == ActorKind::Human
    }

    /// `true` when the string follows the actor convention: either
    /// `<producer>/<version>` or `<scheme>:<id>`, with any scheme, and with
    /// no whitespace.
    ///
    /// A `false` is a producer deviation worth reporting, never a reason to
    /// reject the document. It does not change how the actor is read: the id
    /// is an opaque string, so `human:Jane Doe` is still a person.
    #[must_use]
    pub const fn is_conventional(&self) -> bool {
        self.conventional
    }

    /// The scheme of a `<scheme>:<id>` actor (`human`, `team`, `process`, or
    /// one a producer defined). `None` for an agent or an unconventional
    /// string.
    #[must_use]
    pub fn scheme(&self) -> Option<&str> {
        match (self.kind, self.separator) {
            (ActorKind::Agent, _) | (_, None) => None,
            (_, Some(colon)) => Some(&self.raw.trim()[..colon]),
        }
    }

    /// The identifying part: the text after the scheme of a `<scheme>:<id>`
    /// actor, the producer of an agent, or the whole string otherwise.
    #[must_use]
    pub fn id(&self) -> &str {
        let t = self.raw.trim();
        match (self.kind, self.separator) {
            (ActorKind::Agent, Some(slash)) => &t[..slash],
            (_, Some(colon)) => &t[colon + 1..],
            (_, None) => t,
        }
    }

    /// The `<producer>` half of an agent actor.
    #[must_use]
    pub fn producer(&self) -> Option<&str> {
        self.agent_halves().map(|(p, _)| p)
    }

    /// The `<version>` half of an agent actor.
    #[must_use]
    pub fn version(&self) -> Option<&str> {
        self.agent_halves().map(|(_, v)| v)
    }

    fn agent_halves(&self) -> Option<(&str, &str)> {
        if self.kind != ActorKind::Agent {
            return None;
        }
        let t = self.raw.trim();
        self.separator.map(|slash| (&t[..slash], &t[slash + 1..]))
    }
}

impl fmt::Display for Actor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.raw)
    }
}

impl From<&str> for Actor {
    fn from(s: &str) -> Self {
        Self::parse(s)
    }
}

impl From<String> for Actor {
    fn from(s: String) -> Self {
        Self::parse(s)
    }
}

impl std::str::FromStr for Actor {
    type Err = std::convert::Infallible;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::parse(s))
    }
}

impl AsRef<str> for Actor {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl std::ops::Deref for Actor {
    type Target = str;
    fn deref(&self) -> &str {
        self.as_str()
    }
}

/// `<producer>/<version>`: text up to the first `/` that contains no
/// whitespace and no `:`, then a non-empty version without whitespace.
///
/// The `:` exclusion is what keeps a URL or a `<scheme>:<id>` whose id happens
/// to contain a slash (`human:alice/ops`) out of this form. Returns the offset
/// of the separating `/`.
fn agent_separator(t: &str) -> Option<usize> {
    let slash = t.find('/')?;
    let (producer, version) = (&t[..slash], &t[slash + 1..]);
    let producer_ok =
        !producer.is_empty() && !producer.contains(|c: char| c.is_whitespace() || c == ':');
    let version_ok = !version.is_empty() && !version.contains(char::is_whitespace);
    (producer_ok && version_ok).then_some(slash)
}

/// `<scheme>:<id>`: an RFC 3986 scheme (`ALPHA *( ALPHA / DIGIT / "+" / "-" /
/// "." )`), a colon, and a non-empty id. The id is an opaque string, so
/// anything may follow the colon. Returns the offset of the colon.
fn scheme_separator(t: &str) -> Option<usize> {
    let colon = t.find(':')?;
    let (scheme, id) = (&t[..colon], &t[colon + 1..]);
    let mut chars = scheme.chars();
    let scheme_ok = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    (scheme_ok && !id.trim().is_empty()).then_some(colon)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_the_conventional_forms() {
        let agent = Actor::parse("reference_agent/gemini-2.5-pro");
        assert_eq!(agent.kind(), ActorKind::Agent);
        assert_eq!(agent.producer(), Some("reference_agent"));
        assert_eq!(agent.version(), Some("gemini-2.5-pro"));
        assert_eq!(agent.scheme(), None);
        assert!(!agent.is_human());
        assert!(agent.is_conventional());

        let human = Actor::parse("human:walter");
        assert_eq!(human.kind(), ActorKind::Human);
        assert!(human.is_human());
        assert_eq!(human.id(), "walter");
        assert_eq!(human.scheme(), Some("human"));

        let process = Actor::parse("process:finance-nightly");
        assert_eq!(process.kind(), ActorKind::Process);
        assert!(!process.is_human());
        assert_eq!(process.id(), "finance-nightly");
    }

    #[test]
    fn team_is_a_defined_scheme_but_not_human() {
        let team = Actor::parse("team:ga4-docs");
        assert_eq!(team.kind(), ActorKind::Team);
        assert_eq!(team.as_str(), "team:ga4-docs");
        assert_eq!(team.id(), "ga4-docs");
        assert_eq!(team.scheme(), Some("team"));
        assert!(!team.is_human());
        assert!(team.is_conventional());
    }

    #[test]
    fn other_schemes_are_accepted_as_non_human() {
        let bot = Actor::parse("bot:deploy-42");
        assert_eq!(bot.kind(), ActorKind::Other);
        assert_eq!(bot.scheme(), Some("bot"));
        assert_eq!(bot.id(), "deploy-42");
        assert!(bot.is_conventional());
        assert!(!bot.is_human());

        // An id may contain a slash or an `@` without becoming an agent.
        let scoped = Actor::parse("human:kliu@acme/ops");
        assert_eq!(scoped.kind(), ActorKind::Human);
        assert_eq!(scoped.id(), "kliu@acme/ops");

        // A version may contain a slash.
        let nested = Actor::parse("a/b/c");
        assert_eq!(nested.kind(), ActorKind::Agent);
        assert_eq!(nested.producer(), Some("a"));
        assert_eq!(nested.version(), Some("b/c"));
    }

    #[test]
    fn an_id_is_opaque_so_a_person_with_a_space_is_still_a_person() {
        // The trust tier keys off the `human:` scheme alone. The space is a
        // deviation from how the convention is written, and is reported, but
        // it must not cost the concept its human review.
        let jane = Actor::parse("human:Jane Doe");
        assert_eq!(jane.kind(), ActorKind::Human);
        assert!(jane.is_human());
        assert_eq!(jane.id(), "Jane Doe");
        assert!(!jane.is_conventional());
        assert_eq!(Actor::parse("team:Finance FP&A").kind(), ActorKind::Team);
    }

    #[test]
    fn unconventional_strings_are_kept_not_rejected() {
        for raw in ["human:", "Finance Team", "anything", "", ":x", "a b/c"] {
            let actor = Actor::parse(raw);
            assert_eq!(actor.kind(), ActorKind::Other, "{raw:?}");
            assert!(!actor.is_conventional(), "{raw:?}");
            assert_eq!(actor.as_str(), raw);
            assert_eq!(actor.id(), raw.trim());
        }
    }

    #[test]
    fn display_round_trips_the_raw_string() {
        for s in ["human:walter", "reference_agent/gemini-2.5-pro", "anything"] {
            assert_eq!(Actor::parse(s).to_string(), s);
        }
    }
}
