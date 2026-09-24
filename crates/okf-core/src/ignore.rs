//! Ignore-file grammar and matching (`.okfignore`, `.gitignore`, ...).
//!
//! An OKF bundle often lives inside a larger repository, next to drafts,
//! vendored trees, and build output that will never be published. This module
//! implements the gitignore pattern grammar on the standard library alone so a
//! bundle walk can leave those files out:
//!
//! - blank lines and `#` comments are skipped; `\#` is a literal `#`;
//! - a leading `!` negates; `\!` is a literal `!`;
//! - a trailing `/` matches directories only;
//! - a pattern containing `/` is *anchored* to the ignore file's directory;
//!   one without matches the last path component at any depth;
//! - `*`, `?`, `[...]` (with ranges and `!`/`^` negation), `\` escapes, and
//!   the three `**` forms;
//! - case-sensitive everywhere; later patterns override earlier ones.
//!
//! [`IgnoreRules`] is one parsed file. [`IgnoreMatcher`] is a precedence-
//! ordered stack of them, built by the walker in [`crate::walk`] as it
//! descends. [`IgnoreConfig`] names the extra ignore files a caller wants
//! honoured on top of the always-on `.okfignore`.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

/// The bundle-native ignore file, honoured by every walk.
pub const OKFIGNORE: &str = ".okfignore";

/// The Docker ignore file, read with [`Flavor::Docker`] semantics.
pub const DOCKERIGNORE: &str = ".dockerignore";

/// What every bundle walk ignores before any ignore file is consulted.
///
/// Hidden directories (which covers `.git`, `.hg`, `.svn`), `target`, and
/// `node_modules`, at any depth. These are the directories `okf fmt` and
/// `--fix` always skipped; every other command now agrees with them. The
/// rules sit at the lowest precedence, so a `!` pattern in `.okfignore` or
/// any `--ignore` source re-includes a directory (for example `!.notes/`).
/// Hidden *files* are not matched.
pub const DEFAULT_IGNORE_PATTERNS: &str = ".*/\ntarget/\nnode_modules/\n";

/// Which dialect an ignore file is read with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Flavor {
    /// Full gitignore semantics: unanchored bare names match at any depth,
    /// and the file is discovered per directory.
    Gitignore,
    /// Docker semantics: every pattern is anchored to the file's directory,
    /// and the file is read only at the bundle root.
    Docker,
}

/// One glob token within a path segment.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Tok {
    /// `*`: any run of characters, including none.
    Any,
    /// `?`: exactly one character.
    One,
    /// `[...]`: one character from the class.
    Class {
        negated: bool,
        ranges: Vec<(char, char)>,
    },
    /// A literal character.
    Char(char),
}

/// One `/`-separated segment of a compiled pattern.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Segment {
    /// A segment with no glob metacharacters.
    Literal(String),
    /// A segment with glob tokens.
    Glob(Vec<Tok>),
    /// A bare `**`: zero or more whole path components.
    DoubleStar,
}

impl Segment {
    fn matches(&self, component: &str) -> bool {
        match self {
            Self::Literal(lit) => lit == component,
            Self::Glob(toks) => glob_match(toks, component),
            Self::DoubleStar => true,
        }
    }
}

/// One compiled pattern line.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Pattern {
    negated: bool,
    dir_only: bool,
    anchored: bool,
    segments: Vec<Segment>,
}

impl Pattern {
    /// Compiles one line, or `None` for a blank, comment, or malformed line.
    fn parse(line: &str, flavor: Flavor) -> Option<Self> {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() || line.starts_with('#') {
            return None;
        }
        let line = strip_trailing_spaces(line);
        if line.is_empty() {
            return None;
        }

        let (negated, rest) = line.strip_prefix('!').map_or((false, line), |r| (true, r));
        let (dir_only, rest) = rest.strip_suffix('/').map_or((false, rest), |r| (true, r));
        if rest.is_empty() {
            return None;
        }
        let anchored = flavor == Flavor::Docker || rest.contains('/');
        let rest = rest.strip_prefix('/').unwrap_or(rest);
        if rest.is_empty() {
            return None;
        }

        let mut segments = Vec::new();
        for seg in rest.split('/') {
            if seg.is_empty() {
                return None;
            }
            segments.push(compile_segment(seg)?);
        }

        Some(Self {
            negated,
            dir_only,
            anchored,
            segments,
        })
    }

