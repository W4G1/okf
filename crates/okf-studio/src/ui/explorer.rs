//! Workspace 1 — Explorer: tree → document → inspector.

use crate::app::{App, ExplorerPane, TreeRowKind, TreeSel};
use crate::markdown::{render_document, str_width, truncate_to_width};
use crate::snapshot::LeafKind;
use crate::theme::{
    GLYPH_BROKEN, GLYPH_COMPUTATION, GLYPH_INDEX, GLYPH_LOG, GLYPH_OK, GLYPH_WARN, status_glyph,
    tier_glyph,
};
use okf_core::{ConceptId, ResourceKind};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

/// Draws the explorer workspace.
pub fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let [tree, doc, inspector] = Layout::horizontal([
        Constraint::Percentage(20),
        Constraint::Percentage(60),
        Constraint::Percentage(20),
    ])
    .areas(area);
    draw_tree(frame, app, tree);
    draw_document(frame, app, doc);
    draw_inspector(frame, app, inspector);
}

fn draw_tree(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let snapshot = app.snapshot.as_ref().expect("drawn only with a snapshot");
    let focused = app.explorer.pane == ExplorerPane::Tree;
    let footer = format!(
        " {} concepts, {} {GLYPH_WARN} stale ",
        snapshot.stats.concepts, snapshot.stats.stale
    );
    let block = Block::new()
        .borders(Borders::ALL)
        .title(" Explorer ")
        .title_bottom(Line::from(Span::styled(footer, theme.dim())))
        .border_style(if focused { theme.accent() } else { theme.dim() });
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows = app.tree_rows();
    let height = usize::from(inner.height);
    let selected_ix = app
        .explorer
        .selected
        .as_ref()
        .and_then(|sel| rows.iter().position(|row| &row.sel == sel))
        .unwrap_or(0);
    let mut offset = app
        .explorer
        .tree_offset
        .get()
        .min(rows.len().saturating_sub(1));
    if selected_ix < offset {
        offset = selected_ix;
    } else if height > 0 && selected_ix >= offset + height {
        offset = selected_ix + 1 - height;
    }
    app.explorer.tree_offset.set(offset);

    let mut lines: Vec<Line<'static>> = Vec::new();
    for (ix, row) in rows.iter().enumerate().skip(offset).take(height) {
        let indent = "  ".repeat(row.depth);
        let selected = ix == selected_ix;
        let mut spans: Vec<Span<'static>> = vec![Span::raw(indent)];
        match &row.kind {
            TreeRowKind::Dir { count, collapsed } => {
                spans.push(Span::styled(
                    if *collapsed { "▸ " } else { "▾ " }.to_string(),
                    theme.accent(),
                ));
                spans.push(Span::styled(
                    row.name.clone(),
                    Style::default().add_modifier(Modifier::BOLD),
                ));
                spans.push(Span::styled(format!("  {count}"), theme.dim()));
            }
            TreeRowKind::Leaf(kind) => match kind {
                LeafKind::Concept(id) => {
                    let meta = snapshot.meta(id);
                    let (glyph, style) = meta.map_or_else(
                        || ("●", Style::default()),
                        |m| {
                            if m.is_computation {
                                (GLYPH_COMPUTATION, theme.accent())
                            } else {
                                (tier_glyph(m.tier), theme.tier(m.tier))
                            }
                        },
                    );
                    spans.push(Span::styled(format!("{glyph} "), style));
                    let name_style = meta.map_or_else(Style::default, |m| theme.status(&m.status));
                    spans.push(Span::styled(id.name().to_string(), name_style));
                    if let Some(m) = meta {
                        if m.stale {
                            spans.push(Span::styled(format!(" {GLYPH_WARN}"), theme.warn()));
                        } else if m.stale_in_days.is_some() {
                            spans.push(Span::styled(" ⏳".to_string(), theme.warn()));
                        }
                        if m.diag_errors > 0 {
                            spans.push(Span::styled(format!(" {GLYPH_BROKEN}"), theme.error()));
                        }
                    }
                }
                LeafKind::Index(_) => {
                    spans.push(Span::styled(format!("{GLYPH_INDEX} "), theme.dim()));
                    spans.push(Span::styled(row.name.clone(), theme.dim()));
                }
                LeafKind::Log(_) => {
                    spans.push(Span::styled(format!("{GLYPH_LOG} "), theme.dim()));
                    spans.push(Span::styled(row.name.clone(), theme.dim()));
                }
                LeafKind::Broken(_) => {
                    spans.push(Span::styled(format!("{GLYPH_BROKEN} "), theme.error()));
                    spans.push(Span::styled(row.name.clone(), theme.error()));
                }
            },
        }
        let mut line = Line::from(spans);
        if selected {
            line = line.style(theme.selection());
        }
        lines.push(line);
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

#[allow(clippy::too_many_lines)]
fn draw_document(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let snapshot = app.snapshot.as_ref().expect("drawn only with a snapshot");
    let focused = app.explorer.pane == ExplorerPane::Viewer;

    let (title, header, body): (String, Vec<Line<'static>>, Option<String>) =
        match &app.explorer.selected {
            Some(TreeSel::Concept(id)) => snapshot.bundle.get(id).map_or_else(
                || (String::from(" ? "), Vec::new(), None),
                |concept| {
                    let meta = snapshot.meta(id);
                    let tier = meta.map_or(okf_core::TrustTier::Unverified, |m| m.tier);
                    let title = format!(
                        " {}.md · {} · {} {} ",
                        id,
                        concept.type_().unwrap_or_default(),
                        tier_glyph(tier),
                        tier
                    );
                    let header = frontmatter_header(app, concept);
                    (title, header, Some(concept.document.body.clone()))
                },
            ),
            Some(TreeSel::File(path)) => {
                let text = std::fs::read_to_string(path).unwrap_or_default();
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                (format!(" {name} "), Vec::new(), Some(text))
            }
            Some(TreeSel::Dir(path)) => (format!(" {path}/ "), Vec::new(), None),
            None => (" (nothing selected) ".to_string(), Vec::new(), None),
        };

    let block = Block::new()
        .borders(Borders::ALL)
        .title_bottom(Line::from(Span::styled(
            truncate_to_width(&title, usize::from(area.width).saturating_sub(2)),
            theme.dim(),
        )))
        .border_style(if focused { theme.accent() } else { theme.dim() });
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut y = inner.y;
    let header_max = (inner.height / 3).min(12);
    let header_shown = header.len().min(usize::from(header_max));
    if header_shown > 0 {
        let header_area = Rect {
            x: inner.x,
            y,
            width: inner.width,
            height: u16::try_from(header_shown).unwrap_or(0),
        };
        frame.render_widget(Paragraph::new(header[..header_shown].to_vec()), header_area);
        y += header_area.height;
        if y < inner.y + inner.height {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    "─".repeat(usize::from(inner.width)),
                    theme.dim(),
                ))),
                Rect {
                    x: inner.x,
                    y,
                    width: inner.width,
                    height: 1,
                },
            );
            y += 1;
        }
    }
    let body_area = Rect {
        x: inner.x,
        y,
        width: inner.width,
        height: (inner.y + inner.height).saturating_sub(y),
    };
    if let Some(body) = body {
        let rendered = render_document(
            &body,
            body_area.width.saturating_sub(1),
            theme,
            app.explorer.focused_link,
        );
        let max_scroll = rendered
            .lines
            .len()
            .saturating_sub(usize::from(body_area.height));
        app.explorer.max_scroll.set(max_scroll);
        let scroll = app.explorer.scroll.min(max_scroll);
        let visible: Vec<Line<'static>> = rendered
            .lines
            .into_iter()
            .skip(scroll)
            .take(usize::from(body_area.height))
            .collect();
        frame.render_widget(Paragraph::new(visible), body_area);
    } else {
        app.explorer.max_scroll.set(0);
    }
}

