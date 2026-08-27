//! Workspace 2 — Graph: the link graph on a Braille canvas.

use crate::app::{App, ColorBy};
use crate::graph::{EdgeKind, NodeKind};
use crate::markdown::truncate_to_width;
use crate::theme::{GLYPH_BROKEN, GLYPH_COMPUTATION, GLYPH_INDEX, GLYPH_WARN, tier_glyph};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::canvas::{Canvas, Line as CanvasLine};
use ratatui::widgets::{Block, Borders, Paragraph};

/// Draws the graph workspace.
///
/// # Panics
///
/// Panics if called before the first snapshot has landed; the shell draws
/// a loading screen instead of calling any workspace until then.
#[allow(clippy::too_many_lines)]
pub fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let snapshot = app.snapshot.as_ref().expect("drawn only with a snapshot");
    let model = &snapshot.graph;
    let included = app.graph_included(snapshot);
    let visible_nodes = included.iter().filter(|&&i| i).count();

    let [canvas_area, status_area] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(area);

    let focus_label = app
        .graph
        .focus
        .as_ref()
        .map_or(String::new(), |(_, k)| format!(" · focus {k}-hop"));
    let title = format!(
        " Graph ── {} nodes · {} edges ── layout: {} ── color: {}{focus_label} ",
        visible_nodes,
        model.edges.len(),
        app.graph.mode.name(),
        app.graph.color_by.name(),
    );
    let legend = format!(
        " ◆ human-reviewed  ● machine-confirmed  ○ unverified  {GLYPH_WARN} stale  {GLYPH_BROKEN} broken "
    );
    let block = Block::new()
        .borders(Borders::ALL)
        .title(truncate_to_width(
            &title,
            usize::from(area.width).saturating_sub(2),
        ))
        .title_bottom(Line::from(Span::styled(
            truncate_to_width(&legend, usize::from(area.width).saturating_sub(2)),
            theme.dim(),
        )))
        .border_style(theme.dim());
    let inner = block.inner(canvas_area);

    // Aspect-corrected bounds: terminal cells are ~2× taller than wide.
    let half_w = 1.2 / app.graph.zoom;
    let aspect = f64::from(inner.height) * 2.0 / f64::from(inner.width.max(1));
    let half_h = half_w * aspect;
    let (cx, cy) = app.graph.pan;
    let x_bounds = [cx - half_w, cx + half_w];
    let y_bounds = [cy - half_h, cy + half_h];

    let positions = &app.graph.layout.positions;
    let pos_of = |ix: usize| positions.get(&model.nodes[ix].key).copied();
    let filter = app
        .graph
        .filter_input
        .clone()
        .unwrap_or_else(|| app.graph.filter.clone());
    let matches_filter = |ix: usize| -> bool {
        filter.is_empty() || crate::search::fuzzy_match(&filter, &model.nodes[ix].label).is_some()
    };
    let selected_ix = app
        .graph
        .selected
        .as_ref()
        .and_then(|key| model.nodes.iter().position(|n| &n.key == key));

    // Label budget scales with zoom; highest-degree nodes win.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let label_budget = ((app.graph.zoom * 8.0) as usize).clamp(3, 40);
    let mut by_degree: Vec<usize> = (0..model.nodes.len()).filter(|&i| included[i]).collect();
    by_degree.sort_by_key(|&i| std::cmp::Reverse(model.nodes[i].degree));
    let labeled: std::collections::HashSet<usize> =
        by_degree.into_iter().take(label_budget).collect();

    let canvas = Canvas::default()
        .marker(Marker::Braille)
        .x_bounds(x_bounds)
        .y_bounds(y_bounds)
        .paint(|ctx| {
            for edge in &model.edges {
                if !included[edge.from] || !included[edge.to] {
                    continue;
                }
                if edge.kind == EdgeKind::Derivation && !app.graph.show_derivations {
                    continue;
                }
                let (Some(a), Some(b)) = (pos_of(edge.from), pos_of(edge.to)) else {
                    continue;
                };
                let selected_edge = selected_ix == Some(edge.from) || selected_ix == Some(edge.to);
                let color = if !app.theme.color {
                    Color::Reset
                } else if selected_edge {
                    Color::Cyan
                } else {
                    match edge.kind {
                        EdgeKind::Broken => Color::Red,
                        EdgeKind::Derivation => Color::Magenta,
                        EdgeKind::Source => Color::DarkGray,
                        EdgeKind::Link => Color::Gray,
                    }
                };
                ctx.draw(&CanvasLine {
                    x1: a.0,
                    y1: a.1,
                    x2: b.0,
                    y2: b.1,
                    color,
                });
            }
            ctx.layer();
            for (ix, node) in model.nodes.iter().enumerate() {
                if !included[ix] {
                    continue;
                }
                let Some((x, y)) = pos_of(ix) else { continue };
                let selected = selected_ix == Some(ix);
                let dimmed = !matches_filter(ix);
                let (glyph, mut style) = node_appearance(app, snapshot, ix);
                if dimmed {
                    style = theme.dim();
                }
                if selected {
                    style = style.add_modifier(Modifier::REVERSED);
                }
                let text = if selected || (labeled.contains(&ix) && !dimmed) {
                    format!("{glyph} {}", node.label)
                } else {
                    glyph.to_string()
                };
                ctx.print(x, y, Line::from(Span::styled(text, style)));
            }
        });
    frame.render_widget(block, canvas_area);
    frame.render_widget(canvas, inner);

    // Status line: filter input or selected-node summary.
    let status: Line<'static> = app.graph.filter_input.as_ref().map_or_else(
        || {
            selected_ix.map_or_else(
                || {
                    Line::from(Span::styled(
                        " Tab selects a node · Enter opens it · f focus mode".to_string(),
                        theme.dim(),
                    ))
                },
                |ix| {
                    let node = &model.nodes[ix];
                    node.id.as_ref().map_or_else(
                        || Line::from(Span::raw(format!(" ▸ {}", node.label))),
                        |id| {
                            let meta = snapshot.meta(id);
                            let text = meta.map_or_else(
                                || format!(" ▸ {id}"),
                                |m| {
                                    format!(
                                        " ▸ {id} — {} · {} · {} out / {} in · {} source(s)",
                                        snapshot
                                            .bundle
                                            .get(id)
                                            .and_then(|c| c
                                                .type_()
                                                .map(std::borrow::Cow::into_owned))
                                            .unwrap_or_default(),
                                        tier_glyph(m.tier),
                                        m.out_degree,
                                        m.in_degree,
                                        m.source_count
                                    )
                                },
                            );
                            Line::from(Span::raw(text))
                        },
                    )
                },
            )
        },
        |input| {
            Line::from(vec![
                Span::styled(" filter ⌕ ".to_string(), theme.accent()),
                Span::raw(input.clone()),
                Span::styled("█".to_string(), theme.accent()),
            ])
        },
    );
    frame.render_widget(Paragraph::new(status), status_area);
}

