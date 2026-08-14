//! Rendering. One `draw` per frame, one function per pane.

pub mod detail;
pub mod modals;
pub mod sidebar;
pub mod widgets;

use crate::app::App;
use crate::event::Severity;
use crate::theme;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

pub const SIDEBAR_WIDTH: u16 = 35;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    // Clickable regions are rebuilt every frame; nothing persists between them.
    app.clear_hits();
    frame.render_widget(Block::default().style(Style::default().bg(theme::BG)), area);

    let [header_area, body_area, footer_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_header(frame, app, header_area);

    let sidebar_width = SIDEBAR_WIDTH.min(body_area.width.saturating_sub(10).max(1));
    let [sidebar_area, detail_area] =
        Layout::horizontal([Constraint::Length(sidebar_width), Constraint::Min(0)])
            .areas(body_area);

    sidebar::draw(frame, app, sidebar_area);
    detail::draw(frame, app, detail_area);
    draw_footer(frame, footer_area);
    draw_notifications(frame, app, body_area);

    if !app.modals.is_empty() {
        modals::draw(frame, app, area);
    }
}

/// Spans for a control ("button") so it reads as one: a filled pill with
/// rounded caps. A single row cannot carry a real border, and boxing every
/// control the way Textual did would push the pane back into scrolling.
///
/// The caps are half blocks drawn in the button's own colour *over the page
/// background*, so only the inner half of the cell is filled and the ends look
/// rounded rather than sawn off.
pub fn button(label: &str, focused: bool, destructive: bool) -> Vec<Span<'static>> {
    let fill = theme::action_bg(focused, destructive);
    let cap = Style::default().fg(fill).bg(theme::BG);
    vec![
        Span::styled("▐", cap),
        Span::styled(format!(" {label} "), theme::action(focused, destructive)),
        Span::styled("▌", cap),
    ]
}

/// Rendered width of [`button`], for hit-testing and layout.
pub fn button_width(label: &str) -> u16 {
    u16::try_from(label.chars().count() + 4).unwrap_or(u16::MAX)
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let line = Line::from(vec![Span::styled(
        format!(" {}", app.title()),
        theme::title(),
    )]);
    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(theme::BG_ELEVATED)),
        area,
    );
}

fn draw_footer(frame: &mut Frame, area: Rect) {
    const KEYS: [(&str, &str); 12] = [
        ("q", "Quit"),
        ("a", "Add Repo"),
        ("w", "Add Worktree"),
        ("e", "Editor"),
        ("t", "Terminal"),
        ("o", "Files"),
        ("n", "Claude"),
        ("y", "ClaudeYOLO"),
        ("h", "Archive"),
        ("d", "Delete"),
        ("s", "Settings"),
        ("?", "Help"),
    ];

    let mut spans = Vec::new();
    for (key, label) in KEYS {
        spans.push(Span::styled(
            format!(" {key} "),
            Style::default()
                .bg(theme::ACCENT_DARK)
                .fg(theme::TEXT_PRIMARY),
        ));
        spans.push(Span::styled(format!(" {label}  "), theme::secondary()));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(theme::BG_ELEVATED)),
        area,
    );
}

fn draw_notifications(frame: &mut Frame, app: &App, area: Rect) {
    if app.notifications.is_empty() {
        return;
    }
    let width = area.width.saturating_sub(4).min(60);
    if width < 10 {
        return;
    }
    let height = (app.notifications.len() as u16).min(area.height);
    let rect = Rect {
        x: area.x + area.width.saturating_sub(width + 2),
        y: area.y + area.height.saturating_sub(height + 1),
        width,
        height,
    };

    let lines: Vec<Line> = app
        .notifications
        .iter()
        .map(|notification| {
            let style = match notification.severity {
                Severity::Information => Style::default()
                    .bg(theme::ACCENT_DARK)
                    .fg(theme::TEXT_PRIMARY),
                Severity::Warning => Style::default().bg(theme::WARNING).fg(theme::BG),
                Severity::Error => Style::default().bg(theme::DESTRUCTIVE).fg(theme::BG),
            };
            Line::from(Span::styled(
                format!(
                    " {} ",
                    crate::util::truncate(&notification.text, width as usize - 2)
                ),
                style,
            ))
        })
        .collect();

    frame.render_widget(Clear, rect);
    frame.render_widget(Paragraph::new(lines), rect);
}