fn frontmatter_header(app: &App, concept: &okf_core::Concept) -> Vec<Line<'static>> {
    let theme = &app.theme;
    let fm = &concept.document.frontmatter;
    if app.explorer.raw_yaml {
        return fm
            .to_string()
            .lines()
            .map(|l| Line::from(Span::styled(l.to_string(), theme.dim())))
            .collect();
    }
    let mut lines = Vec::new();
    let mut push = |label: &str, value: String, style: Style| {
        if !value.is_empty() {
            lines.push(Line::from(vec![
                Span::styled(format!("{label:<9}"), theme.dim()),
                Span::styled(value, style),
            ]));
        }
    };
    push(
        "title",
        concept.display_title(),
        Style::default().add_modifier(Modifier::BOLD),
    );
    push(
        "type",
        fm.type_()
            .map(std::borrow::Cow::into_owned)
            .unwrap_or_default(),
        Style::default(),
    );
    let status = fm.status();
    push(
        "status",
        format!("{} {status}", status_glyph(&status)),
        theme.status(&status),
    );
    let tier = fm.trust_tier();
    push(
        "trust",
        format!("{} {tier}", tier_glyph(tier)),
        theme.tier(tier),
    );
    if let Some(stale) = fm.stale_after() {
        push("fresh", format!("until {}", stale.raw), Style::default());
    }
    if !fm.tags().is_empty() {
        push("tags", fm.tags().join(", "), theme.accent());
    }
    lines
}

