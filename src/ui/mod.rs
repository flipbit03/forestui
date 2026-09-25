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
use unicode_width::UnicodeWidthStr;

pub const SIDEBAR_WIDTH: u16 = 35;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    // Clickable regions are rebuilt every frame; nothing persists between them.
    app.clear_hits();
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::active().bg)),
        area,
    );

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

    let hover_reveal = sidebar::draw(frame, app, sidebar_area);
    detail::draw(frame, app, detail_area);
    // Painted after the detail pane so a hovered row that is too wide for the
    // sidebar spills its full text over the divider instead of under it.
    // Skipped under modals: their backdrop repaints the panes anyway, and the
    // theme picker (which keeps the panes visible as its preview) must not
    // have a stray hover band floating through it.
    if app.modals.is_empty()
        && let Some(reveal) = hover_reveal
    {
        sidebar::draw_hover_reveal(frame, reveal);
    }
    draw_footer(frame, app, footer_area);
    // Toasts are hidden while any modal is open — under most modals the
    // backdrop wipes them anyway; the picker skips the backdrop, so it has to
    // skip the toasts explicitly or they float over the preview.
    if app.modals.is_empty() {
        draw_notifications(frame, app, body_area);
    }

    if !app.modals.is_empty() {
        // Textual's `ModalScreen` painted over the app rather than tinting it —
        // nothing behind a dialog is visible. A translucent backdrop leaves the
        // pane competing with the dialog for the eye. The theme picker is the
        // one exception: the panes behind it ARE its preview — moving the
        // highlight repaints the whole app in the candidate theme, so covering
        // the app would leave nothing to preview against.
        if !matches!(app.modals.last(), Some(crate::modal::Modal::ThemePicker(_))) {
            frame.render_widget(Clear, area);
            frame.render_widget(
                Block::default().style(Style::default().bg(theme::active().bg)),
                area,
            );
        }
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
    //
    // A pending update rides along until the process exits, in the compact
    // wording when the full one would not fit — the version and "restart"
    // matter more than the rest of the sentence, and a clipped suffix would
    // lose exactly the part that says what to do.
    let title = app.title();
    let width = area.width as usize;
    let suffix = app.title_suffix(false).and_then(|full| {
        if title.width() + 1 + full.width() <= width {
            Some(full)
        } else {
            app.title_suffix(true)
        }
    });
    let text_width = title.width() + suffix.as_ref().map_or(0, |s| 1 + s.width());
    let indent = width.saturating_sub(text_width) / 2;
    let mut spans = vec![
        Span::raw(" ".repeat(indent)),
        Span::styled(title, theme::title()),
    ];
    if let Some(suffix) = suffix {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(suffix, theme::title_notice()));
    }
    let line = Line::from(spans);
    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(theme::active().bg_elevated)),
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
                .bg(theme::active().accent_dark)
                .fg(theme::active().text_primary),
        ));
        spans.push(Span::styled(
            text,
            if hovered {
                Style::default()
                    .bg(theme::active().bg_hover)
                    .fg(theme::active().text_primary)
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
        Paragraph::new(Line::from(spans)).style(Style::default().bg(theme::active().bg_elevated)),
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
    let palette = theme::active();
    for notification in &app.notifications {
        let style = match notification.severity {
            Severity::Information => Style::default()
                .bg(palette.accent_dark)
                .fg(palette.text_primary),
            Severity::Warning => Style::default().bg(palette.warning).fg(palette.bg),
            Severity::Error => Style::default().bg(palette.destructive).fg(palette.bg),
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

    fn header_row(terminal: &ratatui::Terminal<ratatui::backend::TestBackend>) -> String {
        let buffer = terminal.backend().buffer();
        (0..buffer.area.width)
            .map(|col| buffer[(col, 0)].symbol().to_string())
            .collect()
    }

    /// With nothing pending the header is the bare, centred title — the same
    /// bar every baseline frame was captured against.
    #[tokio::test]
    async fn the_header_is_the_bare_title_with_no_update_pending() {
        use crate::app::test_support::app_with_fixture;
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let (_dir, mut app) = app_with_fixture();
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).expect("test terminal");
        terminal.draw(|frame| draw(frame, &mut app)).expect("draw");

        let row = header_row(&terminal);
        let title = app.title();
        assert_eq!(row.trim(), title);
        assert_eq!(row.find(&title), Some((120 - title.width()) / 2));
    }

    /// The acceptance case: after an update installs, the header reads
    /// `forestui vX (vY ready — restart to update)`, centred as one piece,
    /// with the suffix in the warning colour so it is seen, not just read.
    #[tokio::test]
    async fn a_pending_update_extends_the_title_bar() {
        use crate::app::test_support::app_with_fixture;
        use crate::version_check::UpdateStatus;
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        // The colours are asserted against the active theme, which other
        // tests switch; hold it still for the draw and the comparison.
        let _guard = crate::theme::test_lock();
        let (_dir, mut app) = app_with_fixture();
        app.handle_event(crate::event::AppEvent::UpdateChecked(
            UpdateStatus::Installed("9.9.9".into()),
        ));
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).expect("test terminal");
        terminal.draw(|frame| draw(frame, &mut app)).expect("draw");

        let row = header_row(&terminal);
        let expected = format!("{} (v9.9.9 ready — restart to update)", app.title());
        assert_eq!(row.trim(), expected);
        assert_eq!(row.find(&expected), Some((120 - expected.width()) / 2));

        let buffer = terminal.backend().buffer();
        let suffix_col = row[..row.find("(v9.9.9").expect("suffix drawn")].width() as u16;
        let title_col = row[..row.find("forestui").expect("title drawn")].width() as u16;
        assert_eq!(buffer[(suffix_col, 0)].fg, theme::active().warning);
        assert_eq!(buffer[(title_col, 0)].fg, theme::active().text_primary);
    }

    /// A bar too narrow for the full sentence keeps the version and the verb
    /// rather than clipping the part that says what to do.
    #[tokio::test]
    async fn a_narrow_header_uses_the_compact_update_notice() {
        use crate::app::test_support::app_with_fixture;
        use crate::version_check::UpdateStatus;
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let (_dir, mut app) = app_with_fixture();
        app.handle_event(crate::event::AppEvent::UpdateChecked(
            UpdateStatus::Installed("9.9.9".into()),
        ));
        let full = format!("{} (v9.9.9 ready — restart to update)", app.title());
        let compact = format!("{} (v9.9.9 — restart)", app.title());
        let width = (full.width() - 1) as u16;
        assert!(compact.width() <= width as usize);

        let mut terminal = Terminal::new(TestBackend::new(width, 20)).expect("test terminal");
        terminal.draw(|frame| draw(frame, &mut app)).expect("draw");
        assert_eq!(header_row(&terminal).trim(), compact);

        // Exactly wide enough for the full wording: it is used.
        let mut terminal =
            Terminal::new(TestBackend::new(full.width() as u16, 20)).expect("test terminal");
        terminal.draw(|frame| draw(frame, &mut app)).expect("draw");
        assert_eq!(header_row(&terminal).trim(), full);
    }

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

    /// Hovering a row wider than the sidebar reveals the whole name across
    /// the divider; the same row un-hovered stays clipped at the divider.
    #[tokio::test]
    async fn a_hovered_overlong_row_reveals_past_the_divider() {
        use crate::app::test_support::app_with_fixture;
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let long_name = "a-worktree-name-far-wider-than-the-sidebar-allows";
        let (_dir, mut app) = app_with_fixture();
        if let Some(crate::app::SidebarRow::Worktree { name, .. }) = app.rows.get_mut(1) {
            *name = long_name.into();
        } else {
            panic!("fixture row 1 is not a worktree");
        }

        let row_text = |terminal: &Terminal<TestBackend>, row: u16| -> String {
            let buffer = terminal.backend().buffer();
            (0..buffer.area.width)
                .map(|col| buffer[(col, row)].symbol().chars().next().unwrap_or(' '))
                .collect()
        };
        // Header row + the 3-row gh-status box put the first tree row at y=4;
        // the mutated worktree is the row below it.
        let worktree_row = 5;

        let mut terminal = Terminal::new(TestBackend::new(120, 40)).expect("test terminal");
        terminal.draw(|frame| draw(frame, &mut app)).expect("draw");
        assert!(
            !row_text(&terminal, worktree_row).contains(long_name),
            "an un-hovered overlong row must clip at the divider"
        );

        app.hovered = Some(HitTarget::SidebarRow(1));
        terminal.draw(|frame| draw(frame, &mut app)).expect("draw");
        let revealed = row_text(&terminal, worktree_row);
        let end = revealed
            .find(long_name)
            .expect("the hovered row must reveal its full name across the divider")
            + long_name.len();
        assert!(
            revealed[end..].trim_start().starts_with('│'),
            "the reveal must close with a right border, got: {:?}",
            &revealed[end..]
        );
        // Zero horizontal shift: the left border chomps the one-cell tree
        // angle in column 0 and everything after it keeps its column.
        assert!(
            revealed.starts_with("│─ "),
            "the left border must replace the angle without shifting the text, got: {:?}",
            &revealed[..8]
        );
        // The box: its border rows sit on the neighbouring lines.
        assert!(
            row_text(&terminal, worktree_row - 1).contains('┌'),
            "the reveal must draw a top border above the row"
        );
        assert!(
            row_text(&terminal, worktree_row + 1).contains('└'),
            "the reveal must draw a bottom border below the row"
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
