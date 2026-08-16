//! Rendering. One `draw` per frame, one function per pane.

pub mod detail;
pub mod modals;
pub mod sidebar;
pub mod widgets;

use crate::app::{App, HitTarget};
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

    // Remembered so the wheel can be routed to the pane under the pointer.
    app.sidebar_rect = sidebar_area;
    app.detail_rect = detail_area;

    sidebar::draw(frame, app, sidebar_area);
    detail::draw(frame, app, detail_area);
    draw_footer(frame, app, footer_area);
    draw_notifications(frame, app, body_area);

    if !app.modals.is_empty() {
        // Textual's `ModalScreen` painted over the app rather than tinting it —
        // nothing behind a dialog is visible. A translucent backdrop leaves the
        // pane competing with the dialog for the eye.
        frame.render_widget(Clear, area);
        frame.render_widget(Block::default().style(Style::default().bg(theme::BG)), area);
        modals::draw(frame, app, area);
    }
}

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    // Textual's stock `Header` also drew a `⭘` hard left. There it was a live
    // control that opened the app menu; nothing here is behind it, and a button
    // that cannot be pressed is worse than no button. Deliberately dropped, so
    // this bar is one of the few places the Rust build does not match the
    // Textual frame — the committed `baseline/python` frames still show it.
    // The title keeps its position: it is centred on the whole bar either way.
    let title = app.title();
    let indent = (area.width as usize).saturating_sub(title.chars().count()) / 2;
    let line = Line::from(vec![
        Span::raw(" ".repeat(indent)),
        Span::styled(title, theme::title()),
    ]);
    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(theme::BG_ELEVATED)),
        area,
    );
}

fn draw_footer(frame: &mut Frame, app: &mut App, area: Rect) {
    let mut spans = Vec::new();
    // Tracks where each entry lands so it can be registered as a click target.
    let mut x = area.x;
    for (key, label) in crate::app::BINDINGS {
        let hovered = app.hovered == Some(HitTarget::FooterKey(key));
        let badge = format!(" {key} ");
        // `{label} `, matching Textual exactly: the badge already carries a
        // space inside its own background on each side, so the key sits one
        // column from its label and the single trailing space here plus the next
        // badge's leading one put two between entries. The original
        // ` {label}  ` spent two columns in each place, which pushed the last
        // entries off a 150-column terminal that Textual fitted. The footer is
        // the complete key surface, so entries that do not fit simply clip —
        // a narrow terminal shows a prefix, never a different set.
        let text = format!("{label} ");
        let width = (badge.chars().count() + text.chars().count()) as u16;

        spans.push(Span::styled(
            badge,
            Style::default()
                .bg(theme::ACCENT_DARK)
                .fg(theme::TEXT_PRIMARY),
        ));
        spans.push(Span::styled(
            text,
            if hovered {
                Style::default().bg(theme::BG_HOVER).fg(theme::TEXT_PRIMARY)
            } else {
                theme::secondary()
            },
        ));

        // Only a fully visible entry is clickable. A clipped one would be an
        // invisible control: at some widths its only remaining cell is a bare
        // accent space that would still fire a state-mutating binding.
        if x + width <= area.x + area.width {
            app.push_hit(
                Rect {
                    x,
                    y: area.y,
                    width,
                    height: area.height,
                },
                HitTarget::FooterKey(key),
            );
        }
        x = x.saturating_add(width);
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(theme::BG_ELEVATED)),
        area,
    );
}

fn draw_notifications(frame: &mut Frame, app: &mut App, area: Rect) {
    if app.notifications.is_empty() {
        return;
    }
    let width = area.width.saturating_sub(4).min(60);
    if width < 10 {
        return;
    }
    // Wrap rather than truncate: notifications carry full sentences (git
    // errors, update failures), and cutting them at one line hides the part
    // the user needed to read.
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
    // Recorded after the panes, so it wins the cells it covers.
    app.push_hit(rect, HitTarget::Notification);
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

    /// The toast is drawn over the detail pane, so the frame that draws it has
    /// to claim its cells — otherwise a click there reaches the control beneath.
    #[tokio::test]
    async fn drawing_a_toast_records_its_region() {
        use crate::app::test_support::app_with_fixture;
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let (_dir, mut app) = app_with_fixture();
        app.notify("something happened", Severity::Information);

        let mut terminal = Terminal::new(TestBackend::new(120, 40)).expect("test terminal");
        terminal.draw(|frame| draw(frame, &mut app)).expect("draw");

        assert!(
            app.hits
                .iter()
                .any(|hit| hit.target == HitTarget::Notification),
            "the toast drew but claimed no cells"
        );
    }

    /// End-to-end wrap coverage: with the sweep's help-toast case retired,
    /// this is what guards `draw_notifications`' own arithmetic — a long git
    /// or update error must occupy several drawn rows, never clamp to one.
    #[tokio::test]
    async fn a_long_notification_draws_over_multiple_lines() {
        use crate::app::test_support::app_with_fixture;
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let (_dir, mut app) = app_with_fixture();
        app.notify(
            "Could not delete 'wt-a': fatal: 'wt-a' contains modified or \
             untracked files, use --force to delete it anyway",
            Severity::Error,
        );

        let mut terminal = Terminal::new(TestBackend::new(80, 30)).expect("test terminal");
        terminal.draw(|frame| draw(frame, &mut app)).expect("draw");

        let buffer = terminal.backend().buffer();
        let rows_with_text = (0..buffer.area.height)
            .filter(|&row| {
                let line: String = (0..buffer.area.width)
                    .map(|col| buffer[(col, row)].symbol().chars().next().unwrap_or(' '))
                    .collect();
                line.contains("wt-a") || line.contains("--force") || line.contains("untracked")
            })
            .count();
        assert!(
            rows_with_text > 1,
            "the notification text occupies {rows_with_text} row(s); it must wrap"
        );
    }

    #[test]
    fn wrap_words_breaks_on_word_boundaries() {
        let message = "Could not delete 'wt-a': fatal: 'wt-a' contains modified or untracked files";
        let wrapped = wrap_words(message, 20);
        assert!(wrapped.iter().all(|l| l.chars().count() <= 20));
        assert!(wrapped.len() > 1, "a long error has to wrap, not truncate");
        // Nothing may be lost: the words all survive, in order.
        let joined = wrapped.join(" ");
        assert_eq!(
            joined.split_whitespace().collect::<Vec<_>>(),
            message.split_whitespace().collect::<Vec<_>>()
        );
    }

    #[test]
    fn wrap_words_splits_an_overlong_word() {
        let wrapped = wrap_words("supercalifragilistic", 6);
        assert!(wrapped.iter().all(|l| l.chars().count() <= 6));
        assert_eq!(wrapped.concat(), "supercalifragilistic");
        assert!(wrap_words("anything", 0).is_empty());
    }
}