#[allow(clippy::too_many_lines)]
fn draw_inspector(frame: &mut Frame, app: &App, area: Rect) {
    let theme = &app.theme;
    let snapshot = app.snapshot.as_ref().expect("drawn only with a snapshot");
    let tabs = ["Meta", "Links", "Sources", "History"];
    let mut title_spans = vec![Span::raw(" ")];
    for (i, name) in tabs.iter().enumerate() {
        let style = if i == app.explorer.inspector_tab {
            theme.accent().add_modifier(Modifier::REVERSED)
        } else {
            theme.dim()
        };
        title_spans.push(Span::styled(format!("{name} "), style));
    }
    let block = Block::new()
        .borders(Borders::ALL)
        .title(Line::from(title_spans))
        .title_bottom(Line::from(Span::styled(" i cycles ", theme.dim())))
        .border_style(theme.dim());
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(TreeSel::Concept(id)) = &app.explorer.selected else {
        return;
    };
    let Some(concept) = snapshot.bundle.get(id) else {
        return;
    };
    let width = usize::from(inner.width);
    let lines = match app.explorer.inspector_tab {
        1 => inspector_links(app, id, width),
        2 => inspector_sources(app, id, width),
        3 => inspector_history(app, id, width),
        _ => inspector_meta(app, concept, width),
    };
    let visible: Vec<Line<'static>> = lines.into_iter().take(usize::from(inner.height)).collect();
    frame.render_widget(Paragraph::new(visible), inner);
}

fn kv(label: &str, value: String, style: Style, theme: crate::theme::Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<9}"), theme.dim()),
        Span::styled(value, style),
    ])
}

fn inspector_meta(app: &App, concept: &okf_core::Concept, width: usize) -> Vec<Line<'static>> {
    let theme = &app.theme;
    let snapshot = app.snapshot.as_ref().expect("drawn only with a snapshot");
    let fm = &concept.document.frontmatter;
    let meta = snapshot.meta(&concept.id);
    let mut lines = Vec::new();
    lines.push(kv(
        "type",
        concept.type_().unwrap_or_default().into_owned(),
        Style::default(),
        *theme,
    ));
    let status = fm.status();
    lines.push(kv(
        "status",
        format!("{} {status}", status_glyph(&status)),
        theme.status(&status),
        *theme,
    ));
    let tier = fm.trust_tier();
    lines.push(kv(
        "trust",
        format!("{} {tier}", tier_glyph(tier)),
        theme.tier(tier),
        *theme,
    ));
    if let Some(stale) = fm.stale_after() {
        let style = if meta.is_some_and(|m| m.stale) {
            theme.warn()
        } else {
            Style::default()
        };
        lines.push(kv("fresh", format!("until {}", stale.raw), style, *theme));
    }
    if let Some(generated) = fm.generated() {
        lines.push(Line::from(Span::styled("generated", theme.dim())));
        let by = generated.by.map(|b| b.to_string()).unwrap_or_default();
        let by_kind = okf_core::Actor::parse(by.clone()).kind();
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                truncate_to_width(&by, width.saturating_sub(2)),
                theme.actor(by_kind),
            ),
        ]));
        if let Some(at) = generated.at {
            lines.push(Line::from(Span::styled(
                format!("  {}", at.raw),
                theme.dim(),
            )));
        }
    }
    let verified = fm.verified();
    if !verified.is_empty() {
        lines.push(Line::from(Span::styled("verified", theme.dim())));
        for event in verified {
            let by = event.by.map(|b| b.to_string()).unwrap_or_default();
            let kind = okf_core::Actor::parse(by.clone()).kind();
            let at = event.at.map(|a| a.raw).unwrap_or_default();
            lines.push(Line::from(vec![
                Span::styled(format!("  {GLYPH_OK} "), theme.ok()),
                Span::styled(
                    truncate_to_width(&by, width.saturating_sub(8)),
                    theme.actor(kind),
                ),
            ]));
            if !at.is_empty() {
                lines.push(Line::from(Span::styled(format!("    {at}"), theme.dim())));
            }
        }
    }
    if let Some(m) = meta {
        lines.push(kv(
            "links",
            format!("{} out · {} in", m.out_degree, m.in_degree),
            Style::default(),
            *theme,
        ));
        lines.push(kv(
            "srcs",
            format!("{}", m.source_count),
            Style::default(),
            *theme,
        ));
    }
    let extensions = fm.extension_keys();
    if !extensions.is_empty() {
        lines.push(kv("ext keys", extensions.join(", "), theme.dim(), *theme));
    }
    let legacy = fm.legacy_keys();
    if !legacy.is_empty() {
        lines.push(kv(
            "legacy",
            format!("{} (migrate)", legacy.join(", ")),
            theme.warn(),
            *theme,
        ));
    }
    lines
}

