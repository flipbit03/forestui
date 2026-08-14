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
use ratatui::style::{Color, Style};
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
        // Textual dimmed whatever sat behind a modal screen. Without it the
        // dialog competes with a fully-lit pane and stops reading as modal.
        dim(frame, area);
        modals::draw(frame, app, area);
    }
}

/// Darken everything already drawn, so the modal on top stands out.
fn dim(frame: &mut Frame, area: Rect) {
    let buffer = frame.buffer_mut();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(cell) = buffer.cell_mut((x, y)) {
                let fg = darken(cell.fg);
                let bg = darken(cell.bg);
                cell.set_fg(fg);
                cell.set_bg(bg);
            }
        }
    }
}

fn darken(color: Color) -> Color {
    /// The page background, pre-dimmed, for cells the terminal draws default.
    const DIMMED_PAGE: Color = Color::Rgb(0x1C / 3, 0x1C / 3, 0x1E / 3);
    match color {
        Color::Rgb(r, g, b) => Color::Rgb(r / 3, g / 3, b / 3),
        // An unstyled cell shows the terminal default; treat it as the page
        // colour so the backdrop dims evenly instead of leaving bright gaps.
        Color::Reset => DIMMED_PAGE,
        other => other,
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
    // Wrap rather than truncate: the help toast is a full key list, and cutting
    // it at one line hides most of what the user pressed `?` to read.
    let inner_width = width as usize - 2;
    let mut lines: Vec<Line> = Vec::new();
    for notification in &app.notifications {
        let style = match notification.severity {
            Severity::Information => Style::default()
                .bg(theme::ACCENT_DARK)
                .fg(theme::TEXT_PRIMARY),
            Severity::Warning => Style::default().bg(theme::WARNING).fg(theme::BG),
            Severity::Error => Style::default().bg(theme::DESTRUCTIVE).fg(theme::BG),
        };
        for chunk in wrap_words(&notification.text, inner_width) {
            lines.push(Line::from(Span::styled(
                format!(" {chunk:<inner_width$} "),
                style,
            )));
        }
    }

    let height = (lines.len() as u16).min(area.height.saturating_sub(1));
    if height == 0 {
        return;
    }
    lines.truncate(height as usize);
    let rect = Rect {
        x: area.x + area.width.saturating_sub(width + 2),
        y: area.y + area.height.saturating_sub(height + 1),
        width,
        height,
    };

    frame.render_widget(Clear, rect);
    frame.render_widget(Paragraph::new(lines), rect);
}

/// Break text into lines of at most `width` characters, on word boundaries
/// where possible.
fn wrap_words(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let candidate = if current.is_empty() {
            word.to_string()
        } else {
            format!("{current} {word}")
        };
        if candidate.chars().count() <= width {
            current = candidate;
            continue;
        }
        if !current.is_empty() {
            lines.push(std::mem::take(&mut current));
        }
        // A single word longer than the line has to be split somewhere.
        let mut rest: Vec<char> = word.chars().collect();
        while rest.len() > width {
            lines.push(rest.drain(..width).collect());
        }
        current = rest.into_iter().collect();
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_words_breaks_on_word_boundaries() {
        let wrapped = wrap_words("a: Add Repo | w: Add Worktree | e: Editor", 20);
        assert!(wrapped.iter().all(|l| l.chars().count() <= 20));
        assert!(wrapped.len() > 1, "the help text has to wrap, not truncate");
        // Nothing may be lost: the words all survive, in order.
        let joined = wrapped.join(" ");
        assert_eq!(
            joined.split_whitespace().collect::<Vec<_>>(),
            "a: Add Repo | w: Add Worktree | e: Editor"
                .split_whitespace()
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn wrap_words_splits_an_overlong_word() {
        let wrapped = wrap_words("supercalifragilistic", 6);
        assert!(wrapped.iter().all(|l| l.chars().count() <= 6));
        assert_eq!(wrapped.concat(), "supercalifragilistic");
        assert!(wrap_words("anything", 0).is_empty());
    }

    #[test]
    fn button_width_counts_the_caps() {
        // Two cap cells plus a space either side of the label.
        assert_eq!(button_width("Editor") as usize, "Editor".len() + 4);
        assert_eq!(button("Editor", false, false).len(), 3);
    }
}