    /// Whether this pattern matches a path given as components relative to
    /// the rule set's base directory.
    fn matches(&self, comps: &[&str], is_dir: bool) -> bool {
        if comps.is_empty() || (self.dir_only && !is_dir) {
            return false;
        }
        if !self.anchored && self.segments.len() == 1 {
            // A bare name matches the final component at any depth.
            return self.segments[0].matches(comps[comps.len() - 1]);
        }
        match_segments(&self.segments, comps)
    }
}

/// Strips trailing spaces unless the last one is escaped with `\`.
fn strip_trailing_spaces(line: &str) -> &str {
    let mut end = line.len();
    let bytes = line.as_bytes();
    while end > 0 && bytes[end - 1] == b' ' {
        if end >= 2 && bytes[end - 2] == b'\\' {
            break;
        }
        end -= 1;
    }
    &line[..end]
}

/// Compiles one `/`-free segment. Returns `None` for malformed globs.
fn compile_segment(seg: &str) -> Option<Segment> {
    if seg == "**" {
        return Some(Segment::DoubleStar);
    }
    let mut toks: Vec<Tok> = Vec::new();
    let mut literal = String::new();
    let mut is_literal = true;
    let mut chars = seg.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                let escaped = chars.next()?;
                literal.push(escaped);
                toks.push(Tok::Char(escaped));
            }
            '*' => {
                is_literal = false;
                // Collapse runs of `*` (including `**` inside a segment).
                if toks.last() != Some(&Tok::Any) {
                    toks.push(Tok::Any);
                }
            }
            '?' => {
                is_literal = false;
                toks.push(Tok::One);
            }
            '[' => {
                is_literal = false;
                toks.push(compile_class(&mut chars)?);
            }
            other => {
                literal.push(other);
                toks.push(Tok::Char(other));
            }
        }
    }
    if is_literal {
        return Some(Segment::Literal(literal));
    }
    Some(Segment::Glob(toks))
}