fn inspector_links(app: &App, id: &ConceptId, width: usize) -> Vec<Line<'static>> {
    let theme = &app.theme;
    let snapshot = app.snapshot.as_ref().expect("drawn only with a snapshot");
    let mut lines = vec![Line::from(Span::styled("OUT", theme.dim()))];
    for link in snapshot.bundle.links_from(id) {
        let (glyph, style) = if link.exists {
            (GLYPH_OK, theme.ok())
        } else {
            (GLYPH_BROKEN, theme.error())
        };
        lines.push(Line::from(vec![
            Span::styled(format!(" {glyph} "), style),
            Span::raw(truncate_to_width(
                &link.target.to_string(),
                width.saturating_sub(4),
            )),
        ]));
    }
    lines.push(Line::from(Span::styled("IN", theme.dim())));
    for backlink in snapshot.bundle.backlinks(id) {
        lines.push(Line::from(vec![
            Span::styled(" ← ".to_string(), theme.accent()),
            Span::raw(truncate_to_width(
                &backlink.to_string(),
                width.saturating_sub(4),
            )),
        ]));
    }
    lines
}

fn inspector_sources(app: &App, id: &ConceptId, width: usize) -> Vec<Line<'static>> {
    let theme = &app.theme;
    let snapshot = app.snapshot.as_ref().expect("drawn only with a snapshot");
    let concept = snapshot.bundle.get(id);
    let attributions = concept
        .map(|c| c.document.attributions())
        .unwrap_or_default();
    let mut lines = Vec::new();
    for resolved in snapshot.bundle.sources_of(id) {
        let source = &resolved.source;
        let icon = match source.resource_kind() {
            ResourceKind::Url => "🌐",
            ResourceKind::Path => "▤",
            ResourceKind::Scope => "◌",
            ResourceKind::Missing => GLYPH_BROKEN,
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{icon} "), theme.accent()),
            Span::raw(truncate_to_width(source.label(), width.saturating_sub(3))),
        ]));
        if let Some(author) = &source.author {
            lines.push(Line::from(Span::styled(
                format!("   by {author}"),
                theme.actor(author.kind()),
            )));
        }
        if let Some(modified) = &source.last_modified {
            lines.push(Line::from(Span::styled(
                format!("   mod {}", modified.raw),
                theme.dim(),
            )));
        }
        if let Some(count) = source.usage_count {
            lines.push(Line::from(Span::styled(
                format!("   used {count}×"),
                theme.dim(),
            )));
        }
        let cited = source.id.as_ref().is_some_and(|sid| {
            attributions
                .iter()
                .any(|a| a.label == *sid && a.references > 0)
        });
        if cited {
            lines.push(Line::from(Span::styled(
                format!("   {GLYPH_OK} cited"),
                theme.ok(),
            )));
        } else {
            lines.push(Line::from(Span::styled(
                format!("   {GLYPH_WARN} uncited"),
                theme.warn(),
            )));
        }
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled("(no sources)", theme.dim())));
    }
    lines
}

fn inspector_history(app: &App, id: &ConceptId, width: usize) -> Vec<Line<'static>> {
    let theme = &app.theme;
    let snapshot = app.snapshot.as_ref().expect("drawn only with a snapshot");
    let id_str = id.to_string();
    let name = id.name();
    let mut lines = Vec::new();
    for (date, entries) in &snapshot.log_days {
        let matching: Vec<&okf_core::LogEntry> = entries
            .iter()
            .filter(|entry| entry.text.contains(&id_str) || entry.text.contains(name))
            .collect();
        if matching.is_empty() {
            continue;
        }
        lines.push(Line::from(Span::styled(date.clone(), theme.accent())));
        for entry in matching {
            let kind = entry.kind.clone().unwrap_or_default();
            lines.push(Line::from(vec![
                Span::styled(
                    format!(" {kind} "),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw(truncate_to_width(
                    &entry.text,
                    width.saturating_sub(str_width(&kind) + 2),
                )),
            ]));
        }
    }
    if lines.is_empty() {
        lines.push(Line::from(Span::styled("(no log mentions)", theme.dim())));
    }
    lines
}
