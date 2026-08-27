//! Centered overlays: palette, diagnostics, help, confirmations, forms, the
//! fix-preview diff, the full log view, and the outline.

use crate::app::{
    App, DiagFilter, DiagnosticsState, FixPreviewState, NewConceptState, Overlay, PaletteMode,
    PaletteResults, PaletteState,
};
use crate::markdown::truncate_to_width;
use crate::theme::{GLYPH_BROKEN, GLYPH_FIXABLE, GLYPH_OK, GLYPH_WARN, tier_glyph};
use crate::ui::widgets::{centered, highlight_match, input_line, simple_diff};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

/// Draws one overlay (the refactor modal has its own module).
pub fn draw(frame: &mut Frame, app: &App, body: Rect, overlay: &Overlay) {
    match overlay {
        Overlay::Palette(state) => draw_palette(frame, app, body, state),
        Overlay::Diagnostics(state) => draw_diagnostics(frame, app, body, state),
        Overlay::Help(scroll) => draw_help(frame, app, body, *scroll),
        Overlay::Confirm(state) => {
            let height = u16::try_from(state.body.len()).unwrap_or(0) + 4;
            let area = centered(body, 60, height);
            let block = Block::new()
                .borders(Borders::ALL)
                .title(format!(" {} ", state.title))
                .title_bottom(" ⏎ Confirm   Esc Cancel ")
                .border_style(app.theme.accent());
            let inner = block.inner(area);
            frame.render_widget(Clear, area);
            frame.render_widget(block, area);
            let lines: Vec<Line<'static>> =
                state.body.iter().map(|l| Line::from(l.clone())).collect();
            frame.render_widget(Paragraph::new(lines), inner);
        }
        Overlay::DatePicker(state) => {
            let title = state.id.as_ref().map_or_else(
                || " Pin evaluation date (--today) ".to_string(),
                |id| format!(" Extend stale_after · {id} "),
            );
            let area = centered(body, 52, 6);
            let block = Block::new()
                .borders(Borders::ALL)
                .title(title)
                .title_bottom(" ⏎ Apply   ↑/↓ ±1d   PgUp/PgDn ±30d   Esc ")
                .border_style(app.theme.accent());
            let inner = block.inner(area);
            frame.render_widget(Clear, area);
            frame.render_widget(block, area);
            let valid = okf_core::Date::parse(state.input.trim()).is_some();
            let hint = if valid {
                Span::styled(format!(" {GLYPH_OK}"), app.theme.ok())
            } else if state.input.trim().is_empty() && state.id.is_none() {
                Span::styled(" empty = wall clock", app.theme.dim())
            } else {
                Span::styled(format!(" {GLYPH_BROKEN} YYYY-MM-DD"), app.theme.error())
            };
            let mut line = input_line("date", &state.input, true, &app.theme, 30);
            line.spans.push(hint);
            frame.render_widget(Paragraph::new(vec![Line::default(), line]), inner);
        }
        Overlay::NewConcept(state) => draw_new_concept(frame, app, body, state),
        Overlay::FixPreview(state) => draw_fix_preview(frame, app, body, state),
        Overlay::LogView(scroll) => draw_log_view(frame, app, body, *scroll),
        Overlay::Outline(state) => draw_outline(frame, app, body, state),
        Overlay::Refactor(_) => {}
    }
}

