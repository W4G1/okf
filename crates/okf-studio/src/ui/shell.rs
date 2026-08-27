//! The persistent chrome: tab bar, status bar, hint bar, toasts, and the
//! overlay stack.

use crate::app::{App, ExplorerPane, LoadPhase, Overlay};
use crate::keymap::{Context, hints};
use crate::markdown::truncate_to_width;
use crate::theme::{GLYPH_BROKEN, GLYPH_OK, GLYPH_WARN, SPINNER_FRAMES};
use crate::{Tab, ui};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

/// Draws the whole UI.
pub fn draw(frame: &mut Frame, app: &App) {
    let [tab_bar, body, status, hint_bar] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    draw_tabs(frame, app, tab_bar);
    if let Some(error) = &app.load_error {
        let text = Paragraph::new(vec![
            Line::default(),
            Line::from(Span::styled(
                format!("{GLYPH_BROKEN} could not load bundle"),
                app.theme.error(),
            )),
            Line::from(error.as_str()),
            Line::default(),
            Line::from(Span::styled("R retries · q quits", app.theme.dim())),
        ]);
        frame.render_widget(text, body);
    } else if app.snapshot.is_some() {
        match app.tab {
            Tab::Explorer => ui::explorer::draw(frame, app, body),
            Tab::Graph => ui::graph::draw(frame, app, body),
            Tab::Trust => ui::trust::draw(frame, app, body),
            Tab::Computations => ui::computations::draw(frame, app, body),
        }
    } else {
        let spinner = SPINNER_FRAMES[usize::try_from(app.tick).unwrap_or(0) % SPINNER_FRAMES.len()];
        frame.render_widget(
            Paragraph::new(format!("\n {spinner} loading bundle…")),
            body,
        );
    }
    draw_status(frame, app, status);
    draw_hints(frame, app, hint_bar);

    for overlay in &app.overlays {
        match overlay {
            Overlay::Refactor(state) => ui::refactor_modal::draw(frame, app, body, state),
            other => ui::overlays::draw(frame, app, body, other),
        }
    }

    draw_toasts(frame, app, body);
}

fn draw_tabs(frame: &mut Frame, app: &App, area: Rect) {
    let stats = app.snapshot.as_ref().map(|s| &s.stats);
    let stale_badge = stats.map_or(0, |s| s.stale);
    let labels: [(Tab, String); 4] = [
        (Tab::Explorer, "[1]Explorer".to_string()),
        (Tab::Graph, "[2]Graph".to_string()),
        (
            Tab::Trust,
            if stale_badge > 0 {
                format!("[3]Trust {GLYPH_WARN}{stale_badge}")
            } else {
                "[3]Trust".to_string()
            },
        ),
        (Tab::Computations, "[4]Compute".to_string()),
    ];
    let mut spans = vec![Span::styled(
        format!(" okf studio ─ {} ", app.root_name),
        app.theme.accent().add_modifier(Modifier::BOLD),
    )];
    for (tab, label) in labels {
        let style = if tab == app.tab {
            app.theme.accent().add_modifier(Modifier::REVERSED)
        } else {
            Style::default()
        };
        spans.push(Span::styled(format!(" {label} "), style));
        spans.push(Span::raw(" "));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let mut spans: Vec<Span<'static>> = Vec::new();
    if let Some(snapshot) = &app.snapshot {
        let errors = snapshot.validation.error_count();
        let warnings = snapshot.validation.warning_count();
        if snapshot.validation.is_conformant() {
            spans.push(Span::styled(format!(" {GLYPH_OK} conformant"), theme.ok()));
        } else {
            spans.push(Span::styled(
                format!(" {GLYPH_BROKEN} not conformant"),
                theme.error(),
            ));
        }
        spans.push(Span::styled(
            format!("  {GLYPH_WARN} {warnings} warnings"),
            if warnings > 0 {
                theme.warn()
            } else {
                theme.dim()
            },
        ));
        spans.push(Span::styled(
            format!("  {GLYPH_BROKEN} {errors} errors"),
            if errors > 0 {
                theme.error()
            } else {
                theme.dim()
            },
        ));
        spans.push(Span::styled(
            format!(
                " │ {} concepts │ {} stale │ today {}",
                snapshot.stats.concepts, snapshot.stats.stale, snapshot.today
            ),
            Style::default(),
        ));
    }
    match app.loading {
        LoadPhase::Idle => {}
        LoadPhase::Reloading | LoadPhase::Applying => {
            let spinner =
                SPINNER_FRAMES[usize::try_from(app.tick).unwrap_or(0) % SPINNER_FRAMES.len()];
            let what = if app.loading == LoadPhase::Reloading {
                "reloading"
            } else {
                "applying"
            };
            spans.push(Span::styled(format!(" │ {spinner} {what}"), theme.accent()));
        }
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_hints(frame: &mut Frame, app: &App, area: Rect) {
    let context = match (app.tab, app.explorer.pane) {
        (Tab::Explorer, ExplorerPane::Tree) => Context::Tree,
        (Tab::Explorer, ExplorerPane::Viewer) => Context::Viewer,
        (Tab::Graph, _) => Context::Graph,
        (Tab::Trust, _) => Context::Trust,
        (Tab::Computations, _) => Context::Computations,
    };
    let mut spans: Vec<Span<'static>> = vec![Span::raw(" ")];
    for binding in hints(context).take(5) {
        spans.push(Span::styled(binding.label.to_string(), app.theme.accent()));
        spans.push(Span::styled(
            format!(" {}  ", binding.help),
            app.theme.dim(),
        ));
    }
    spans.push(Span::styled("/".to_string(), app.theme.accent()));
    spans.push(Span::styled(" search  ".to_string(), app.theme.dim()));
    spans.push(Span::styled("?".to_string(), app.theme.accent()));
    spans.push(Span::styled(" help  ".to_string(), app.theme.dim()));
    spans.push(Span::styled("q".to_string(), app.theme.accent()));
    spans.push(Span::styled(" quit".to_string(), app.theme.dim()));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_toasts(frame: &mut Frame, app: &App, body: Rect) {
    for (i, toast) in app.toasts.iter().rev().take(3).enumerate() {
        let text = truncate_to_width(&toast.text, usize::from(body.width).saturating_sub(4));
        let width = u16::try_from(crate::markdown::str_width(&text)).unwrap_or(body.width) + 2;
        let i16 = u16::try_from(i).unwrap_or(0);
        if body.height < 2 + i16 {
            break;
        }
        let area = Rect {
            x: body.x + body.width.saturating_sub(width + 1),
            y: body.y + body.height - 1 - i16,
            width: width.min(body.width),
            height: 1,
        };
        let style = if toast.error {
            app.theme.error().add_modifier(Modifier::REVERSED)
        } else {
            app.theme.ok().add_modifier(Modifier::REVERSED)
        };
        frame.render_widget(Clear, area);
        frame.render_widget(Paragraph::new(format!(" {text} ")).style(style), area);
    }
}
