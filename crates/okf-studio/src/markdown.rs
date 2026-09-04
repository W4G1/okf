//! A purpose-built markdown → styled-lines renderer for the document pane.
//!
//! Not a general `CommonMark` engine: OKF bodies are deliberately simple, and
//! okf-core already ships the hard parts (heading extraction, link parsing,
//! footnote scanning). The renderer's extra job over plain text is the link
//! focus map: every rendered link and footnote reference records its line so
//! the viewer can Tab-cycle and Enter-follow them.

use crate::theme::{GLYPH_BROKEN, GLYPH_OK, Theme};
use okf_core::markdown::parse_heading_line;
use okf_core::{Link, LinkKind};
use okf_validator::{Language, check_syntax};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use std::collections::HashMap;

/// What a focusable element in the rendered document points at.
#[derive(Clone, Debug)]
pub enum FocusKind {
    /// A markdown link, classified.
    Link {
        /// The link text.
        text: String,
        /// The raw destination.
        target: String,
        /// The destination's classification.
        kind: LinkKind,
    },
    /// A `[^label]` footnote reference.
    Footnote(String),
}

/// One focusable element and the rendered line it starts on.
#[derive(Clone, Debug)]
pub struct FocusTarget {
    /// 0-based index into [`RenderedDoc::lines`].
    pub line: usize,
    /// What the element points at.
    pub kind: FocusKind,
}

/// A heading's position in the rendered output.
#[derive(Clone, Debug)]
pub struct HeadingPos {
    /// 0-based index into [`RenderedDoc::lines`].
    pub line: usize,
    /// Heading level (1–6).
    pub level: usize,
    /// The heading text.
    pub text: String,
}

/// The rendered document: styled lines plus the focus and jump maps.
#[derive(Clone, Debug, Default)]
pub struct RenderedDoc {
    /// The styled output, one entry per terminal row (before scrolling).
    pub lines: Vec<Line<'static>>,
    /// Focusable links and footnote refs, in document order.
    pub links: Vec<FocusTarget>,
    /// Headings, for the outline jump list.
    pub headings: Vec<HeadingPos>,
    /// Rendered line of each `[^label]:` definition, the Enter-jump target
    /// for a focused footnote reference.
    pub footnote_defs: HashMap<String, usize>,
}

/// Display width of a char: a small `wcwidth` approximation on std.
#[must_use]
pub const fn char_width(c: char) -> usize {
    let cp = c as u32;
    if c.is_control() {
        return 0;
    }
    let wide = matches!(
        cp,
        0x1100..=0x115F
            | 0x2E80..=0xA4CF
            | 0xAC00..=0xD7A3
            | 0xF900..=0xFAFF
            | 0xFE30..=0xFE4F
            | 0xFF00..=0xFF60
            | 0xFFE0..=0xFFE6
            | 0x1F300..=0x1FAFF
            | 0x20000..=0x3FFFD
    );
    if wide { 2 } else { 1 }
}

/// Display width of a string.
#[must_use]
pub fn str_width(s: &str) -> usize {
    s.chars().map(char_width).sum()
}

