//! Markdown scanning, link rewriting, heading extraction, and anchor slugs.
//!
//! OKF documents use Markdown with YAML frontmatter. This module provides pure-Rust
//! Markdown utilities for inspecting and transforming Markdown content:
//!
//! - [`heading_slug`]: Generates kebab-case URL anchor slugs matching GitHub/CommonMark conventions.
//! - [`extract_headings`]: Extracts all headings in document order, skipping fenced code blocks.
//! - [`rewrite_markdown_links`]: Rewrites links while ignoring code blocks and inline code.
//!
//! Two link syntaxes are links in OKF: inline `[text](target)` links and
//! reference-style links (`[text][label]`, `[label][]`, or `[label]`, with a
//! `[label]: target` definition elsewhere in the body). Other syntaxes, such
//! as `[[wikilinks]]`, are not.

use crate::links::Link;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

/// A parsed markdown heading.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarkdownHeading<'a> {
    /// Heading level (1 for `#`, 2 for `##`, up to 6).
    pub level: usize,
    /// The trimmed heading text after `#...# `.
    pub text: &'a str,
    /// 1-based line number within the markdown text.
    pub line_num: usize,
    /// 0-based line index within lines.
    pub line_index: usize,
}

impl MarkdownHeading<'_> {
    /// Derives a standard Markdown anchor slug from this heading's text.
    #[must_use]
    pub fn slug(&self) -> String {
        heading_slug(self.text)
    }
}

/// Derives a standard Markdown anchor slug from a heading text.
///
/// Example: `"## Pricing Tiers"` -> `"pricing-tiers"`
#[must_use]
pub fn heading_slug(text: &str) -> String {
    let clean = text.trim().trim_start_matches('#').trim();
    let mut slug = String::with_capacity(clean.len());
    for c in clean.chars() {
        if c.is_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if (c.is_whitespace() || c == '-' || c == '_') && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.trim_matches('-').to_string()
}

/// Parses a single line as an ATX heading (`# ` through `###### `).
///
/// Returns `(level, text)` if the line is a heading, or `None` otherwise.
#[must_use]
pub fn parse_heading_line(line: &str) -> Option<(usize, &str)> {
    let t = line.trim_start();
    if !t.starts_with('#') {
        return None;
    }
    let count = t.chars().take_while(|&c| c == '#').count();
    if (1..=6).contains(&count) && t[count..].starts_with(' ') {
        Some((count, t[count..].trim()))
    } else {
        None
    }
}

/// Extracts all markdown headings in document order, ignoring headings inside fenced code blocks.
#[must_use]
pub fn extract_headings(markdown: &str) -> Vec<MarkdownHeading<'_>> {
    let mut headings = Vec::new();
    let mut fence: Option<char> = None;

    for (i, line) in markdown.lines().enumerate() {
        let trimmed_start = line.trim_start();
        if let Some(f) = fence {
            if trimmed_start.starts_with(&f.to_string().repeat(3)) {
                fence = None;
            }
            continue;
        }
        if trimmed_start.starts_with("```") {
            fence = Some('`');
            continue;
        }
        if trimmed_start.starts_with("~~~") {
            fence = Some('~');
            continue;
        }

        if let Some((level, text)) = parse_heading_line(line) {
            headings.push(MarkdownHeading {
                level,
                text,
                line_num: i + 1,
                line_index: i,
            });
        }
    }

    headings
}

/// Checks whether a heading text matches a search query by title, slug, or normalized spacing.
#[must_use]
pub fn matches_heading(heading_text: &str, query: &str) -> bool {
    let clean_query = query.trim().trim_start_matches('#').trim();
    let query_slug = heading_slug(clean_query);
    let title = heading_text.trim().trim_start_matches('#').trim();

    title.eq_ignore_ascii_case(clean_query)
        || heading_slug(title) == query_slug
        || title
            .replace(['-', '_'], " ")
            .eq_ignore_ascii_case(&clean_query.replace(['-', '_'], " "))
}

/// Removes an optional `"title"` (or `'title'`) suffix from a link destination.
#[must_use]
pub fn strip_title(dest: &str) -> String {
    let d = dest.trim();
    if let Some(idx) = d.find([' ', '\t']) {
        let (url, rest) = d.split_at(idx);
        let rest = rest.trim_start();
        if rest.starts_with('"') || rest.starts_with('\'') {
            return url.to_string();
        }
    }
    d.to_string()
}

