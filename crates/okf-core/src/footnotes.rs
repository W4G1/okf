//! Markdown footnotes, the carrier for per-claim attribution.
//!
//! v0.2 retires the v0.1 body `# Citations` list. To attribute a specific claim
//! to a source, a producer writes a footnote whose **label is a
//! `sources[].id`**:
//!
//! ```markdown
//! The `events_` table is sharded daily as `events_YYYYMMDD`.[^ga4-schema]
//!
//! [^ga4-schema]: GA4 BigQuery Export schema
//! ```
//!
//! The label is the join key into `sources`; consumers resolve attribution
//! through the matching entry, not by parsing the footnote prose. Labels are
//! keyed rather than positional precisely because agents reorder these lists.
//!
//! This module only finds the footnotes. Joining them to `sources` is
//! [`provenance::attributions`](crate::provenance::attributions).

use crate::markdown::code_free_lines;

/// A `[^label]` reference in the body prose.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FootnoteRef {
    /// The label between `[^` and `]`.
    pub label: String,
    /// 1-based line number within the body.
    pub line: usize,
}

/// A `[^label]: text` definition line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FootnoteDef {
    /// The label between `[^` and `]:`.
    pub label: String,
    /// The prose after the colon.
    pub text: String,
    /// 1-based line number within the body.
    pub line: usize,
}

/// Extracts every `[^label]` reference from the body prose.
///
/// Definition lines are not counted as references to themselves, and footnotes
/// inside fenced code blocks or inline code spans are ignored, since those
/// are content, not attribution.
#[must_use]
pub fn extract_refs(body: &str) -> Vec<FootnoteRef> {
    let mut out = Vec::new();
    for (line_no, line) in code_free_lines(body) {
        // A definition line's own `[^label]:` marker is not a reference, but
        // anything after it may be.
        let scan = match split_definition(&line) {
            Some((_, rest_offset)) => &line[rest_offset..],
            None => line.as_str(),
        };
        for label in scan_labels(scan) {
            out.push(FootnoteRef {
                label,
                line: line_no,
            });
        }
    }
    out
}

/// Extracts every `[^label]: text` definition from the body.
#[must_use]
pub fn extract_definitions(body: &str) -> Vec<FootnoteDef> {
    let mut out = Vec::new();
    for (line_no, line) in code_free_lines(body) {
        if let Some((label, rest_offset)) = split_definition(&line) {
            out.push(FootnoteDef {
                label,
                text: line[rest_offset..].trim().to_string(),
                line: line_no,
            });
        }
    }
    out
}

/// If `line` is a footnote definition, returns its label and the byte offset of
/// the text after `]:`.
fn split_definition(line: &str) -> Option<(String, usize)> {
    let indent = line.len() - line.trim_start().len();
    // Definitions may be indented up to three spaces, like other block markers.
    if indent > 3 {
        return None;
    }
    let rest = &line[indent..];
    let inner = rest.strip_prefix("[^")?;
    let close = inner.find(']')?;
    let label = inner[..close].trim();
    if label.is_empty() {
        return None;
    }
    let after = &inner[close + 1..];
    let text = after.strip_prefix(':')?;
    Some((label.to_string(), line.len() - text.len()))
}

/// Collects the labels of every `[^label]` occurrence in a single line.
fn scan_labels(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = line.as_bytes();
    let mut i = 0;
    while i + 2 < bytes.len() {
        if bytes[i] == b'['
            && bytes[i + 1] == b'^'
            && let Some(close) = line[i + 2..].find(']')
        {
            let label = line[i + 2..i + 2 + close].trim();
            if !label.is_empty() && !label.contains('[') {
                out.push(label.to_string());
                i += 2 + close + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = "\
# Computation

    SELECT SUM(amount) FROM t WHERE year = @year

Recognized revenue per the recognition policy,[^rev-policy] corroborated by
the executive revenue dashboard.[^exec-rev-dash]

[^rev-policy]: Revenue recognition policy
[^exec-rev-dash]: Executive revenue dashboard
";

    #[test]
    fn refs_and_definitions_are_separated() {
        let refs: Vec<String> = extract_refs(BODY).into_iter().map(|r| r.label).collect();
        assert_eq!(refs, vec!["rev-policy", "exec-rev-dash"]);

        let defs = extract_definitions(BODY);
        assert_eq!(defs.len(), 2);
        assert_eq!(defs[0].label, "rev-policy");
        assert_eq!(defs[0].text, "Revenue recognition policy");
        assert_eq!(defs[1].label, "exec-rev-dash");
    }

    #[test]
    fn footnotes_in_code_are_ignored() {
        let body = "Real.[^a]\n\n```\nCode [^b] here.\n```\n\nInline `[^c]` too.\n";
        let refs: Vec<String> = extract_refs(body).into_iter().map(|r| r.label).collect();
        assert_eq!(refs, vec!["a"]);
    }

    #[test]
    fn multiple_refs_on_one_line() {
        let refs = extract_refs("Both [^a] and [^b] apply.\n");
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].line, 1);
        assert_eq!(refs[1].label, "b");
    }

    #[test]
    fn markdown_links_are_not_footnotes() {
        assert!(extract_refs("See [customers](/tables/customers.md).").is_empty());
        assert!(extract_definitions("[label]: https://example.com").is_empty());
    }
}