/// Truncates `s` to at most `max` display columns, appending `…` when cut.
#[must_use]
pub fn truncate_to_width(s: &str, max: usize) -> String {
    if str_width(s) <= max {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = char_width(c);
        if w + cw + 1 > max {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

/// An inline fragment: text with one style and an optional focus id.
#[derive(Clone, Debug)]
struct Frag {
    text: String,
    style: Style,
    focus: Option<usize>,
}

/// Renders a document body at a pane width.
///
/// `focused` selects which entry of the returned focus map renders with the
/// selection style; pass the previously returned map's index.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn render_document(
    body: &str,
    width: u16,
    theme: &Theme,
    focused: Option<usize>,
) -> RenderedDoc {
    let width = usize::from(width.max(10));
    let mut doc = RenderedDoc::default();
    let lines: Vec<&str> = body.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();

        // Fenced code block.
        if let Some(marker) = fence_marker(trimmed) {
            let lang_tag = trimmed[3..].trim().to_string();
            let mut code: Vec<&str> = Vec::new();
            let mut j = i + 1;
            while j < lines.len() && fence_marker(lines[j].trim_start()) != Some(marker) {
                code.push(lines[j]);
                j += 1;
            }
            render_code_block(&mut doc, &code, &lang_tag, width, *theme);
            i = if j < lines.len() { j + 1 } else { j };
            continue;
        }

        // Heading.
        if let Some((level, text)) = parse_heading_line(line) {
            let style = theme.accent().add_modifier(Modifier::BOLD);
            let indent = " ".repeat(level.saturating_sub(1));
            doc.headings.push(HeadingPos {
                line: doc.lines.len(),
                level,
                text: text.to_string(),
            });
            let frags = parse_inline(text, style, *theme, &mut doc, focused);
            let prefix = Span::styled(indent, Style::default());
            push_wrapped(
                &mut doc,
                &frags,
                width,
                std::slice::from_ref(&prefix),
                std::slice::from_ref(&prefix),
            );
            if level == 1 {
                doc.lines
                    .push(Line::from(Span::styled("─".repeat(width), theme.dim())));
            }
            i += 1;
            continue;
        }

        // Horizontal rule.
        if is_hr(trimmed) {
            doc.lines
                .push(Line::from(Span::styled("─".repeat(width), theme.dim())));
            i += 1;
            continue;
        }

        // Table: a run of `|`-prefixed lines.
        if trimmed.starts_with('|') {
            let mut j = i;
            while j < lines.len() && lines[j].trim_start().starts_with('|') {
                j += 1;
            }
            render_table(&mut doc, &lines[i..j], width, *theme);
            i = j;
            continue;
        }

        // Blank line.
        if trimmed.is_empty() {
            doc.lines.push(Line::default());
            i += 1;
            continue;
        }

        // Footnote definition.
        if let Some((label, rest)) = footnote_def(trimmed) {
            doc.footnote_defs.insert(label.clone(), doc.lines.len());
            let marker = Span::styled(format!("[^{label}] "), theme.accent());
            let cont = Span::styled(
                " ".repeat(str_width(&format!("[^{label}] "))),
                Style::default(),
            );
            let frags = parse_inline(rest, theme.dim(), *theme, &mut doc, focused);
            push_wrapped(
                &mut doc,
                &frags,
                width,
                std::slice::from_ref(&marker),
                std::slice::from_ref(&cont),
            );
            i += 1;
            continue;
        }

        // Block quote.
        if let Some(rest) = trimmed.strip_prefix('>') {
            let gutter = Span::styled("│ ", theme.dim());
            let frags = parse_inline(rest.trim_start(), theme.dim(), *theme, &mut doc, focused);
            push_wrapped(
                &mut doc,
                &frags,
                width,
                std::slice::from_ref(&gutter),
                std::slice::from_ref(&gutter),
            );
            i += 1;
            continue;
        }

        // List item (bullet, numbered, task).
        if let Some((marker, rest)) = list_marker(line) {
            let indent = line.len() - trimmed.len();
            let pad = " ".repeat(indent);
            let first = Span::styled(format!("{pad}{marker} "), theme.accent());
            let cont = Span::raw(" ".repeat(indent + str_width(&marker) + 1));
            let frags = parse_inline(rest, Style::default(), *theme, &mut doc, focused);
            push_wrapped(
                &mut doc,
                &frags,
                width,
                std::slice::from_ref(&first),
                std::slice::from_ref(&cont),
            );
            i += 1;
            continue;
        }

        // Paragraph line.
        let frags = parse_inline(line, Style::default(), *theme, &mut doc, focused);
        push_wrapped(&mut doc, &frags, width, &[], &[]);
        i += 1;
    }

    doc
}

fn fence_marker(trimmed: &str) -> Option<char> {
    if trimmed.starts_with("```") {
        Some('`')
    } else if trimmed.starts_with("~~~") {
        Some('~')
    } else {
        None
    }
}

fn is_hr(trimmed: &str) -> bool {
    trimmed.len() >= 3
        && (trimmed.chars().all(|c| c == '-')
            || trimmed.chars().all(|c| c == '*')
            || trimmed.chars().all(|c| c == '_'))
}

/// Parses `[^label]: rest` definitions.
fn footnote_def(trimmed: &str) -> Option<(String, &str)> {
    let inner = trimmed.strip_prefix("[^")?;
    let close = inner.find("]:")?;
    let label = inner[..close].trim();
    if label.is_empty() {
        return None;
    }
    Some((label.to_string(), inner[close + 2..].trim_start()))
}

/// Recognizes `- `, `* `, `+ `, `1. `, and task-list markers, returning the
/// rendered marker and the item text.
fn list_marker(line: &str) -> Option<(String, &str)> {
    let trimmed = line.trim_start();
    for bullet in ["- [ ] ", "* [ ] "] {
        if let Some(rest) = trimmed.strip_prefix(bullet) {
            return Some(("☐".to_string(), rest));
        }
    }
    for bullet in ["- [x] ", "* [x] ", "- [X] ", "* [X] "] {
        if let Some(rest) = trimmed.strip_prefix(bullet) {
            return Some(("☑".to_string(), rest));
        }
    }
    for bullet in ["- ", "* ", "+ "] {
        if let Some(rest) = trimmed.strip_prefix(bullet) {
            return Some(("•".to_string(), rest));
        }
    }
    // Numbered: digits then `. ` or `) `.
    let digits: String = trimmed.chars().take_while(char::is_ascii_digit).collect();
    if !digits.is_empty() {
        let rest = &trimmed[digits.len()..];
        if let Some(text) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")) {
            return Some((format!("{digits}."), text));
        }
    }
    None
}

