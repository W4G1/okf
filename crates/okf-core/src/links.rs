//! Markdown link extraction, classification, and path-valued fields.
//!
//! OKF relationships are expressed as ordinary markdown links, so this module
//! provides a small scanner for inline `[text](dest)` links
//! plus the link-classification rules (absolute bundle-relative vs.
//! relative vs. external). It ignores links inside fenced code blocks and
//! inline code spans, which are content rather than relationships.
//!
//! The same path grammar extends to *frontmatter* fields (`resource`,
//! `sources[].resource`, `computation`, `executor.resource`, and
//! `attester.resource`), which are resolved by
//! [`field_path_candidates`] rather than by [`Link::resolve`].
//!
//! It also still parses the v0.1 body `# Citations` list
//! ([`extract_citations`]), which v0.2 supersedes with `sources` but
//! which consumers MAY keep reading for legacy documents.

use crate::concept_id::ConceptId;
use crate::markdown::{clean_destination, code_free_lines, is_escaped, parse_inline_link};
use std::fmt;

/// How a link target is interpreted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LinkKind {
    /// Begins with `/`: resolved relative to the bundle root (recommended).
    Absolute,
    /// A relative path such as `./other.md`.
    Relative,
    /// An external URI (`https://…`, `mailto:…`, …).
    External,
    /// A pure in-document anchor (`#section`).
    Anchor,
    /// Anything else (e.g. an empty target).
    Other,
}

impl LinkKind {
    /// Returns the string representation of this link kind.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Absolute => "absolute",
            Self::Relative => "relative",
            Self::External => "external",
            Self::Anchor => "anchor",
            Self::Other => "other",
        }
    }
}

impl fmt::Display for LinkKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl AsRef<str> for LinkKind {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

/// Error returned when a string cannot be parsed into a [`LinkKind`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseLinkKindError(pub String);

impl fmt::Display for ParseLinkKindError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown link kind: {:?}", self.0)
    }
}

impl std::error::Error for ParseLinkKindError {}

impl std::str::FromStr for LinkKind {
    type Err = ParseLinkKindError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "absolute" => Ok(Self::Absolute),
            "relative" => Ok(Self::Relative),
            "external" => Ok(Self::External),
            "anchor" => Ok(Self::Anchor),
            "other" => Ok(Self::Other),
            other => Err(ParseLinkKindError(other.to_string())),
        }
    }
}

/// A markdown link found in a concept body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Link {
    /// The link text (between `[` and `]`).
    pub text: String,
    /// The raw destination (between `(` and `)`), with any title removed.
    pub target: String,
    /// The classification of [`Link::target`].
    pub kind: LinkKind,
}

impl Link {
    /// Classifies a raw target string.
    #[must_use]
    pub fn classify(target: &str) -> LinkKind {
        let t = target.trim();
        if t.is_empty() {
            LinkKind::Other
        } else if t.starts_with('#') {
            LinkKind::Anchor
        } else if is_external(t) {
            LinkKind::External
        } else if t.starts_with('/') {
            LinkKind::Absolute
        } else {
            LinkKind::Relative
        }
    }

    /// Resolves an internal link to the concept id it points at, given the id
    /// of the concept the link appears in.
    ///
    /// Returns `None` for external links, anchors, links to directories
    /// (targets ending in `/`), or targets that cannot form a valid concept id.
    /// The result is *not* guaranteed to exist in the bundle: broken links are
    /// permitted by the spec.
    ///
    /// Where a target is percent-encoded this returns the literal reading; use
    /// [`Link::resolve_all`] to also consider the decoded one.
    #[must_use]
    pub fn resolve(&self, source: &ConceptId) -> Option<ConceptId> {
        self.resolve_all(source).into_iter().next()
    }

    /// Every concept id this link may denote, most likely first.
    ///
    /// A markdown destination is a URL, so a concept whose filename contains a
    /// space is normally linked as `/tables/my%20notes.md`. Decoding is offered
    /// as a second candidate rather than applied outright, so that a file
    /// genuinely named `my%20notes.md` still resolves by its literal spelling.
    /// Callers should prefer the first candidate that exists in the bundle.
    #[must_use]
    pub fn resolve_all(&self, source: &ConceptId) -> Vec<ConceptId> {
        let mut out = Vec::new();
        let mut push = |target: &str| {
            let id = match self.kind {
                LinkKind::Absolute => resolve_absolute_path(target),
                LinkKind::Relative => resolve_relative_path(target, source),
                _ => None,
            };
            if let Some(id) = id
                && !out.contains(&id)
            {
                out.push(id);
            }
        };
        // Strip an anchor before decoding. Otherwise a filename containing an
        // encoded `%23` would turn into `#` and be mistaken for the anchor
        // delimiter on the second candidate.
        let target = strip_anchor(&self.target);
        push(target);
        if let Some(decoded) = percent_decode(target) {
            push(&decoded);
        }
        out
    }