fn draw_palette(frame: &mut Frame, app: &App, body: Rect, state: &PaletteState) {
    let theme = &app.theme;
    let results = app.palette_results(state);
    let count = match &results {
        PaletteResults::Search(hits) => hits.len(),
        PaletteResults::Commands(commands) => commands.len(),
    };
    let height = u16::try_from(count.min(14)).unwrap_or(0) + 3;
    let area = Rect {
        x: body.x + body.width.saturating_sub(64) / 2,
        y: body.y + 1,
        width: 64.min(body.width),
        height: height.min(body.height),
    };
    let prompt = if state.mode() == PaletteMode::Command {
        "＞"
    } else {
        "⌕"
    };
    let block = Block::new()
        .borders(Borders::ALL)
        .title(format!(" {prompt} {} ", state.input))
        .title_bottom(" #tag type: tier: is:stale is:broken · > commands ")
        .border_style(theme.accent());
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);

    let sel = state.sel.min(count.saturating_sub(1));
    let mut lines: Vec<Line<'static>> = Vec::new();
    match results {
        PaletteResults::Search(hits) => {
            for (ix, hit) in hits.iter().enumerate().take(usize::from(inner.height)) {
                let snapshot = app.snapshot.as_ref();
                let meta = snapshot.and_then(|s| s.meta(&hit.id));
                let glyph = meta.map_or("○", |m| tier_glyph(m.tier));
                let glyph_style = meta.map_or_else(Style::default, |m| theme.tier(m.tier));
                let marker = if ix == sel { "▸ " } else { "  " };
                let mut spans = vec![
                    Span::raw(marker.to_string()),
                    Span::styled(format!("{glyph} "), glyph_style),
                ];
                if hit.heading.is_some() {
                    spans.push(Span::styled(format!("{} › ", hit.id), theme.dim()));
                    spans.extend(highlight_match(
                        &hit.label,
                        &hit.indices,
                        Style::default(),
                        theme,
                    ));
                    spans.push(Span::styled("  (heading)".to_string(), theme.dim()));
                } else if hit.label == hit.id.to_string() {
                    spans.extend(highlight_match(
                        &hit.label,
                        &hit.indices,
                        Style::default(),
                        theme,
                    ));
                } else {
                    spans.push(Span::raw(format!("{}  ", hit.id)));
                    spans.extend(highlight_match(
                        &hit.label,
                        &hit.indices,
                        theme.dim(),
                        theme,
                    ));
                }
                let mut line = Line::from(spans);
                if ix == sel {
                    line = line.style(theme.selection());
                }
                lines.push(line);
            }
        }
        PaletteResults::Commands(commands) => {
            for (ix, (label, key, _)) in commands.iter().enumerate().take(usize::from(inner.height))
            {
                let marker = if ix == sel { "▸ " } else { "  " };
                let width = usize::from(inner.width);
                let mut line = Line::from(vec![
                    Span::raw(marker.to_string()),
                    Span::raw(format!(
                        "{:<w$}",
                        truncate_to_width(label, width.saturating_sub(10)),
                        w = width.saturating_sub(10)
                    )),
                    Span::styled(key.clone(), theme.accent()),
                ]);
                if ix == sel {
                    line = line.style(theme.selection());
                }
                lines.push(line);
            }
        }
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled("  no matches", theme.dim())));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_diagnostics(frame: &mut Frame, app: &App, body: Rect, state: &DiagnosticsState) {
    let theme = &app.theme;
    let rows = app.diag_rows(state.filter);
    let snapshot = app.snapshot.as_ref();
    let (errors, warnings, lint_count) = snapshot.map_or((0, 0, 0), |s| {
        (
            s.validation.error_count(),
            s.validation.warning_count(),
            s.lint.diagnostics.len(),
        )
    });
    let filter_tag = |f: DiagFilter, label: &str| {
        if state.filter == f {
            format!("[{label}]")
        } else {
            format!(" {label} ")
        }
    };
    let title = format!(
        " Diagnostics ── {errors} errors · {warnings} warnings · {lint_count} lint ── {}{}{}{} ",
        filter_tag(DiagFilter::All, "a"),
        filter_tag(DiagFilter::Errors, "e"),
        filter_tag(DiagFilter::Warnings, "w"),
        filter_tag(DiagFilter::Lint, "l"),
    );
    let area = centered(
        body,
        body.width.saturating_sub(6),
        body.height.saturating_sub(2),
    );
    let block = Block::new()
        .borders(Borders::ALL)
        .title(truncate_to_width(
            &title,
            usize::from(area.width).saturating_sub(2),
        ))
        .title_bottom(" ⏎ open   f fix file   F fix all (preview)   t pin --today   Esc ")
        .border_style(theme.accent());
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);

    let height = usize::from(inner.height);
    let sel = state.sel.min(rows.len().saturating_sub(1));
    let offset = sel.saturating_sub(height.saturating_sub(1));
    let mut lines = Vec::new();
    for (ix, row) in rows.iter().enumerate().skip(offset).take(height) {
        let (glyph, style) = if row.fixable {
            (GLYPH_FIXABLE, theme.accent())
        } else {
            match row.severity {
                okf_validator::Severity::Error => (GLYPH_BROKEN, theme.error()),
                okf_validator::Severity::Warning => (GLYPH_WARN, theme.warn()),
                okf_validator::Severity::Info => ("·", theme.dim()),
            }
        };
        let subject = row.concept.as_ref().map_or_else(
            || {
                row.path.as_ref().map_or_else(String::new, |p| {
                    p.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                })
            },
            ToString::to_string,
        );
        let marker = if ix == sel { "▸" } else { " " };
        let source = if row.from_lint { "lint" } else { "" };
        let width = usize::from(inner.width);
        let mut line = Line::from(vec![
            Span::raw(format!("{marker} ")),
            Span::styled(format!("{glyph} "), style),
            Span::raw(format!("{:<28}", truncate_to_width(&subject, 28))),
            Span::raw(truncate_to_width(&row.message, width.saturating_sub(40))),
            Span::styled(format!(" {source}"), theme.dim()),
        ]);
        if ix == sel {
            line = line.style(theme.selection());
        }
        lines.push(line);
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            format!(" {GLYPH_OK} nothing to report"),
            theme.ok(),
        )));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_help(frame: &mut Frame, app: &App, body: Rect, scroll: usize) {
    let theme = &app.theme;
    let area = centered(body, 62, body.height.saturating_sub(2));
    let block = Block::new()
        .borders(Borders::ALL)
        .title(" Help ── keybindings ")
        .title_bottom(" ↑/↓ scroll   Esc close ")
        .border_style(theme.accent());
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut last_context = None;
    for binding in crate::keymap::bindings() {
        if Some(binding.context) != last_context {
            last_context = Some(binding.context);
            lines.push(Line::default());
            lines.push(Line::from(Span::styled(
                format!("{:?}", binding.context).to_uppercase(),
                theme.accent().add_modifier(Modifier::BOLD),
            )));
        }
        let alias = if binding.primary { "" } else { "(alias)" };
        lines.push(Line::from(vec![
            Span::styled(format!("  {:<8}", binding.label), theme.accent()),
            Span::raw(format!("{:<38}", binding.help)),
            Span::styled(alias.to_string(), theme.dim()),
        ]));
    }
    let visible: Vec<Line<'static>> = lines
        .into_iter()
        .skip(scroll)
        .take(usize::from(inner.height))
        .collect();
    frame.render_widget(Paragraph::new(visible), inner);
}