/// Renders a fenced code block as a boxed region with the language tag and a
/// syntax verdict badge in the top border.
fn render_code_block(
    doc: &mut RenderedDoc,
    code: &[&str],
    lang_tag: &str,
    width: usize,
    theme: Theme,
) {
    let inner = width.saturating_sub(4).max(4);
    let source = code.join("\n");
    // No badge for an untagged block, an unknown language, or a language whose
    // parser is compiled out of this build: an unchecked block must not look
    // like a passing one.
    let verdict = if Language::from_tag(lang_tag).is_supported() {
        Some(check_syntax(lang_tag, &source))
    } else {
        None
    };
    let mut title = String::new();
    if !lang_tag.is_empty() {
        title.push_str(lang_tag);
    }
    let (badge, badge_style) = match &verdict {
        Some(Ok(())) => (format!(" syntax {GLYPH_OK}"), theme.ok()),
        Some(Err(e)) => (format!(" syntax {GLYPH_BROKEN} {e}"), theme.error()),
        None => (String::new(), theme.dim()),
    };
    let head = format!("┌ {title}");
    let head_width = str_width(&head) + str_width(&badge);
    let fill = width.saturating_sub(head_width + 2);
    doc.lines.push(Line::from(vec![
        Span::styled(head, theme.dim()),
        Span::styled(
            truncate_to_width(&badge, width.saturating_sub(4)),
            badge_style,
        ),
        Span::styled(format!(" {}", "─".repeat(fill)), theme.dim()),
    ]));
    for line in code {
        let text = truncate_to_width(line, inner);
        doc.lines.push(Line::from(vec![
            Span::styled("│ ", theme.dim()),
            Span::raw(text),
        ]));
    }
    doc.lines.push(Line::from(Span::styled(
        format!("└{}", "─".repeat(width.saturating_sub(1))),
        theme.dim(),
    )));
}