/// The glyph and style for a node under the active coloring dimension.
fn node_appearance(
    app: &App,
    snapshot: &crate::snapshot::Snapshot,
    ix: usize,
) -> (&'static str, Style) {
    let theme = &app.theme;
    let node = &snapshot.graph.nodes[ix];
    match node.kind {
        NodeKind::Phantom => return (GLYPH_BROKEN, theme.error()),
        NodeKind::Source => return (GLYPH_INDEX, theme.dim()),
        NodeKind::Computation => {
            if app.graph.color_by == ColorBy::Trust {
                return (GLYPH_COMPUTATION, theme.accent());
            }
        }
        NodeKind::Concept => {}
    }
    let Some(meta) = node.id.as_ref().and_then(|id| snapshot.meta(id)) else {
        return ("●", Style::default());
    };
    match app.graph.color_by {
        ColorBy::Trust => (tier_glyph(meta.tier), theme.tier(meta.tier)),
        ColorBy::Status => (
            crate::theme::status_glyph(&meta.status),
            theme.status(&meta.status),
        ),
        ColorBy::Staleness => {
            if meta.stale {
                (GLYPH_WARN, theme.warn())
            } else if meta.stale_in_days.is_some() {
                ("⏳", theme.warn())
            } else {
                ("●", theme.ok())
            }
        }
        ColorBy::Type => {
            if meta.is_computation {
                (GLYPH_COMPUTATION, theme.accent())
            } else {
                ("●", Style::default())
            }
        }
        ColorBy::Diagnostics => {
            if meta.diag_errors > 0 {
                (GLYPH_BROKEN, theme.error())
            } else if meta.diag_warnings + meta.lint_findings > 0 {
                (GLYPH_WARN, theme.warn())
            } else {
                ("●", theme.ok())
            }
        }
    }
}