/// Compiles a `[...]` class; the opening `[` has been consumed. Returns
/// `None` if the class is unterminated.
fn compile_class(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Option<Tok> {
    let negated = if matches!(chars.peek(), Some('!' | '^')) {
        chars.next();
        true
    } else {
        false
    };
    let mut ranges: Vec<(char, char)> = Vec::new();
    let mut first = true;
    loop {
        let c = chars.next()?;
        // A `]` immediately after `[` or `[!` is a literal.
        if c == ']' && !first {
            break;
        }
        first = false;
        let lo = if c == '\\' { chars.next()? } else { c };
        // Range `a-z`, unless `-` is the last char before `]`.
        if chars.peek() == Some(&'-') {
            let mut look = chars.clone();
            look.next();
            match look.peek() {
                Some(']') | None => ranges.push((lo, lo)),
                Some(_) => {
                    chars.next(); // '-'
                    let hi_raw = chars.next()?;
                    let hi = if hi_raw == '\\' {
                        chars.next()?
                    } else {
                        hi_raw
                    };
                    ranges.push((lo, hi));
                }
            }
        } else {
            ranges.push((lo, lo));
        }
    }
    Some(Tok::Class { negated, ranges })
}

/// Matches glob tokens against one path component with backtracking on `*`.
fn glob_match(toks: &[Tok], s: &str) -> bool {
    let chars: Vec<char> = s.chars().collect();
    let mut ti = 0;
    let mut ci = 0;
    let mut star: Option<(usize, usize)> = None; // (tok index after *, char index)

    while ci < chars.len() {
        match toks.get(ti) {
            Some(Tok::Any) => {
                star = Some((ti + 1, ci));
                ti += 1;
            }
            Some(tok) if tok_matches(tok, chars[ci]) => {
                ti += 1;
                ci += 1;
            }
            _ => match star {
                Some((st, sc)) => {
                    ti = st;
                    ci = sc + 1;
                    star = Some((st, sc + 1));
                }
                None => return false,
            },
        }
    }
    while toks.get(ti) == Some(&Tok::Any) {
        ti += 1;
    }
    ti == toks.len()
}

fn tok_matches(tok: &Tok, c: char) -> bool {
    match tok {
        Tok::Any | Tok::One => true,
        Tok::Char(x) => *x == c,
        Tok::Class { negated, ranges } => {
            let inside = ranges.iter().any(|(lo, hi)| *lo <= c && c <= *hi);
            inside != *negated
        }
    }
}

/// Matches segments against path components; `**` consumes zero or more.
///
/// Tracking which component positions are still reachable, rather than
/// recursing into every split a `**` allows, keeps the work proportional to
/// segments times components. Trying the splits instead would let a pattern
/// with several `**` segments and a suffix that never matches explore a
/// combinatorial number of paths, so an ignore file could make a walk of a
/// deep tree arbitrarily slow.
fn match_segments(segs: &[Segment], comps: &[&str]) -> bool {
    // `reach[i]`: the first `i` components can be consumed by the segments
    // considered so far.
    let mut reach = vec![false; comps.len() + 1];
    let mut next = vec![false; comps.len() + 1];
    reach[0] = true;

    for (i, seg) in segs.iter().enumerate() {
        if matches!(seg, Segment::DoubleStar) {
            // `**` consumes zero or more components, so everything from the
            // earliest reachable position onwards becomes reachable.
            let Some(first) = reach.iter().position(|&r| r) else {
                return false;
            };
            // A trailing `/**` matches everything *inside* a directory, not
            // the directory itself.
            if i + 1 == segs.len() {
                return first < comps.len();
            }
            reach[first..].fill(true);
            continue;
        }

        next.fill(false);
        for (j, comp) in comps.iter().enumerate() {
            if reach[j] && seg.matches(comp) {
                next[j + 1] = true;
            }
        }
        std::mem::swap(&mut reach, &mut next);
    }

    reach[comps.len()]
}

/// One parsed ignore file, with the directory its rules are relative to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IgnoreRules {
    base: PathBuf,
    patterns: Vec<Pattern>,
}

impl IgnoreRules {
    /// Parses ignore-file text. Rules are relative to `base`, the directory
    /// that contains the file. Malformed lines are skipped, never an error.
    pub fn parse(text: &str, base: impl Into<PathBuf>, flavor: Flavor) -> Self {
        let patterns = text
            .lines()
            .filter_map(|l| Pattern::parse(l, flavor))
            .collect();
        Self {
            base: base.into(),
            patterns,
        }
    }

    /// Reads and parses an ignore file. Rules are relative to the file's
    /// parent directory.
    ///
    /// # Errors
    ///
    /// Returns the underlying [`io::Error`] if the file cannot be read.
    pub fn read(path: &Path, flavor: Flavor) -> io::Result<Self> {
        let text = fs::read_to_string(path)?;
        let base = path.parent().unwrap_or_else(|| Path::new("")).to_path_buf();
        Ok(Self::parse(&text, base, flavor))
    }

    /// The default rules every walk starts from: [`DEFAULT_IGNORE_PATTERNS`]
    /// anchored at `base`, the bundle root.
    #[must_use]
    pub fn builtin(base: impl Into<PathBuf>) -> Self {
        Self::parse(DEFAULT_IGNORE_PATTERNS, base, Flavor::Gitignore)
    }

