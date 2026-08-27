//! Workspace 3 — Mission Control: trust, lifecycle, freshness, the attention
//! queue, activity, and actors.

use crate::app::{App, Cohort, TrustPanel};
use crate::markdown::truncate_to_width;
use crate::snapshot::ActorStats;
use crate::theme::{GLYPH_STALE_SOON, GLYPH_WARN, status_glyph, tier_glyph};
use crate::ui::widgets::{BarRowSpec, bar_row, date_sparkline};
use okf_core::{ActorKind, Status, TrustTier};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

/// Draws the mission-control workspace.
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
        .title(format!(" Mission Control ── today: {} ", snapshot.today))
        .border_style(theme.dim());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let [bars_area, queue_area, bottom_area] = Layout::vertical([
        Constraint::Length(5),
        Constraint::Min(4),
        Constraint::Length(6),
    ])
    .areas(inner);

    draw_bars(frame, app, bars_area);
    draw_queue(frame, app, queue_area);
    let [activity_area, actors_area] =
        Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
            .areas(bottom_area);
    draw_activity(frame, app, activity_area);
    draw_actors(frame, app, actors_area);
}

fn draw_bars(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let snapshot = app.snapshot.as_ref().expect("drawn only with a snapshot");
    let stats = &snapshot.stats;
    let [trust_area, lifecycle_area, fresh_area] = Layout::horizontal([
        Constraint::Percentage(34),
        Constraint::Percentage(33),
        Constraint::Percentage(33),
    ])
    .areas(area);

    let max = stats.concepts.max(1);
    let bar_width = usize::from(area.width / 3).saturating_sub(28).max(4);
    let focus = app.trust.panel;
    let sel = app.trust.bar_sel;

    let header = |text: &str, focused: bool| {
        Line::from(Span::styled(
            text.to_string(),
            if focused {
                theme.accent().add_modifier(Modifier::BOLD)
            } else {
                theme.dim()
            },
        ))
    };

    let mut trust_lines = vec![header("TRUST", focus == TrustPanel::TrustBars)];
    let tiers = [
        (TrustTier::HumanReviewed, stats.tier_counts[2]),
        (TrustTier::MachineConfirmed, stats.tier_counts[1]),
        (TrustTier::Unverified, stats.tier_counts[0]),
    ];
    for (i, (tier, count)) in tiers.iter().enumerate() {
        trust_lines.push(bar_row(
            &BarRowSpec {
                glyph: tier_glyph(*tier),
                glyph_style: theme.tier(*tier),
                label: tier.as_str(),
                count: *count,
                max,
                bar_width,
                selected: focus == TrustPanel::TrustBars && sel == i,
            },
            theme,
        ));
    }
    frame.render_widget(Paragraph::new(trust_lines), trust_area);

    let mut lifecycle_lines = vec![header("LIFECYCLE", focus == TrustPanel::LifecycleBars)];
    let statuses = [
        (Status::Stable, stats.status_counts[1]),
        (Status::Draft, stats.status_counts[0]),
        (Status::Deprecated, stats.status_counts[2]),
        (Status::Other("other".into()), stats.status_counts[3]),
    ];
    for (i, (status, count)) in statuses.iter().enumerate() {
        if *count == 0 && i == 3 {
            continue;
        }
        lifecycle_lines.push(bar_row(
            &BarRowSpec {
                glyph: status_glyph(status),
                glyph_style: theme.status(status),
                label: status.as_str(),
                count: *count,
                max,
                bar_width,
                selected: focus == TrustPanel::LifecycleBars && sel == i,
            },
            theme,
        ));
    }
    frame.render_widget(Paragraph::new(lifecycle_lines), lifecycle_area);

    let fresh_count = stats
        .concepts
        .saturating_sub(stats.stale + stats.stale_soon);
    let mut fresh_lines = vec![header("FRESHNESS", focus == TrustPanel::FreshnessBars)];
    let freshness = [
        ("fresh", fresh_count, theme.ok()),
        ("stale", stats.stale, theme.warn()),
        ("<30 days", stats.stale_soon, theme.warn()),
    ];
    for (i, (label, count, style)) in freshness.iter().enumerate() {
        fresh_lines.push(bar_row(
            &BarRowSpec {
                glyph: "●",
                glyph_style: *style,
                label,
                count: *count,
                max,
                bar_width,
                selected: focus == TrustPanel::FreshnessBars && sel == i,
            },
            theme,
        ));
    }
    frame.render_widget(Paragraph::new(fresh_lines), fresh_area);
}

