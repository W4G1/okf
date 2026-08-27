//! Shared drawing helpers: centered rects, input fields, unicode bar charts,
//! and the date sparkline.

use crate::markdown::{str_width, truncate_to_width};
use crate::theme::Theme;
use okf_core::Date;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// A centered rect of at most `width`×`height` within `area`.
#[must_use]
pub fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width.saturating_sub(2));
    let h = height.min(area.height.saturating_sub(2));
    Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    }
}

/// Renders a labeled text-input line: `label ▏value▕` with a cursor block
/// when active.
#[must_use]
pub fn input_line(
    label: &str,
    value: &str,
    active: bool,
    theme: &Theme,
    width: usize,
) -> Line<'static> {
    let label_span = Span::styled(format!("{label} "), theme.dim());
    let avail = width.saturating_sub(str_width(label) + 4);
    let shown = if str_width(value) > avail {
        let mut s = String::new();
        let mut w = 0;
        for c in value.chars().rev() {
            let cw = crate::markdown::char_width(c);
            if w + cw > avail {
                break;
            }
            s.insert(0, c);
            w += cw;
        }
        format!("…{s}")
    } else {
        value.to_string()
    };
    let mut spans = vec![
        label_span,
        Span::styled("▏", theme.dim()),
        Span::styled(
            shown,
            if active {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            },
        ),
    ];
    if active {
        spans.push(Span::styled("█", theme.accent()));
    }
    spans.push(Span::styled("▕", theme.dim()));
    Line::from(spans)
}

/// A unicode block bar of `value / max`, `width` cells wide, with eighth
/// precision on the final cell.
#[must_use]
#[allow(clippy::cast_precision_loss)] // bar widths and counts are tiny
pub fn block_bar(value: usize, max: usize, width: usize) -> String {
    const EIGHTHS: [&str; 8] = ["▏", "▎", "▍", "▌", "▋", "▊", "▉", "█"];
    if max == 0 || width == 0 {
        return String::new();
    }
    let cells = value as f64 / max as f64 * width as f64;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let full = cells.floor() as usize;
    let mut bar = "█".repeat(full.min(width));
    if full < width {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let rem = ((cells - cells.floor()) * 8.0).round() as usize;
        if rem > 0 && value > 0 {
            bar.push_str(EIGHTHS[(rem - 1).min(7)]);
        }
    }
    bar
}

/// What one selectable distribution row shows.
pub struct BarRowSpec<'a> {
    /// The leading state glyph.
    pub glyph: &'a str,
    /// The glyph's (and bar's) style.
    pub glyph_style: Style,
    /// The row label.
    pub label: &'a str,
    /// The row's count.
    pub count: usize,
    /// The scale maximum.
    pub max: usize,
    /// The bar's width in cells.
    pub bar_width: usize,
    /// Whether the row is selected.
    pub selected: bool,
}

/// One selectable distribution row: `glyph label  count ███▌`.
#[must_use]
pub fn bar_row(spec: &BarRowSpec<'_>, theme: &Theme) -> Line<'static> {
    let style = if spec.selected {
        theme.selection()
    } else {
        Style::default()
    };
    Line::from(vec![
        Span::styled(format!("{} ", spec.glyph), spec.glyph_style),
        Span::styled(format!("{:<18}", truncate_to_width(spec.label, 18)), style),
        Span::styled(format!("{:>4} ", spec.count), style),
        Span::styled(
            block_bar(spec.count, spec.max, spec.bar_width),
            spec.glyph_style,
        ),
    ])
}

/// A sparkline over the last `days` days of a date-bucketed series.
#[must_use]
pub fn date_sparkline(
    series: &[(Date, usize)],
    today: Date,
    days: i64,
    theme: &Theme,
) -> Line<'static> {
    const LEVELS: [&str; 8] = ["▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
    let start = today.days_since_epoch() - days + 1;
    let mut buckets = vec![0usize; usize::try_from(days).unwrap_or(0)];
    for (date, count) in series {
        let offset = date.days_since_epoch() - start;
        if offset >= 0
            && let Ok(ix) = usize::try_from(offset)
            && ix < buckets.len()
        {
            buckets[ix] += count;
        }
    }
    let max = buckets.iter().copied().max().unwrap_or(0).max(1);
    let text: String = buckets
        .iter()
        .map(|&v| {
            if v == 0 {
                " "
            } else {
                LEVELS[(v * 7).div_ceil(max).min(7)]
            }
        })
        .collect();
    Line::from(Span::styled(text, theme.accent()))
}

/// A simple line diff for the fix preview: unchanged context around `-`/`+`
/// runs, computed by trimming the common prefix and suffix.
#[must_use]
pub fn simple_diff(before: &str, after: &str, theme: &Theme) -> Vec<Line<'static>> {
    let a: Vec<&str> = before.lines().collect();
    let b: Vec<&str> = after.lines().collect();
    let mut prefix = 0;
    while prefix < a.len() && prefix < b.len() && a[prefix] == b[prefix] {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < a.len() - prefix
        && suffix < b.len() - prefix
        && a[a.len() - 1 - suffix] == b[b.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let mut out = Vec::new();
    let context = 2usize;
    for line in &a[prefix.saturating_sub(context)..prefix] {
        out.push(Line::from(Span::styled(format!("  {line}"), theme.dim())));
    }
    for line in &a[prefix..a.len() - suffix] {
        out.push(Line::from(Span::styled(format!("- {line}"), theme.error())));
    }
    for line in &b[prefix..b.len() - suffix] {
        out.push(Line::from(Span::styled(format!("+ {line}"), theme.ok())));
    }
    for line in &a[a.len() - suffix..(a.len() - suffix + context).min(a.len())] {
        out.push(Line::from(Span::styled(format!("  {line}"), theme.dim())));
    }
    if out.is_empty() {
        out.push(Line::from(Span::styled("(no content change)", theme.dim())));
    }
    out
}

/// Highlights fuzzy-match positions within a label.
#[must_use]
pub fn highlight_match(
    label: &str,
    indices: &[usize],
    base: Style,
    theme: &Theme,
) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut buf = String::new();
    let mut matched = String::new();
    for (i, c) in label.chars().enumerate() {
        if indices.contains(&i) {
            if !buf.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut buf), base));
            }
            matched.push(c);
        } else {
            if !matched.is_empty() {
                spans.push(Span::styled(
                    std::mem::take(&mut matched),
                    theme.accent().add_modifier(Modifier::BOLD),
                ));
            }
            buf.push(c);
        }
    }
    if !matched.is_empty() {
        spans.push(Span::styled(
            matched,
            theme.accent().add_modifier(Modifier::BOLD),
        ));
    }
    if !buf.is_empty() {
        spans.push(Span::styled(buf, base));
    }
    spans
}