    /// The directory these rules are relative to.
    #[must_use]
    pub fn base(&self) -> &Path {
        &self.base
    }

    /// Whether this rule set has no patterns.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    /// Number of compiled patterns.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.patterns.len()
    }

    /// The verdict of this rule set alone for `path`: `Some(true)` ignored,
    /// `Some(false)` explicitly re-included, `None` no pattern matched.
    fn verdict(&self, path: &Path, is_dir: bool) -> Option<bool> {
        let rel = path.strip_prefix(&self.base).ok()?;
        let comps = components(rel)?;
        if comps.is_empty() {
            return None;
        }
        self.patterns
            .iter()
            .rev()
            .find(|p| p.matches(&comps, is_dir))
            .map(|p| !p.negated)
    }
}

/// Path components as `&str`, or `None` if any is not UTF-8 or is not a
/// normal component.
fn components(rel: &Path) -> Option<Vec<&str>> {
    let mut out = Vec::new();
    for c in rel.components() {
        match c {
            Component::Normal(os) => out.push(os.to_str()?),
            Component::CurDir => {}
            _ => return None,
        }
    }
    Some(out)
}

/// A precedence-ordered stack of rule sets for one walk.
///
/// Layers are pushed shallow-to-deep (ancestors, then the bundle root, then
/// each directory entered), and within one directory in the order built-in,
/// configured sources, `.okfignore`. The last layer pushed has the highest
/// precedence, and within a layer the last matching pattern wins.
#[derive(Clone, Debug, Default)]
pub struct IgnoreMatcher {
    layers: Vec<IgnoreRules>,
}

impl IgnoreMatcher {
    /// An empty matcher that ignores nothing.
    #[must_use]
    pub const fn new() -> Self {
        Self { layers: Vec::new() }
    }

    /// Pushes a rule set with the highest precedence so far.
    pub fn push(&mut self, rules: IgnoreRules) {
        self.layers.push(rules);
    }

    /// Pops the most recently pushed rule set.
    pub fn pop(&mut self) -> Option<IgnoreRules> {
        self.layers.pop()
    }

    /// Number of layers currently pushed.
    #[must_use]
    pub const fn depth(&self) -> usize {
        self.layers.len()
    }

    /// Whether `path` (in the same absolute form as the layer bases) is
    /// ignored. An entry is ignored if it, or any ancestor directory, is
    /// matched by an ignoring pattern that no higher-precedence negation
    /// overrides; this is how an ignored directory prunes its subtree.
    #[must_use]
    pub fn is_ignored(&self, path: &Path, is_dir: bool) -> bool {
        // Walk prefixes shallow-to-deep so an ignored parent prunes children
        // even when a deeper negation would otherwise re-include them.
        let mut prefixes: Vec<&Path> = path.ancestors().collect();
        prefixes.reverse();
        let last = prefixes.len().saturating_sub(1);
        prefixes.iter().enumerate().any(|(i, prefix)| {
            let prefix_is_dir = i < last || is_dir;
            self.direct_verdict(prefix, prefix_is_dir) == Some(true)
        })
    }

    fn direct_verdict(&self, path: &Path, is_dir: bool) -> Option<bool> {
        self.layers
            .iter()
            .rev()
            .find_map(|layer| layer.verdict(path, is_dir))
    }
}

/// One extra ignore source a caller wants honoured.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum IgnoreSource {
    /// An ignore-file *name* (`.gitignore`), discovered in every directory
    /// walked and in ancestors up to the repository root.
    Name(String),
    /// An explicit ignore-file *path*, read once and anchored to its parent
    /// directory.
    File(PathBuf),
}

impl IgnoreSource {
    /// Interprets a command-line value: anything containing a path separator
    /// is a [`IgnoreSource::File`], otherwise a [`IgnoreSource::Name`].
    #[must_use]
    pub fn parse_cli(raw: &str) -> Self {
        if raw.contains('/') || raw.contains(std::path::MAIN_SEPARATOR) {
            Self::File(PathBuf::from(raw))
        } else {
            Self::Name(raw.to_string())
        }
    }