/// Normalizes a raw link destination.
///
/// Unwraps the `CommonMark` `<...>` form, which is how a destination is allowed
/// to contain spaces, and otherwise removes an optional title suffix.
#[must_use]
pub fn clean_destination(dest: &str) -> String {
    let d = dest.trim();
    if let Some(rest) = d.strip_prefix('<')
        && let Some(end) = rest.find('>')
    {
        return rest[..end].to_string();
    }
    strip_title(d)
}

/// Extracts the title suffix part (e.g. ` "My Title"`) from a destination string.
#[must_use]
pub fn extract_title_suffix(dest: &str) -> String {
    let d = dest.trim();
    let after_dest = if let Some(rest) = d.strip_prefix('<')
        && let Some(end) = rest.find('>')
    {
        &rest[end + 1..]
    } else if let Some(idx) = d.find([' ', '\t']) {
        &d[idx..]
    } else {
        ""
    };
    let trimmed_suffix = after_dest.trim();
    if trimmed_suffix.starts_with('"') || trimmed_suffix.starts_with('\'') {
        format!(" {trimmed_suffix}")
    } else {
        String::new()
    }
}

/// Whether the character at `index` is preceded by an odd number of
/// backslashes, and is therefore escaped in Markdown.
#[must_use]
pub const fn is_escaped(chars: &[char], index: usize) -> bool {
    let mut backslashes = 0;
    let mut i = index;
    while i > 0 && chars[i - 1] == '\\' {
        backslashes += 1;
        i -= 1;
    }
    backslashes % 2 == 1
}

/// Attempts to parse `[text](dest)` starting at `start` (the `[`). Returns the
/// text, destination, and index just past the closing `)`.
#[must_use]
pub fn parse_inline_link(chars: &[char], start: usize) -> Option<(String, String, usize)> {
    let mut i = start + 1;
    let mut depth = 1;
    let text_start = i;
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 1, // skip escaped char
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
        i += 1;
    }
    if depth != 0 || i >= chars.len() {
        return None;
    }
    let text: String = chars[text_start..i].iter().collect();

    let mut j = i + 1;
    if j >= chars.len() || chars[j] != '(' {
        return None;
    }
    j += 1;
    let dest_start = j;
    let mut paren = 1;
    while j < chars.len() {
        match chars[j] {
            '\\' => j += 1,
            '(' => paren += 1,
            ')' => {
                paren -= 1;
                if paren == 0 {
                    break;
                }
            }
            _ => {}
        }
        j += 1;
    }
    if paren != 0 || j >= chars.len() {
        return None;
    }
    let dest: String = chars[dest_start..j].iter().collect();
    Some((text, dest, j + 1))
}

/// A link reference definition: `[label]: destination "optional title"`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReferenceDefinition {
    /// The label as written, without the brackets.
    pub label: String,
    /// The raw destination, including any title suffix.
    pub destination: String,
    /// 0-based index of the line the definition is on.
    pub line_index: usize,
}