fn draw_new_concept(frame: &mut Frame, app: &App, body: Rect, state: &NewConceptState) {
    let theme = &app.theme;
    let area = centered(body, 58, 8);
    let block = Block::new()
        .borders(Borders::ALL)
        .title(" New concept ")
        .title_bottom(" ⏎ Create   Tab next field   Esc Cancel ")
        .border_style(theme.accent());
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    let width = usize::from(inner.width).saturating_sub(2);
    let labels = ["path ", "type ", "title"];
    let mut lines = vec![Line::default()];
    for (ix, label) in labels.iter().enumerate() {
        lines.push(input_line(
            label,
            &state.fields[ix],
            ix == state.field,
            theme,
            width,
        ));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_fix_preview(frame: &mut Frame, app: &App, body: Rect, state: &FixPreviewState) {
    let theme = &app.theme;
    let changed: Vec<&okf_core::FileFixReport> = state.report.changed_files().collect();
    let area = centered(
        body,
        body.width.saturating_sub(6),
        body.height.saturating_sub(2),
    );
    let file_sel = state.file_sel.min(changed.len().saturating_sub(1));
    let title = format!(
        " Fix preview ── {} remediation(s) in {} file(s) ── file {}/{} (Tab cycles) ",
        state.report.total_remediations(),
        changed.len(),
        file_sel + 1,
        changed.len().max(1)
    );
    let block = Block::new()
        .borders(Borders::ALL)
        .title(truncate_to_width(
            &title,
            usize::from(area.width).saturating_sub(2),
        ))
        .title_bottom(" ⏎ Apply all   ↑/↓ scroll   Esc Cancel ")
        .border_style(theme.accent());
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);

    let Some(file) = changed.get(file_sel) else {
        frame.render_widget(Paragraph::new(" nothing to fix"), inner);
        return;
    };
    let mut lines: Vec<Line<'static>> = vec![Line::from(Span::styled(
        file.path.display().to_string(),
        Style::default().add_modifier(Modifier::BOLD),
    ))];
    for remediation in &file.remediations {
        lines.push(Line::from(Span::styled(
            format!("  {GLYPH_FIXABLE} {}", remediation.description),
            theme.accent(),
        )));
    }
    lines.push(Line::default());
    lines.extend(simple_diff(
        &file.original_content,
        &file.remediated_content,
        theme,
    ));
    let visible: Vec<Line<'static>> = lines
        .into_iter()
        .skip(state.scroll)
        .take(usize::from(inner.height))
        .collect();
    frame.render_widget(Paragraph::new(visible), inner);
}