    /// The file name this source is looking for.
    #[must_use]
    pub fn file_name(&self) -> Option<&str> {
        match self {
            Self::Name(n) => Some(n.as_str()),
            Self::File(p) => p.file_name().and_then(|n| n.to_str()),
        }
    }

    /// The dialect this source is read with: Docker for `.dockerignore`,
    /// gitignore for everything else.
    #[must_use]
    pub fn flavor(&self) -> Flavor {
        if self.file_name() == Some(DOCKERIGNORE) {
            Flavor::Docker
        } else {
            Flavor::Gitignore
        }
    }
}

/// The ignore sources a walk honours on top of the built-in rules and
/// `.okfignore`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IgnoreConfig {
    /// Extra sources, applied in order after the built-in rules and before
    /// `.okfignore`.
    pub sources: Vec<IgnoreSource>,
}

impl IgnoreConfig {
    /// A configuration with no extra sources.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            sources: Vec::new(),
        }
    }

    /// Adds a source.
    #[must_use]
    pub fn with_source(mut self, source: IgnoreSource) -> Self {
        self.sources.push(source);
        self
    }

    /// Adds an ignore-file name.
    #[must_use]
    pub fn with_name(self, name: impl Into<String>) -> Self {
        self.with_source(IgnoreSource::Name(name.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(text: &str) -> IgnoreRules {
        IgnoreRules::parse(text, "/b", Flavor::Gitignore)
    }

    fn matcher(text: &str) -> IgnoreMatcher {
        let mut m = IgnoreMatcher::new();
        m.push(rules(text));
        m
    }

    fn ign(m: &IgnoreMatcher, rel: &str, is_dir: bool) -> bool {
        m.is_ignored(&Path::new("/b").join(rel), is_dir)
    }

    #[test]
    fn blank_and_comment_lines_are_skipped() {
        let r = rules("\n# comment\n   \n");
        assert!(r.is_empty());
    }

    #[test]
    fn escaped_hash_is_literal() {
        let m = matcher("\\#notes.md");
        assert!(ign(&m, "#notes.md", false));
        assert!(!ign(&m, "notes.md", false));
    }

    #[test]
    fn trailing_spaces_stripped_unless_escaped() {
        let m = matcher("a.md   \nb\\ .md");
        assert!(ign(&m, "a.md", false));
        assert!(ign(&m, "b .md", false));
        assert!(!ign(&m, "b.md", false));
    }

    #[test]
    fn escaped_bang_is_literal() {
        let m = matcher("\\!important.md");
        assert!(ign(&m, "!important.md", false));
    }

    #[test]
    fn directory_only_pattern() {
        let m = matcher("drafts/");
        assert!(ign(&m, "drafts", true));
        assert!(ign(&m, "drafts/a.md", false));
        assert!(ign(&m, "drafts/x/b.md", false));
        assert!(!ign(&m, "notes/drafts.md", false));
        assert!(!ign(&m, "drafts", false));
    }

    #[test]
    fn unanchored_glob_matches_at_any_depth() {
        let m = matcher("*.draft.md");
        assert!(ign(&m, "a.draft.md", false));
        assert!(ign(&m, "deep/b.draft.md", false));
        assert!(!ign(&m, "a.md", false));
    }

    #[test]
    fn leading_slash_anchors() {
        let m = matcher("/README.md");
        assert!(ign(&m, "README.md", false));
        assert!(!ign(&m, "sub/README.md", false));
    }

    #[test]
    fn bare_name_matches_component_exactly() {
        let m = matcher("tmp");
        assert!(ign(&m, "tmp", true));
        assert!(ign(&m, "tmp", false));
        assert!(ign(&m, "x/tmp/y.md", false));
        assert!(!ign(&m, "tmp.md", false));
    }

    #[test]
    fn last_match_wins() {
        let m = matcher("*.md\n!keep.md");
        assert!(!ign(&m, "keep.md", false));
        assert!(ign(&m, "other.md", false));
    }

    #[test]
    fn double_star_prefix() {
        let m = matcher("**/generated");
        assert!(ign(&m, "generated", true));
        assert!(ign(&m, "a/generated", true));
        assert!(ign(&m, "a/b/generated", false));
        assert!(!ign(&m, "a/generated.md", false));
    }

    #[test]
    fn double_star_suffix_and_middle() {
        let m = matcher("build/**\na/**/z.md");
        assert!(ign(&m, "build/x.md", false));
        assert!(ign(&m, "build/y/x.md", false));
        assert!(!ign(&m, "build", true));
        assert!(ign(&m, "a/z.md", false));
        assert!(ign(&m, "a/b/z.md", false));
        assert!(ign(&m, "a/b/c/z.md", false));
        assert!(!ign(&m, "b/z.md", false));
    }

    #[test]
    fn many_double_stars_match_without_backtracking() {
        // Every `**` here could consume any number of components, so trying
        // the splits would explore them combinatorially. The suffix never
        // matches, which is the worst case.
        let pattern = "**/".repeat(20) + "zzz.md";
        let m = matcher(&pattern);
        let dirs = "d/".repeat(20);
        assert!(!ign(&m, &format!("{dirs}a.md"), false));
        assert!(ign(&m, &format!("{dirs}zzz.md"), false));
    }

    #[test]
    fn inner_double_star_acts_as_star() {
        let m = matcher("a**b.md");
        assert!(ign(&m, "ab.md", false));
        assert!(ign(&m, "axyzb.md", false));
        assert!(!ign(&m, "a/b.md", false));
    }

    #[test]
    fn question_mark_and_classes() {
        let m = matcher("v?.md\n[a-c]*.md\n[!x]z.md\n[]]y.md");
        assert!(ign(&m, "v1.md", false));
        assert!(!ign(&m, "v12.md", false));
        assert!(ign(&m, "banana.md", false));
        assert!(!ign(&m, "danana.md", false));
        assert!(ign(&m, "az.md", false));
        assert!(!ign(&m, "xz.md", false));
        assert!(ign(&m, "]y.md", false));
    }

    #[test]
    fn star_does_not_cross_slash() {
        let m = matcher("docs/*.md");
        assert!(ign(&m, "docs/a.md", false));
        assert!(!ign(&m, "docs/x/a.md", false));
    }

    #[test]
    fn case_sensitive() {
        let m = matcher("Draft.md");
        assert!(ign(&m, "Draft.md", false));
        assert!(!ign(&m, "draft.md", false));
    }

    #[test]
    fn malformed_pattern_is_skipped() {
        let r = rules("[");
        assert_eq!(r.len(), 0);
        let r = rules("a[b\ngood.md\ntrailing\\");
        assert_eq!(r.len(), 1);
        let m = matcher("[");
        assert!(!ign(&m, "anything.md", false));
    }

    #[test]
    fn negation_cannot_rescue_child_of_ignored_dir() {
        let m = matcher("build\n!build/keep.md");
        assert!(ign(&m, "build/keep.md", false));
    }

    #[test]
    fn deeper_layer_overrides_shallower() {
        let mut m = IgnoreMatcher::new();
        m.push(IgnoreRules::parse("*.wip.md", "/b", Flavor::Gitignore));
        m.push(IgnoreRules::parse(
            "!*.wip.md",
            "/b/team",
            Flavor::Gitignore,
        ));
        assert!(!m.is_ignored(Path::new("/b/team/a.wip.md"), false));
        assert!(m.is_ignored(Path::new("/b/a.wip.md"), false));
    }

    #[test]
    fn later_source_in_same_dir_overrides_earlier() {
        let mut m = IgnoreMatcher::new();
        m.push(IgnoreRules::parse("secret.md", "/b", Flavor::Gitignore));
        m.push(IgnoreRules::parse("!secret.md", "/b", Flavor::Gitignore));
        assert!(!m.is_ignored(Path::new("/b/secret.md"), false));
    }

    #[test]
    fn layer_does_not_apply_outside_its_base() {
        let mut m = IgnoreMatcher::new();
        m.push(IgnoreRules::parse("*.md", "/b/sub", Flavor::Gitignore));
        assert!(!m.is_ignored(Path::new("/b/a.md"), false));
        assert!(m.is_ignored(Path::new("/b/sub/a.md"), false));
    }

    #[test]
    fn ancestor_anchored_pattern() {
        let mut m = IgnoreMatcher::new();
        m.push(IgnoreRules::parse(
            "docs/kb/private/",
            "/r",
            Flavor::Gitignore,
        ));
        assert!(m.is_ignored(Path::new("/r/docs/kb/private/a.md"), false));
        assert!(!m.is_ignored(Path::new("/r/docs/kb/public/a.md"), false));
    }

    #[test]
    fn docker_flavor_anchors_bare_names() {
        let mut m = IgnoreMatcher::new();
        m.push(IgnoreRules::parse("README.md", "/b", Flavor::Docker));
        assert!(m.is_ignored(Path::new("/b/README.md"), false));
        assert!(!m.is_ignored(Path::new("/b/sub/README.md"), false));
    }

    #[test]
    fn builtin_rules_skip_defaults_and_negation_reincludes() {
        let mut m = IgnoreMatcher::new();
        m.push(IgnoreRules::builtin("/b"));
        // Hidden directories at any depth, including VCS metadata.
        assert!(m.is_ignored(Path::new("/b/.git"), true));
        assert!(m.is_ignored(Path::new("/b/x/.hg"), true));
        assert!(m.is_ignored(Path::new("/b/.svn/notes.md"), false));
        assert!(m.is_ignored(Path::new("/b/.drafts/a.md"), false));
        // `target` and `node_modules` at any depth.
        assert!(m.is_ignored(Path::new("/b/node_modules/a.md"), false));
        assert!(m.is_ignored(Path::new("/b/x/target/out.md"), false));
        // Hidden *files* and near-miss names are not touched.
        assert!(!m.is_ignored(Path::new("/b/.hidden.md"), false));
        assert!(!m.is_ignored(Path::new("/b/targets/a.md"), false));

        // Any ignore file outranks the defaults.
        m.push(IgnoreRules::parse("!.drafts/", "/b", Flavor::Gitignore));
        assert!(!m.is_ignored(Path::new("/b/.drafts/a.md"), false));
        assert!(m.is_ignored(Path::new("/b/.other/a.md"), false));
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_component_never_matches() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let m = matcher("*");
        let bad = Path::new("/b").join(OsStr::from_bytes(b"\xff\xfe.md"));
        assert!(!m.is_ignored(&bad, false));
    }

    #[test]
    fn parse_cli_distinguishes_name_and_path() {
        assert_eq!(
            IgnoreSource::parse_cli(".gitignore"),
            IgnoreSource::Name(".gitignore".into())
        );
        assert_eq!(
            IgnoreSource::parse_cli("../shared.ignore"),
            IgnoreSource::File("../shared.ignore".into())
        );
        assert_eq!(
            IgnoreSource::parse_cli("./nope"),
            IgnoreSource::File("./nope".into())
        );
    }

    #[test]
    fn dockerignore_source_uses_docker_flavor() {
        assert_eq!(
            IgnoreSource::Name(".dockerignore".into()).flavor(),
            Flavor::Docker
        );
        assert_eq!(
            IgnoreSource::Name(".gitignore".into()).flavor(),
            Flavor::Gitignore
        );
        assert_eq!(
            IgnoreSource::File("x/.dockerignore".into()).flavor(),
            Flavor::Docker
        );
    }
}