/// Normalizes a reference label for matching: labels compare without regard
/// to case, and runs of whitespace count as one space.
#[must_use]
pub fn normalize_reference_label(label: &str) -> String {
    label
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Parses a line as a link reference definition, returning the label and the
/// raw destination (with any title).
///
/// A footnote definition (`[^label]: text`) is not a link reference
/// definition and returns `None`.
#[must_use]
pub fn parse_reference_definition(line: &str) -> Option<(&str, &str)> {
    let trimmed = line.trim_start();
    // Four or more columns of indentation makes an indented code block.
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let rest = trimmed.strip_prefix('[')?;
    let close = rest.find(']')?;
    let label = &rest[..close];
    if label.trim().is_empty() || label.starts_with('^') || label.contains('[') {
        return None;
    }
    let destination = rest[close + 1..].strip_prefix(':')?.trim();
    (!destination.is_empty()).then_some((label, destination))
}

/// Every link reference definition in a body, skipping fenced code blocks.
/// When a label is defined more than once, the first definition is the one
/// that applies, so later ones are left out.
#[must_use]
pub fn reference_definitions(body: &str) -> Vec<ReferenceDefinition> {
    // `code_free_lines` says which lines are outside fenced code, but it also
    // blanks inline code spans. The definition is taken from the line as
    // written, so that a title such as "the `orders` table" stays whole.
    let original: Vec<&str> = body.lines().collect();
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = Vec::new();
    for (line_number, _) in code_free_lines(body) {
        let Some(line) = original.get(line_number - 1) else {
            continue;
        };
        if let Some((label, destination)) = parse_reference_definition(line)
            && seen.insert(normalize_reference_label(label))
        {
            out.push(ReferenceDefinition {
                label: label.to_string(),
                destination: destination.to_string(),
                line_index: line_number - 1,
            });
        }
    }
    out
}

/// Attempts to parse a reference-style link starting at `start` (the `[`):
/// `[text][label]`, the collapsed `[label][]`, or the shortcut `[label]`.
///
/// Returns the link text, the label it refers to, and the index just past the
/// link. Whether the label is defined is for the caller to check; without a
/// definition the brackets are plain text. A footnote reference (`[^label]`)
/// and a definition line's own `[label]:` are not links.
#[must_use]
pub fn parse_reference_link(chars: &[char], start: usize) -> Option<(String, String, usize)> {
    let mut i = start + 1;
    let mut depth = 1;
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 1,
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            _ => {}
        }
        i += 1;
    }
    if depth != 0 || i >= chars.len() {
        return None;
    }
    let text: String = chars[start + 1..i].iter().collect();
    if text.trim().is_empty() || text.starts_with('^') {
        return None;
    }

    match chars.get(i + 1) {
        // `[text][label]` or `[label][]`.
        Some('[') => {
            let label_start = i + 2;
            let close = (label_start..chars.len()).find(|&j| chars[j] == ']')?;
            let label: String = chars[label_start..close].iter().collect();
            if label.starts_with('^') {
                return None;
            }
            let label = if label.trim().is_empty() {
                text.clone()
            } else {
                label
            };
            Some((text, label, close + 1))
        }
        // An inline link that did not parse, or a definition line.
        Some('(' | ':') => None,
        // The shortcut form.
        _ => Some((text.clone(), text, i + 1)),
    }
}

