//! Workspace 4 — Computations Playground: contract inspection and an
//! execution-free invocation builder.

use crate::app::{App, CompPane};
use crate::markdown::truncate_to_width;
use crate::theme::{GLYPH_BROKEN, GLYPH_COMPUTATION, GLYPH_OK};
use crate::ui::widgets::input_line;
use okf_core::ComputationSource;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

/// Draws the computations workspace.
///
/// # Panics
///
/// Panics if called before the first snapshot has landed; the shell draws
/// a loading screen instead of calling any workspace until then.
pub fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let snapshot = app.snapshot.as_ref().expect("drawn only with a snapshot");
    let block = Block::new()
        .borders(Borders::ALL)
        .title(format!(
            " Computations ── {} contract(s) ",
            snapshot.contracts.len()
        ))
        .border_style(theme.dim());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if snapshot.contracts.is_empty() {
        frame.render_widget(
            Paragraph::new("\n no Attested Computation concepts in this bundle"),
            inner,
        );
        return;
    }

    let [list_area, detail_area] =
        Layout::horizontal([Constraint::Length(26), Constraint::Min(20)]).areas(inner);
    draw_list(frame, app, list_area);
    draw_detail(frame, app, detail_area);
}

fn draw_list(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let snapshot = app.snapshot.as_ref().expect("drawn only with a snapshot");
    let focused = app.computations.pane == CompPane::List;
    let mut lines = vec![Line::from(Span::styled(
        "CONTRACTS",
        if focused {
            theme.accent().add_modifier(Modifier::BOLD)
        } else {
            theme.dim()
        },
    ))];
    for (ix, info) in snapshot.contracts.iter().enumerate() {
        let selected = ix == app.computations.selected;
        let marker = if selected { "▸" } else { " " };
        let (verdict, style) = if info.healthy() {
            (GLYPH_OK, theme.ok())
        } else {
            (GLYPH_BROKEN, theme.error())
        };
        let mut line = Line::from(vec![
            Span::raw(format!("{marker} ")),
            Span::styled(format!("{GLYPH_COMPUTATION} "), theme.accent()),
            Span::raw(format!("{:<16}", truncate_to_width(info.id.name(), 16))),
            Span::styled(verdict.to_string(), style),
        ]);
        if selected && focused {
            line = line.style(theme.selection());
        }
        lines.push(line);
    }
    frame.render_widget(Paragraph::new(lines), area);
}

