//! Markdown link extraction, classification, and path resolution.
//!
//! OKF relationships are expressed as ordinary markdown links, so this module
//! provides a small scanner for inline `[text](dest)` links and
//! reference-style links, plus the resolution rules that links and
//! frontmatter *path fields* share. It ignores links inside fenced code blocks
//! and inline code spans, which are content rather than relationships.
//!
//! ## Resolution
//!
//! A link target or a path field value resolves the same way:
//!
//! 1. A value with a URI scheme (for example `https:`) is an absolute URL.
//! 2. A value that starts with `/` resolves from the bundle root.
//! 3. Any other value resolves from the directory of the file that contains
//!    it.
//!
//! Both in-bundle forms are valid, and neither is preferred. A value that
//! resolves to a location outside the bundle root is external and is never
//! read from the local file system. Targets are URI references, so a
//! `#fragment` or `?query` is not part of the path and a space is written
//! `%20`; paths are case-sensitive.
//!
//! The path fields are `resource`, `sources[].resource`, `computation`,
//! `executor.resource`, `attester.resource`, and `superseded_by`. They are
//! resolved by [`field_path_candidates`]; links are resolved to concept ids by
//! [`Link::resolve`].
//!
//! Earlier versions of the specification wrote path fields from the bundle
//! root without a leading `/`. [`legacy_root_path_candidates`] gives that
//! reading, which a consumer may fall back to only for a bundle that declares
//! `okf_version` 0.2 or earlier, or no version. It never applies to links.
//!
//! This module also still parses the v0.1 body `# Citations` list
//! ([`extract_citations`]), which `sources` superseded but which consumers MAY
//! keep reading for legacy documents.

use crate::concept_id::ConceptId;
use crate::markdown::{
    clean_destination, code_free_lines, is_escaped, normalize_reference_label, parse_inline_link,
    parse_reference_definition, parse_reference_link, reference_definitions,
};
use std::collections::HashMap;
use std::fmt::{self, Write as _};