    /// Returns the target with any anchor fragment removed.
    #[must_use]
    pub fn target_without_anchor(&self) -> &str {
        strip_anchor(&self.target)
    }

    /// Returns the anchor fragment, if one was present.
    #[must_use]
    pub fn anchor(&self) -> Option<&str> {
        self.target.find('#').map(|i| &self.target[i + 1..])
    }
}

/// Percent-decodes a link destination, or `None` if there is nothing to decode
/// or the result is not valid UTF-8.
fn percent_decode(s: &str) -> Option<String> {
    if !s.contains('%') {
        return None;
    }
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut decoded_any = false;
    let mut i = 0;
    while i < bytes.len() {
        let escape = (bytes[i] == b'%' && i + 3 <= bytes.len())
            .then(|| &bytes[i + 1..i + 3])
            .filter(|hex| hex.iter().all(u8::is_ascii_hexdigit));
        if let Some(hex) = escape {
            let hex = std::str::from_utf8(hex).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            decoded_any = true;
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    if !decoded_any {
        return None;
    }
    String::from_utf8(out).ok()
}

/// A numbered entry under a legacy v0.1 `# Citations` heading.
///
/// v0.2 supersedes the body citations list with the `sources` frontmatter field
/// and footnote attribution; consumers MAY still parse this form for
/// v0.1 documents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Citation {
    /// The citation number (the `n` in `[n]`).
    pub number: u32,
    /// The link text, if the entry is a markdown link.
    pub text: Option<String>,
    /// The cited URL/target, if present.
    pub target: Option<String>,
    /// The full raw text of the entry after the `[n]` marker.
    pub raw: String,
}

/// Whether a target names something outside the bundle.
///
/// Any RFC-3986 scheme prefix counts, not just `http`. The spec calls `resource`
/// "a URI that uniquely identifies the underlying asset", and producers do use
/// non-http schemes for warehouse assets (`bigquery:project.dataset.table`);
/// treating those as relative paths would have a consumer looking for a file
/// that was never meant to exist.
fn is_external(t: &str) -> bool {
    t.starts_with("//") /* protocol-relative URL */ || has_uri_scheme(t)
}

/// Matches `scheme:` where scheme is `ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )`.
fn has_uri_scheme(t: &str) -> bool {
    let Some((scheme, _)) = t.split_once(':') else {
        return false;
    };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

fn strip_anchor(target: &str) -> &str {
    target.find('#').map_or(target, |i| &target[..i])
}

fn resolve_absolute_path(t: &str) -> Option<ConceptId> {
    if t.ends_with('/') {
        return None; // directory link
    }
    // Normalize `.`/`..` segments relative to the bundle root, consistent with
    // relative-link resolution.
    normalize_segments(t, &[])
        .and_then(strip_md)
        .and_then(|segs| ConceptId::new(segs).ok())
}

fn resolve_relative_path(t: &str, source: &ConceptId) -> Option<ConceptId> {
    if t.is_empty() || t.ends_with('/') {
        return None;
    }
    // Start from the source concept's directory.
    let base = source
        .parent()
        .map(|p| p.segments().to_vec())
        .unwrap_or_default();
    normalize_segments(t, &base)
        .and_then(strip_md)
        .and_then(|segs| ConceptId::new(segs).ok())
}

/// Resolves `.`/`..`/empty components in a `/`-separated path against `base`.
///
/// A `..` at the bundle root is invalid rather than being allowed to disappear:
/// silently popping an empty vector would make `../x.md` from a root concept
/// point at `x.md` inside the bundle.
fn normalize_segments(path: &str, base: &[String]) -> Option<Vec<String>> {
    let mut segs = base.to_vec();
    for comp in path.split('/') {
        match comp {
            "" | "." => {}
            ".." => {
                segs.pop()?;
            }
            other => segs.push(other.to_string()),
        }
    }
    Some(segs)
}

/// Drops a trailing `.md` from the last segment, or `None` if there are none.
fn strip_md(mut segs: Vec<String>) -> Option<Vec<String>> {
    let last = segs.last_mut()?;
    if let Some(s) = last.strip_suffix(".md") {
        *last = s.to_string();
    }
    Some(segs)
}

/// Normalizes a **path-valued frontmatter field** into the
/// bundle-relative paths it might name, most likely first.
///
/// `resource`, `sources[].resource`, `computation`, `executor.resource`, and
/// `attester.resource` all accept an absolute URL, a bundle-relative path
/// beginning with `/`, or a relative path. URLs (and anchors) yield an empty
/// vector, since there is nothing in the bundle to resolve.
///
/// A relative path yields **two** candidates, because the spec uses both
/// readings: one reading treats `../computations/revenue.md` relative to the concept,
/// while the `references/` convention is written from the bundle root
/// (`executor.resource: references/skills/run-on-bq.md` on a concept that lives
/// in `computations/`). Callers should take the first candidate that exists.
///
/// Unlike [`Link::resolve`], the returned paths keep their file extension:
/// these fields routinely name non-markdown files such as
/// `references/attesters/revenue.py`.
#[must_use]
pub fn field_path_candidates(raw: &str, from: &ConceptId) -> Vec<String> {
    let target = raw.trim();
    match Link::classify(target) {
        LinkKind::Absolute => normalize_segments(strip_anchor(target), &[])
            .map(|segments| segments.join("/"))
            .into_iter()
            .collect(),
        LinkKind::Relative => {
            let base = from
                .parent()
                .map(|p| p.segments().to_vec())
                .unwrap_or_default();
            let stripped = strip_anchor(target);
            let mut out = Vec::new();
            if let Some(path) = normalize_segments(stripped, &base) {
                out.push(path.join("/"));
            }
            if let Some(path) = normalize_segments(stripped, &[]) {
                let from_root = path.join("/");
                if !out.contains(&from_root) {
                    out.push(from_root);
                }
            }
            out.retain(|p| !p.is_empty());
            out
        }
        _ => Vec::new(),
    }
}

/// The concept id a bundle-relative markdown path denotes, or `None` if the
/// path is not a `.md` file or is not a valid id.
#[must_use]
pub fn concept_id_for_path(path: &str) -> Option<ConceptId> {
    let stem = path.strip_suffix(".md")?;
    ConceptId::parse(stem).ok()
}

/// Extracts all inline markdown links from a body, skipping fenced code blocks
/// and inline code spans.
#[must_use]
pub fn extract_links(body: &str) -> Vec<Link> {
    let mut links = Vec::new();
    for (_, line) in code_free_lines(body) {
        scan_line_links(&line, &mut links);
    }
    links
}

/// Scans a single (code-free) line for `[text](dest)` links.
fn scan_line_links(line: &str, out: &mut Vec<Link>) {
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '['
            && !is_escaped(&chars, i)
            && let Some((text, dest, next)) = parse_inline_link(&chars, i)
        {
            let target = clean_destination(&dest);
            out.push(Link {
                text,
                kind: Link::classify(&target),
                target,
            });
            i = next;
            continue;
        }
        i += 1;
    }
}

/// Extracts numbered citation entries from the `# Citations` section.
#[must_use]
pub fn extract_citations(body: &str) -> Vec<Citation> {
    let mut out = Vec::new();
    let mut in_section = false;
    for line in body.lines() {
        let trimmed = line.trim();
        if let Some(heading) = trimmed.strip_prefix('#') {
            let title = heading.trim_start_matches('#').trim();
            if in_section {
                // A new heading ends the citations section.
                break;
            }
            in_section = title.eq_ignore_ascii_case("citations");
            continue;
        }
        if !in_section || trimmed.is_empty() {
            continue;
        }
        if let Some(cit) = parse_citation_line(trimmed) {
            out.push(cit);
        }
    }
    out
}

/// Parses a single `[n] …` citation line.
fn parse_citation_line(line: &str) -> Option<Citation> {
    let rest = line.strip_prefix('[')?;
    let close = rest.find(']')?;
    let number: u32 = rest[..close].trim().parse().ok()?;
    let after = rest[close + 1..].trim().to_string();

    // If the remainder is itself a markdown link, capture its text and target.
    let mut text = None;
    let mut target = None;
    let chars: Vec<char> = after.chars().collect();
    if let Some(open) = chars.iter().position(|&c| c == '[')
        && let Some((t, dest, _)) = parse_inline_link(&chars, open)
    {
        text = Some(t);
        target = Some(clean_destination(&dest));
    }
    Some(Citation {
        number,
        text,
        target,
        raw: after,
    })
}