#[allow(clippy::too_many_lines)]
fn draw_detail(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let snapshot = app.snapshot.as_ref().expect("drawn only with a snapshot");
    let Some(info) = snapshot.contracts.get(app.computations.selected) else {
        return;
    };
    let contract = &info.contract;
    let width = usize::from(area.width).saturating_sub(2);
    let form_focused = app.computations.pane == CompPane::Form;

    let mut lines: Vec<Line<'static>> = Vec::new();
    let meta = snapshot.meta(&info.id);
    lines.push(Line::from(vec![
        Span::styled(
            format!("{} ", info.id),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "· runtime: {} · {}",
                contract.runtime.clone().unwrap_or_else(|| "?".into()),
                meta.map_or_else(String::new, |m| format!(
                    "{} {}",
                    crate::theme::status_glyph(&m.status),
                    crate::theme::tier_glyph(m.tier)
                ))
            ),
            theme.dim(),
        ),
    ]));
    lines.push(Line::default());

    // Contract card.
    lines.push(Line::from(Span::styled("CONTRACT", theme.accent())));
    if contract.parameters.is_empty() {
        lines.push(Line::from(Span::styled("  (no parameters)", theme.dim())));
    }
    for parameter in &contract.parameters {
        lines.push(Line::from(vec![
            Span::styled("  parameter  ".to_string(), theme.dim()),
            Span::raw(parameter.to_string()),
        ]));
    }
    for (field, raw, resolved) in &info.path_checks {
        let (glyph, style) = if resolved.is_some() {
            (GLYPH_OK, theme.ok())
        } else {
            (GLYPH_BROKEN, theme.error())
        };
        lines.push(Line::from(vec![
            Span::styled(format!("  {field:<11}"), theme.dim()),
            Span::raw(truncate_to_width(raw, width.saturating_sub(16))),
            Span::styled(format!(" {glyph}"), style),
        ]));
    }
    if let Some(executor) = &contract.executor
        && !executor.receipt.is_empty()
    {
        lines.push(Line::from(vec![
            Span::styled("  receipt    ".to_string(), theme.dim()),
            Span::raw(executor.receipt.join(", ")),
        ]));
    }
    for issue in &info.issues {
        lines.push(Line::from(Span::styled(
            format!("  {GLYPH_BROKEN} {issue}"),
            theme.error(),
        )));
    }
    lines.push(Line::default());

    // Computation block.
    match &contract.computation {
        ComputationSource::Inline(inline) => {
            let verdict = match &info.syntax {
                Some(Ok(())) => format!(" syntax {GLYPH_OK}"),
                Some(Err(_)) => format!(" syntax {GLYPH_BROKEN}"),
                None => String::new(),
            };
            lines.push(Line::from(Span::styled(
                format!(
                    "COMPUTATION (inline · {}{verdict})",
                    inline.language.clone().unwrap_or_else(|| "?".into())
                ),
                theme.accent(),
            )));
            for code_line in inline.code.lines().take(10) {
                lines.push(Line::from(vec![
                    Span::styled("│ ".to_string(), theme.dim()),
                    Span::raw(truncate_to_width(code_line, width.saturating_sub(2))),
                ]));
            }
            if inline.code.lines().count() > 10 {
                lines.push(Line::from(Span::styled("│ …", theme.dim())));
            }
        }
        ComputationSource::File(path) => {
            lines.push(Line::from(vec![
                Span::styled("COMPUTATION ".to_string(), theme.accent()),
                Span::raw(format!("file: {path}")),
            ]));
        }
        ComputationSource::Missing => {
            lines.push(Line::from(Span::styled(
                format!("COMPUTATION {GLYPH_BROKEN} missing"),
                theme.error(),
            )));
        }
    }
    lines.push(Line::default());

    // Playground.
    lines.push(Line::from(Span::styled(
        "PLAYGROUND ── invocation builder",
        if form_focused {
            theme.accent().add_modifier(Modifier::BOLD)
        } else {
            theme.accent()
        },
    )));
    let mut args: Vec<String> = Vec::new();
    for (ix, parameter) in contract.parameters.iter().enumerate() {
        let name = parameter.name.clone().unwrap_or_default();
        let key = format!("{}\u{0}{}", info.id, name);
        let value = app
            .computations
            .values
            .get(&key)
            .cloned()
            .unwrap_or_default();
        let type_ = parameter.type_.clone().unwrap_or_else(|| "string".into());
        let valid = check_type(&type_, &value);
        let required = parameter.is_required();
        let verdict = if value.is_empty() {
            if required {
                Span::styled(format!(" {GLYPH_BROKEN} required"), app.theme.error())
            } else {
                Span::styled(format!(" ({type_}, opt)"), app.theme.dim())
            }
        } else if valid {
            Span::styled(format!(" {GLYPH_OK} {type_}"), app.theme.ok())
        } else {
            Span::styled(format!(" {GLYPH_BROKEN} not a {type_}"), app.theme.error())
        };
        let active = form_focused && ix == app.computations.field;
        let mut line = input_line(
            &format!("  {name:<14}"),
            &value,
            active,
            theme,
            width.saturating_sub(16),
        );
        line.spans.push(verdict);
        lines.push(line);
        if !value.is_empty() || required {
            args.push(format!("{name}={value}"));
        }
    }
    lines.push(Line::default());
    lines.push(Line::from(vec![
        Span::styled("  ▶ call sketch:  ".to_string(), theme.dim()),
        Span::styled(
            format!("{}({})", info.id.name(), args.join(", ")),
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ]));
    if let Some(executor) = &contract.executor
        && !executor.receipt.is_empty()
    {
        lines.push(Line::from(vec![
            Span::styled("  ▶ expected receipt: ".to_string(), theme.dim()),
            Span::raw(executor.receipt.join(", ")),
        ]));
    }
    lines.push(Line::from(Span::styled(
        "  Tab focuses the form · Ctrl+Y copies the sketch",
        theme.dim(),
    )));

    let visible: Vec<Line<'static>> = lines.into_iter().take(usize::from(area.height)).collect();
    frame.render_widget(Paragraph::new(visible), area);
}

/// Live type check for a playground value.
fn check_type(type_: &str, value: &str) -> bool {
    if value.is_empty() {
        return false;
    }
    match type_.to_ascii_lowercase().as_str() {
        "number" | "float" | "integer" | "int" => value.parse::<f64>().is_ok(),
        "boolean" | "bool" => matches!(value, "true" | "false"),
        _ => true,
    }
}