/// How a link target is interpreted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LinkKind {
    /// Begins with `/`: resolved from the bundle root.
    Absolute,
    /// Any other in-bundle path, such as `./other.md`: resolved from the
    /// directory of the file that contains it.
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
        // Strip the fragment and query before decoding. Otherwise a filename
        // containing an encoded `%23` would turn into `#` and be mistaken for
        // the fragment delimiter on the second candidate.
        let target = path_part(&self.target);
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

    /// The bundle-relative file paths this link may name, most likely first,
    /// with their extension kept. Empty for an external link, an in-document
    /// anchor, or a target that leaves the bundle root.
    ///
    /// Unlike [`Link::resolve`], which answers "which concept", this answers
    /// "which file": a link may validly point at an asset, a directory, or a
    /// markdown file that is not a concept.
    #[must_use]
    pub fn path_candidates(&self, source: &ConceptId) -> Vec<String> {
        field_path_candidates(&self.target, source)
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
/// The `sources` frontmatter field and footnote attribution superseded the
/// body citations list in v0.2; consumers MAY still parse this form for
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

/// Percent-encodes one path segment for use in a link target or a path field.
///
/// Targets are URI references, so a space or another character that would
/// end or split the reference must be percent-encoded. Only those are
/// encoded: letters, digits, and non-ASCII text are left readable.
#[must_use]
pub fn encode_segment(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for c in segment.chars() {
        let reserved = c.is_ascii_control()
            || matches!(
                c,
                ' ' | '%'
                    | '#'
                    | '?'
                    | '"'
                    | '<'
                    | '>'
                    | '('
                    | ')'
                    | '['
                    | ']'
                    | '{'
                    | '}'
                    | '|'
                    | '\\'
                    | '^'
                    | '`'
            );
        if reserved {
            let mut buffer = [0u8; 4];
            for byte in c.encode_utf8(&mut buffer).bytes() {
                // Writing to a `String` cannot fail.
                let _ = write!(out, "%{byte:02X}");
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The `/`-rooted target of a concept: `/tables/orders.md`, with each segment
/// percent-encoded where it has to be.
#[must_use]
pub fn rooted_target(concept: &ConceptId) -> String {
    let segments: Vec<String> = concept
        .segments()
        .iter()
        .map(|segment| encode_segment(segment))
        .collect();
    format!("/{}.md", segments.join("/"))
}

/// The path of a URI reference: the target with its `#fragment` and `?query`
/// removed. A `?` or `#` that belongs to a file name must be percent-encoded.
#[must_use]
pub fn path_part(target: &str) -> &str {
    let without_fragment = strip_anchor(target);
    without_fragment
        .find('?')
        .map_or(without_fragment, |i| &without_fragment[..i])
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
    let base = source.directory().to_vec();
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

/// Resolves a link target or a **path field** value to the bundle-relative
/// paths it may name, most likely first.
///
/// The value resolves from the bundle root when it starts with `/`, and
/// otherwise from the directory of `from`, the concept that contains it. A URL
/// (any value with a URI scheme), an in-document anchor, and a path that
/// leaves the bundle root yield an empty vector: there is nothing in the
/// bundle to resolve, and a consumer must not go looking outside it.
///
/// A percent-encoded value has two readings, the literal one and the decoded
/// one, and both are returned so that a file genuinely named `my%20notes.md`
/// still resolves by its literal spelling. Callers should take the first
/// candidate that exists.
///
/// Unlike [`Link::resolve`], the returned paths keep their file extension:
/// these fields routinely name non-markdown files such as
/// `/references/attesters/revenue.py`.
#[must_use]
pub fn field_path_candidates(raw: &str, from: &ConceptId) -> Vec<String> {
    let base = from.directory().to_vec();
    path_candidates_in(raw, &base)
}

/// As [`field_path_candidates`], for a value in a file that is not a concept.
///
/// `dir` is the bundle-relative directory of the file that contains the value
/// (empty for the bundle root). This is how the `license` key of a
/// bundle-root `index.md` resolves.
#[must_use]
pub fn path_candidates_in(raw: &str, dir: &[String]) -> Vec<String> {
    let target = raw.trim();
    let base: &[String] = match Link::classify(target) {
        LinkKind::Absolute => &[],
        LinkKind::Relative => dir,
        _ => return Vec::new(),
    };
    readings(target)
        .into_iter()
        .filter_map(|reading| normalize_segments(&reading, base))
        .map(|segments| segments.join("/"))
        .fold(Vec::new(), |mut out, path| {
            if !path.is_empty() && !out.contains(&path) {
                out.push(path);
            }
            out
        })
}

/// The reading earlier OKF versions gave a path field written without a
/// leading `/`: resolved from the bundle root rather than from the directory
/// of its file.
///
/// The v0.2 examples wrote `executor.resource: references/skills/run-on-bq.md`
/// on a concept in `computations/`, meaning the bundle root. A consumer MAY
/// fall back to these candidates when the value does not resolve from its
/// file, and only for a bundle that declares `okf_version` 0.2 or earlier or
/// declares no version. Returns an empty vector for a URL, an anchor, or a
/// value that already starts with `/`.
#[must_use]
pub fn legacy_root_path_candidates(raw: &str) -> Vec<String> {
    let target = raw.trim();
    if Link::classify(target) != LinkKind::Relative {
        return Vec::new();
    }
    path_candidates_in(&format!("/{target}"), &[])
}

/// The literal and percent-decoded readings of a target's path.
fn readings(target: &str) -> Vec<String> {
    let path = path_part(target);
    let mut out = vec![path.to_string()];
    if let Some(decoded) = percent_decode(path)
        && decoded != path
    {
        out.push(decoded);
    }
    out
}

/// The concept id a bundle-relative markdown path denotes, or `None` if the
/// path is not a `.md` file or is not a valid id.
#[must_use]
pub fn concept_id_for_path(path: &str) -> Option<ConceptId> {
    let stem = path.strip_suffix(".md")?;
    ConceptId::parse(stem).ok()
}

/// Extracts all markdown links from a body, skipping fenced code blocks
/// and inline code spans.
///
/// Both link syntaxes that count as links are found: inline `[text](dest)`
/// links, and reference-style links whose label has a `[label]: dest`
/// definition in the body. A bracketed label with no definition is plain text,
/// and `[[wikilinks]]` are not links.
#[must_use]
pub fn extract_links(body: &str) -> Vec<Link> {
    let definitions: HashMap<String, String> = reference_definitions(body)
        .into_iter()
        .map(|definition| {
            (
                normalize_reference_label(&definition.label),
                clean_destination(&definition.destination),
            )
        })
        .collect();

    let mut links = Vec::new();
    for (_, line) in code_free_lines(body) {
        if parse_reference_definition(&line).is_some() {
            continue;
        }
        scan_line_links(&line, &definitions, &mut links);
    }
    links
}

/// Scans a single (code-free) line for inline and reference-style links.
fn scan_line_links(line: &str, definitions: &HashMap<String, String>, out: &mut Vec<Link>) {
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '[' && !is_escaped(&chars, i) {
            if let Some((text, dest, next)) = parse_inline_link(&chars, i) {
                let target = clean_destination(&dest);
                out.push(Link {
                    text,
                    kind: Link::classify(&target),
                    target,
                });
                i = next;
                continue;
            }
            if !definitions.is_empty()
                && let Some((text, label, next)) = parse_reference_link(&chars, i)
                && let Some(target) = definitions.get(&normalize_reference_label(&label))
            {
                out.push(Link {
                    text,
                    kind: Link::classify(target),
                    target: target.clone(),
                });
                i = next;
                continue;
            }
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