/// Renders a run of `|`-delimited rows: box-drawn when the columns fit the
/// pane, otherwise emitted as preformatted text.
fn render_table(doc: &mut RenderedDoc, rows: &[&str], width: usize, theme: Theme) {
    let parsed: Vec<Vec<String>> = rows
        .iter()
        .filter(|r| !is_table_separator(r))
        .map(|r| {
            r.trim()
                .trim_matches('|')
                .split('|')
                .map(|c| c.trim().to_string())
                .collect()
        })
        .collect();
    if parsed.is_empty() {
        return;
    }
    let cols = parsed.iter().map(Vec::len).max().unwrap_or(0);
    let mut widths = vec![0usize; cols];
    for row in &parsed {
        for (c, cell) in row.iter().enumerate() {
            widths[c] = widths[c].max(str_width(cell));
        }
    }
    let total: usize = widths.iter().sum::<usize>() + cols * 3 + 1;
    if total > width {
        // Preformatted fallback.
        for row in rows {
            doc.lines.push(Line::from(Span::raw((*row).to_string())));
        }
        return;
    }
    let rule = |l: &str, m: &str, r: &str| {
        let mut s = String::from(l);
        for (c, w) in widths.iter().enumerate() {
            s.push_str(&"─".repeat(w + 2));
            s.push_str(if c + 1 == cols { r } else { m });
        }
        Line::from(Span::styled(s, theme.dim()))
    };
    doc.lines.push(rule("┌", "┬", "┐"));
    for (r, row) in parsed.iter().enumerate() {
        let mut spans = vec![Span::styled("│", theme.dim())];
        for (c, w) in widths.iter().enumerate() {
            let cell = row.get(c).map_or("", String::as_str);
            let pad = w.saturating_sub(str_width(cell));
            let style = if r == 0 {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            spans.push(Span::styled(format!(" {cell}{} ", " ".repeat(pad)), style));
            spans.push(Span::styled("│", theme.dim()));
        }
        doc.lines.push(Line::from(spans));
        if r == 0 && parsed.len() > 1 {
            doc.lines.push(rule("├", "┼", "┤"));
        }
    }
    doc.lines.push(rule("└", "┴", "┘"));
}

fn is_table_separator(row: &str) -> bool {
    let t = row.trim().trim_matches('|');
    !t.is_empty() && t.chars().all(|c| matches!(c, '-' | ':' | '|' | ' '))
}

/// Parses inline markdown (bold, italic, code, links, footnote refs) into
/// styled fragments, registering focusable elements in `doc.links`.
#[allow(clippy::too_many_lines)]
fn parse_inline(
    text: &str,
    base: Style,
    theme: Theme,
    doc: &mut RenderedDoc,
    focused: Option<usize>,
) -> Vec<Frag> {
    let chars: Vec<char> = text.chars().collect();
    let mut frags: Vec<Frag> = Vec::new();
    let mut buf = String::new();
    let mut i = 0;
    let mut bold = false;
    let mut italic = false;

    let flush = |buf: &mut String, frags: &mut Vec<Frag>, bold: bool, italic: bool| {
        if buf.is_empty() {
            return;
        }
        let mut style = base;
        if bold {
            style = style.add_modifier(Modifier::BOLD);
        }
        if italic {
            style = style.add_modifier(Modifier::ITALIC);
        }
        frags.push(Frag {
            text: std::mem::take(buf),
            style,
            focus: None,
        });
    };

    while i < chars.len() {
        let c = chars[i];
        // Inline code span.
        if c == '`'
            && let Some(close) = chars[i + 1..].iter().position(|&x| x == '`')
        {
            flush(&mut buf, &mut frags, bold, italic);
            let code: String = chars[i + 1..i + 1 + close].iter().collect();
            frags.push(Frag {
                text: code,
                style: theme.accent().add_modifier(Modifier::DIM),
                focus: None,
            });
            i += close + 2;
            continue;
        }
        // Bold / italic toggles.
        if c == '*' {
            if chars.get(i + 1) == Some(&'*') {
                flush(&mut buf, &mut frags, bold, italic);
                bold = !bold;
                i += 2;
                continue;
            }
            flush(&mut buf, &mut frags, bold, italic);
            italic = !italic;
            i += 1;
            continue;
        }
        // Footnote reference.
        if c == '['
            && chars.get(i + 1) == Some(&'^')
            && let Some(close) = chars[i + 2..].iter().position(|&x| x == ']')
        {
            let label: String = chars[i + 2..i + 2 + close].iter().collect();
            if !label.is_empty() && chars.get(i + 2 + close + 1) != Some(&':') {
                flush(&mut buf, &mut frags, bold, italic);
                let focus_id = doc.links.len();
                doc.links.push(FocusTarget {
                    line: usize::MAX, // fixed up by push_wrapped
                    kind: FocusKind::Footnote(label.clone()),
                });
                let mut style = theme.accent();
                if focused == Some(focus_id) {
                    style = style.add_modifier(Modifier::REVERSED);
                }
                frags.push(Frag {
                    text: format!("[^{label}]"),
                    style,
                    focus: Some(focus_id),
                });
                i += 2 + close + 1;
                continue;
            }
        }
        // Markdown link.
        if c == '['
            && !okf_core::markdown::is_escaped(&chars, i)
            && let Some((ltext, dest, next)) = okf_core::markdown::parse_inline_link(&chars, i)
        {
            flush(&mut buf, &mut frags, bold, italic);
            let target = okf_core::markdown::clean_destination(&dest);
            let kind = Link::classify(&target);
            let focus_id = doc.links.len();
            doc.links.push(FocusTarget {
                line: usize::MAX,
                kind: FocusKind::Link {
                    text: ltext.clone(),
                    target: target.clone(),
                    kind,
                },
            });
            let (mut style, rendered) = if kind == LinkKind::External {
                (
                    theme.accent().add_modifier(Modifier::UNDERLINED),
                    ltext.clone(),
                )
            } else {
                (base.add_modifier(Modifier::UNDERLINED), format!("→{ltext}"))
            };
            if focused == Some(focus_id) {
                style = style.add_modifier(Modifier::REVERSED);
            }
            frags.push(Frag {
                text: rendered,
                style,
                focus: Some(focus_id),
            });
            i = next;
            continue;
        }
        buf.push(c);
        i += 1;
    }
    flush(&mut buf, &mut frags, bold, italic);
    frags
}

/// Word-wraps styled fragments to `width`, prefixing the first output line
/// with `first_prefix` and continuations with `cont_prefix`, and fixes up the
/// line index of any focus target the fragments carry.
fn push_wrapped(
    doc: &mut RenderedDoc,
    frags: &[Frag],
    width: usize,
    first_prefix: &[Span<'static>],
    cont_prefix: &[Span<'static>],
) {
    // Split fragments into atoms: words and whitespace runs, styles kept.
    struct Atom {
        text: String,
        style: Style,
        focus: Option<usize>,
        is_space: bool,
    }
    let mut atoms: Vec<Atom> = Vec::new();
    for frag in frags {
        let mut cur = String::new();
        let mut cur_space = None;
        for c in frag.text.chars() {
            let space = c == ' ';
            if cur_space != Some(space) && !cur.is_empty() {
                atoms.push(Atom {
                    text: std::mem::take(&mut cur),
                    style: frag.style,
                    focus: frag.focus,
                    is_space: cur_space == Some(true),
                });
            }
            cur_space = Some(space);
            cur.push(c);
        }
        if !cur.is_empty() {
            atoms.push(Atom {
                text: cur,
                style: frag.style,
                focus: frag.focus,
                is_space: cur_space == Some(true),
            });
        }
    }

    let prefix_width: usize = first_prefix.iter().map(|s| str_width(&s.content)).sum();
    let avail = width.saturating_sub(prefix_width).max(8);
    let mut lines: Vec<Vec<Span<'static>>> = vec![Vec::new()];
    let mut cur_width = 0usize;
    let mut focus_lines: Vec<(usize, usize)> = Vec::new();

    for atom in atoms {
        let w = str_width(&atom.text);
        if cur_width + w > avail && cur_width > 0 && !atom.is_space {
            lines.push(Vec::new());
            cur_width = 0;
        }
        if atom.is_space && cur_width == 0 && lines.last().is_some_and(Vec::is_empty) {
            continue; // drop leading spaces on wrapped lines
        }
        if let Some(f) = atom.focus {
            focus_lines.push((f, lines.len() - 1));
        }
        lines
            .last_mut()
            .expect("lines is never empty")
            .push(Span::styled(atom.text, atom.style));
        cur_width += w;
    }

    let base = doc.lines.len();
    for (idx, spans) in lines.into_iter().enumerate() {
        let mut full = if idx == 0 {
            first_prefix.to_vec()
        } else {
            cont_prefix.to_vec()
        };
        full.extend(spans);
        doc.lines.push(Line::from(full));
    }
    for (f, rel) in focus_lines {
        if let Some(target) = doc.links.get_mut(f) {
            target.line = base + rel;
        }
    }
}