fn cohort_label(app: &App) -> String {
    match app.trust.cohort {
        None => String::new(),
        Some(Cohort::Tier(tier)) => format!(" · filter: {tier}"),
        Some(Cohort::Status(ix)) => format!(
            " · filter: {}",
            ["draft", "stable", "deprecated", "other"][ix.min(3)]
        ),
        Some(Cohort::Fresh(ix)) => {
            format!(" · filter: {}", ["fresh", "stale", "stale <30d"][ix.min(2)])
        }
    }
}

fn draw_queue(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let snapshot = app.snapshot.as_ref().expect("drawn only with a snapshot");
    let queue = app.filtered_queue();
    let focused = app.trust.panel == TrustPanel::Queue;
    let title = format!(
        " ATTENTION QUEUE ({}){}    sort: {} ▾ ",
        queue.len(),
        cohort_label(app),
        app.trust.sort.name()
    );
    let block = Block::new()
        .borders(Borders::TOP)
        .title(Line::from(Span::styled(
            title,
            if focused {
                theme.accent().add_modifier(Modifier::BOLD)
            } else {
                theme.dim()
            },
        )))
        .border_style(theme.dim());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let height = usize::from(inner.height);
    let sel = app.trust.queue_sel.min(queue.len().saturating_sub(1));
    let offset = sel.saturating_sub(height.saturating_sub(1));
    let mut lines = Vec::new();
    for (ix, item) in queue.iter().enumerate().skip(offset).take(height) {
        let meta = snapshot.meta(&item.id);
        let glyph = meta.map_or(" ", |m| {
            if m.stale {
                GLYPH_WARN
            } else if m.stale_in_days.is_some() {
                GLYPH_STALE_SOON
            } else if m.status.is_deprecated() {
                "◌"
            } else {
                tier_glyph(m.tier)
            }
        });
        let marker = if ix == sel && focused { "▸" } else { " " };
        let reasons = item.reasons.join(" · ");
        let width = usize::from(inner.width);
        let mut line = Line::from(vec![
            Span::raw(format!("{marker} ")),
            Span::styled(format!("{glyph} "), theme.warn()),
            Span::styled(
                format!("{:<32}", truncate_to_width(&item.id.to_string(), 32)),
                Style::default(),
            ),
            Span::styled(
                truncate_to_width(&reasons, width.saturating_sub(38)),
                theme.dim(),
            ),
        ]);
        if ix == sel && focused {
            line = line.style(theme.selection());
        }
        lines.push(line);
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            " nothing needs attention ✔",
            theme.ok(),
        )));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_activity(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let snapshot = app.snapshot.as_ref().expect("drawn only with a snapshot");
    let focused = app.trust.panel == TrustPanel::Activity;
    let block = Block::new()
        .borders(Borders::TOP)
        .title(Line::from(Span::styled(
            " ACTIVITY  log.md, last 90 days ".to_string(),
            if focused {
                theme.accent().add_modifier(Modifier::BOLD)
            } else {
                theme.dim()
            },
        )))
        .title_bottom(Line::from(Span::styled(
            if focused { " ⏎ full log " } else { "" }.to_string(),
            theme.dim(),
        )))
        .border_style(theme.dim());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let days = i64::from(inner.width.saturating_sub(2)).clamp(7, 90);
    let spark = date_sparkline(&snapshot.log_timeline, snapshot.today, days, theme);
    let total: usize = snapshot.log_timeline.iter().map(|(_, c)| c).sum();
    let lines = vec![
        Line::default(),
        spark,
        Line::from(Span::styled(
            format!("{total} log entries total"),
            theme.dim(),
        )),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_actors(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let snapshot = app.snapshot.as_ref().expect("drawn only with a snapshot");
    let block = Block::new()
        .borders(Borders::TOP)
        .title(Line::from(Span::styled(
            " ACTORS ".to_string(),
            theme.dim(),
        )))
        .border_style(theme.dim());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut actors: Vec<(&String, &ActorStats)> = snapshot.stats.actors.iter().collect();
    actors.sort_by_key(|(_, stats)| std::cmp::Reverse(stats.generated + stats.verified));
    let mut lines = Vec::new();
    for (name, stats) in actors.into_iter().take(usize::from(inner.height)) {
        let style = theme.actor(stats.kind.unwrap_or(ActorKind::Other));
        let counts = format!("gen {:>3} · ver {:>3}", stats.generated, stats.verified);
        let width = usize::from(inner.width);
        lines.push(Line::from(vec![
            Span::styled(
                format!(
                    "{:<w$}",
                    truncate_to_width(name, width.saturating_sub(20)),
                    w = width.saturating_sub(20)
                ),
                style,
            ),
            Span::styled(counts, theme.dim()),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}