fn draw_log_view(frame: &mut Frame, app: &App, body: Rect, scroll: usize) {
    let theme = &app.theme;
    let snapshot = app.snapshot.as_ref().expect("drawn only with a snapshot");
    let area = centered(
        body,
        body.width.saturating_sub(10),
        body.height.saturating_sub(2),
    );
    let block = Block::new()
        .borders(Borders::ALL)
        .title(" Activity log (merged) ")
        .title_bottom(" ↑/↓ scroll   Esc close ")
        .border_style(theme.accent());
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    let width = usize::from(inner.width);
    let mut lines: Vec<Line<'static>> = Vec::new();
    for (date, entries) in &snapshot.log_days {
        lines.push(Line::from(Span::styled(
            date.clone(),
            theme.accent().add_modifier(Modifier::BOLD),
        )));
        for entry in entries {
            let kind = entry
                .kind
                .clone()
                .map_or(String::new(), |k| format!("{k}: "));
            lines.push(Line::from(vec![
                Span::styled(
                    format!(" • {kind}"),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw(truncate_to_width(
                    &entry.text,
                    width.saturating_sub(kind.len() + 4),
                )),
            ]));
        }
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled("(no log entries)", theme.dim())));
    }
    let visible: Vec<Line<'static>> = lines
        .into_iter()
        .skip(scroll)
        .take(usize::from(inner.height))
        .collect();
    frame.render_widget(Paragraph::new(visible), inner);
}

fn draw_outline(frame: &mut Frame, app: &App, body: Rect, state: &crate::app::OutlineState) {
    let theme = &app.theme;
    let headings: Vec<(usize, String)> = app
        .snapshot
        .as_ref()
        .and_then(|s| s.meta(&state.id))
        .map(|m| m.headings.clone())
        .unwrap_or_default();
    let height = u16::try_from(headings.len().min(16)).unwrap_or(0) + 2;
    let area = centered(body, 54, height.max(4));
    let block = Block::new()
        .borders(Borders::ALL)
        .title(format!(" Outline · {} ", state.id))
        .title_bottom(" ⏎ jump   s split   r rename   Esc ")
        .border_style(theme.accent());
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    let sel = state.sel.min(headings.len().saturating_sub(1));
    let mut lines = Vec::new();
    for (ix, (level, text)) in headings.iter().enumerate().take(usize::from(inner.height)) {
        let indent = "  ".repeat(level.saturating_sub(1));
        let marker = if ix == sel { "▸" } else { " " };
        let mut line = Line::from(format!("{marker} {indent}{text}"));
        if ix == sel {
            line = line.style(theme.selection());
        }
        lines.push(line);
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(" (no headings)", theme.dim())));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}