/// Replaces inline code spans (backtick-delimited) with spaces so links/footnotes inside
/// them are not extracted.
#[must_use]
pub fn blank_inline_code(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut in_code = false;
    for c in line.chars() {
        if c == '`' {
            in_code = !in_code;
            out.push(' ');
        } else if in_code {
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    out
}

/// Returns the body's lines, as `(1-based line number, text)`, with fenced
/// code blocks removed and inline code spans blanked out.
#[must_use]
pub fn code_free_lines(body: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut fence: Option<char> = None;
    for (i, line) in body.lines().enumerate() {
        let trimmed = line.trim_start();
        if let Some(f) = fence {
            if trimmed.starts_with(&f.to_string().repeat(3)) {
                fence = None;
            }
            continue;
        }
        if trimmed.starts_with("```") {
            fence = Some('`');
            continue;
        }
        if trimmed.starts_with("~~~") {
            fence = Some('~');
            continue;
        }
        out.push((i + 1, blank_inline_code(line)));
    }
    out
}

/// Action to perform on a detected markdown link during rewrite.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkRewriteAction {
    /// Keep the link destination as-is.
    Keep,
    /// Rewrite the destination URL to a new target (preserving title and brackets).
    Rewrite(String),
    /// Unlink: replace `[Text](dest)` with plain `Text`.
    Unlink,
}

/// Rewrites markdown links in a document body using a callback function.
///
/// Inline links are rewritten in place. A reference-style link is rewritten
/// through its definition line, which is where its destination lives; when
/// the callback unlinks a definition, the definition is removed and every use
/// of its label is replaced with the link text. The callback sees each
/// definition once, however many times its label is used.
///
/// Preserves fenced code blocks and inline code spans untouched.
pub fn rewrite_markdown_links<F>(body: &str, mut rewrite_fn: F) -> (String, usize)
where
    F: FnMut(&Link, &str) -> LinkRewriteAction,
{
    let mut total_rewritten = 0;

    // First pass: decide what happens to each reference definition, so that
    // the uses of an unlinked label can be flattened on the second pass.
    let mut definition_lines: HashMap<usize, Option<String>> = HashMap::new();
    let mut unlinked_labels: HashSet<String> = HashSet::new();
    for definition in reference_definitions(body) {
        let target = clean_destination(&definition.destination);
        let link = Link {
            text: definition.label.clone(),
            kind: Link::classify(&target),
            target,
        };
        match rewrite_fn(&link, &definition.destination) {
            LinkRewriteAction::Keep => {}
            LinkRewriteAction::Rewrite(new_dest) => {
                let title_suffix = extract_title_suffix(&definition.destination);
                let formatted_dest = if new_dest.contains(' ') && !new_dest.starts_with('<') {
                    format!("<{new_dest}>")
                } else {
                    new_dest
                };
                definition_lines.insert(
                    definition.line_index,
                    Some(format!(
                        "[{}]: {formatted_dest}{title_suffix}",
                        definition.label
                    )),
                );
                total_rewritten += 1;
            }
            LinkRewriteAction::Unlink => {
                definition_lines.insert(definition.line_index, None);
                unlinked_labels.insert(normalize_reference_label(&definition.label));
                total_rewritten += 1;
            }
        }
    }

    let mut out_lines = Vec::new();
    let mut fence: Option<char> = None;

    for (line_index, line) in body.lines().enumerate() {
        let trimmed_start = line.trim_start();
        if let Some(f) = fence {
            if trimmed_start.starts_with(&f.to_string().repeat(3)) {
                fence = None;
            }
            out_lines.push(line.to_string());
            continue;
        }
        if trimmed_start.starts_with("```") {
            fence = Some('`');
            out_lines.push(line.to_string());
            continue;
        }
        if trimmed_start.starts_with("~~~") {
            fence = Some('~');
            out_lines.push(line.to_string());
            continue;
        }

        match definition_lines.get(&line_index) {
            Some(Some(replacement)) => {
                let indent = &line[..line.len() - trimmed_start.len()];
                out_lines.push(format!("{indent}{replacement}"));
                continue;
            }
            Some(None) => continue,
            None => {}
        }

        let (new_line, count) = rewrite_line_links(line, &mut rewrite_fn, &unlinked_labels);
        total_rewritten += count;
        out_lines.push(new_line);
    }

    let mut result = out_lines.join("\n");
    if body.ends_with('\n') {
        result.push('\n');
    }
    (result, total_rewritten)
}

fn rewrite_line_links<F>(
    line_text: &str,
    rewrite_fn: &mut F,
    unlinked_labels: &HashSet<String>,
) -> (String, usize)
where
    F: FnMut(&Link, &str) -> LinkRewriteAction,
{
    let chars: Vec<char> = line_text.chars().collect();
    let mut out = String::with_capacity(line_text.len());
    let mut count = 0;
    let mut i = 0;
    let mut in_inline_code = false;

    while i < chars.len() {
        if chars[i] == '`' && !is_escaped(&chars, i) {
            in_inline_code = !in_inline_code;
            out.push('`');
            i += 1;
            continue;
        }

        if !in_inline_code
            && chars[i] == '['
            && !is_escaped(&chars, i)
            && let Some((text, dest_raw, next_i)) = parse_inline_link(&chars, i)
        {
            let target = clean_destination(&dest_raw);
            let link = Link {
                text: text.clone(),
                kind: Link::classify(&target),
                target,
            };

            match rewrite_fn(&link, &dest_raw) {
                LinkRewriteAction::Keep => {
                    let raw_slice: String = chars[i..next_i].iter().collect();
                    out.push_str(&raw_slice);
                }
                LinkRewriteAction::Rewrite(new_dest) => {
                    let title_suffix = extract_title_suffix(&dest_raw);
                    let formatted_dest = if new_dest.contains(' ') && !new_dest.starts_with('<') {
                        format!("<{new_dest}>")
                    } else {
                        new_dest
                    };
                    let _ = write!(out, "[{text}]({formatted_dest}{title_suffix})");
                    count += 1;
                }
                LinkRewriteAction::Unlink => {
                    out.push_str(&text);
                    count += 1;
                }
            }
            i = next_i;
            continue;
        }

        // A use of a reference label whose definition was unlinked becomes
        // plain text.
        if !in_inline_code
            && !unlinked_labels.is_empty()
            && chars[i] == '['
            && !is_escaped(&chars, i)
            && let Some((text, label, next_i)) = parse_reference_link(&chars, i)
            && unlinked_labels.contains(&normalize_reference_label(&label))
        {
            out.push_str(&text);
            i = next_i;
            continue;
        }

        out.push(chars[i]);
        i += 1;
    }

    (out, count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_heading_slug() {
        assert_eq!(heading_slug("Pricing Tiers"), "pricing-tiers");
        assert_eq!(heading_slug("## Deep Heading!"), "deep-heading");
        assert_eq!(
            heading_slug("Special_Chars & Symbols"),
            "special-chars-symbols"
        );
    }

    #[test]
    fn test_extract_headings() {
        let md = "\
# Top Heading

Some content.

```python
# Not a heading
pass
```

## Sub Heading

~~~bash
# Also not a heading
~~~

### Third Heading
";
        let headings = extract_headings(md);
        assert_eq!(headings.len(), 3);
        assert_eq!(headings[0].level, 1);
        assert_eq!(headings[0].text, "Top Heading");
        assert_eq!(headings[0].slug(), "top-heading");

        assert_eq!(headings[1].level, 2);
        assert_eq!(headings[1].text, "Sub Heading");
        assert_eq!(headings[1].slug(), "sub-heading");

        assert_eq!(headings[2].level, 3);
        assert_eq!(headings[2].text, "Third Heading");
        assert_eq!(headings[2].slug(), "third-heading");
    }

    #[test]
    fn test_rewrite_markdown_links() {
        let body = "\
# Title

See [User Guide](../guides/user.md) and [Profile](/users/profile.md#info).
Also `[Code Link](../not/a/link.md)` should not change.

```python
# [Python Link](../ignored.md)
pass
```
";

        let (rewritten, count) = rewrite_markdown_links(body, |link, _| {
            if link.target.starts_with("../guides/user.md") {
                LinkRewriteAction::Rewrite("../../docs/user.md".to_string())
            } else if link.target.starts_with("/users/profile.md") {
                LinkRewriteAction::Unlink
            } else {
                LinkRewriteAction::Keep
            }
        });

        assert_eq!(count, 2);
        assert!(rewritten.contains("[User Guide](../../docs/user.md)"));
        assert!(rewritten.contains("and Profile."));
        assert!(rewritten.contains("`[Code Link](../not/a/link.md)`"));
        assert!(rewritten.contains("# [Python Link](../ignored.md)"));
    }

    #[test]
    fn reference_definitions_are_found_and_footnotes_are_not() {
        let body = "\
See [the guide][guide] and [Orders][].

[guide]: ../guides/user.md \"User guide\"
[Orders]: </tables/my orders.md>
[^note]: A footnote, not a link definition.

```
[fenced]: ../ignored.md
```
";
        let definitions = reference_definitions(body);
        assert_eq!(definitions.len(), 2);
        assert_eq!(definitions[0].label, "guide");
        assert_eq!(
            definitions[0].destination,
            "../guides/user.md \"User guide\""
        );
        assert_eq!(definitions[0].line_index, 2);
        assert_eq!(
            clean_destination(&definitions[1].destination),
            "/tables/my orders.md"
        );
    }

    #[test]
    fn reference_links_parse_in_all_three_forms() {
        let parse = |s: &str| {
            let chars: Vec<char> = s.chars().collect();
            parse_reference_link(&chars, 0)
        };
        assert_eq!(
            parse("[the guide][guide] tail"),
            Some(("the guide".to_string(), "guide".to_string(), 18))
        );
        assert_eq!(
            parse("[Orders][]"),
            Some(("Orders".to_string(), "Orders".to_string(), 10))
        );
        assert_eq!(
            parse("[Orders] are"),
            Some(("Orders".to_string(), "Orders".to_string(), 8))
        );
        // Not links: a footnote, an inline link, and a definition.
        assert_eq!(parse("[^note]"), None);
        assert_eq!(parse("[text](dest)"), None);
        assert_eq!(parse("[label]: dest"), None);
        assert_eq!(normalize_reference_label("  The   Guide "), "the guide");
    }

    #[test]
    fn rewrites_reference_definitions_and_flattens_unlinked_ones() {
        let body = "\
See [the guide][guide], [Orders][] and [Legacy].

[guide]: ../guides/user.md \"User guide\"
[orders]: ../tables/orders.md
[legacy]: ../old/legacy.md
";
        let (rewritten, count) = rewrite_markdown_links(body, |link, _| {
            if link.target == "../guides/user.md" {
                LinkRewriteAction::Rewrite("../docs/user.md".to_string())
            } else if link.target == "../old/legacy.md" {
                LinkRewriteAction::Unlink
            } else {
                LinkRewriteAction::Keep
            }
        });
        assert_eq!(count, 2);
        assert!(rewritten.contains("[guide]: ../docs/user.md \"User guide\"\n"));
        assert!(rewritten.contains("[orders]: ../tables/orders.md\n"));
        assert!(!rewritten.contains("[legacy]:"));
        assert!(rewritten.contains("See [the guide][guide], [Orders][] and Legacy."));
    }
}
