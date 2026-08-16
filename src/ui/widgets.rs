//! Small building blocks the panes and modals share.

use crate::theme;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

/// A single-line text field with a cursor.
///
/// Hand-rolled rather than pulled from a crate: the app needs exactly insert,
/// delete, and horizontal cursor movement, and owning it keeps the widget's
/// behaviour identical across every modal.
#[derive(Debug, Clone, Default)]
pub struct TextInput {
    value: String,
    /// Cursor position as a character index.
    cursor: usize,
    pub placeholder: String,
    pub max_length: Option<usize>,
}

impl TextInput {
    pub fn new(value: impl Into<String>) -> Self {
        let value = value.into();
        let cursor = value.chars().count();
        Self {
            value,
            cursor,
            placeholder: String::new(),
            max_length: None,
        }
    }

    pub fn with_placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    pub fn with_max_length(mut self, max: usize) -> Self {
        self.max_length = Some(max);
        self
    }

    pub fn value(&self) -> &str {
        &self.value
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn is_empty(&self) -> bool {
        self.value.is_empty()
    }

    pub fn set_value(&mut self, value: impl Into<String>) {
        self.value = value.into();
        self.cursor = self.value.chars().count();
    }

    fn byte_index(&self, char_index: usize) -> usize {
        self.value
            .char_indices()
            .nth(char_index)
            .map(|(i, _)| i)
            .unwrap_or(self.value.len())
    }

    pub fn insert(&mut self, ch: char) {
        if let Some(max) = self.max_length
            && self.value.chars().count() >= max
        {
            return;
        }
        let at = self.byte_index(self.cursor);
        self.value.insert(at, ch);
        self.cursor += 1;
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let at = self.byte_index(self.cursor - 1);
        self.value.remove(at);
        self.cursor -= 1;
    }

    pub fn delete(&mut self) {
        if self.cursor >= self.value.chars().count() {
            return;
        }
        let at = self.byte_index(self.cursor);
        self.value.remove(at);
    }

    pub fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn move_right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.value.chars().count());
    }

    pub fn move_home(&mut self) {
        self.cursor = 0;
    }

    pub fn move_end(&mut self) {
        self.cursor = self.value.chars().count();
    }

    /// Apply a text-editing key. Returns whether the key was consumed.
    ///
    /// The one implementation for every text field in the app — the modals and
    /// the detail pane's rename fields both route here, so an editing
    /// improvement (word delete, paste, a cursor fix) cannot land in one and
    /// silently miss the other. Callers layer their own Enter/Escape handling.
    pub fn apply_edit_key(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.kill_to_start();
                true
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.insert(c);
                true
            }
            KeyCode::Backspace => {
                self.backspace();
                true
            }
            KeyCode::Delete => {
                self.delete();
                true
            }
            KeyCode::Left => {
                self.move_left();
                true
            }
            KeyCode::Right => {
                self.move_right();
                true
            }
            KeyCode::Home => {
                self.move_home();
                true
            }
            KeyCode::End => {
                self.move_end();
                true
            }
            _ => false,
        }
    }

    /// Delete every character before the cursor (readline's `Ctrl+U`).
    pub fn kill_to_start(&mut self) {
        let at = self.byte_index(self.cursor);
        self.value.drain(..at);
        self.cursor = 0;
    }

    /// Render into `area`, drawing a block whose border shows focus.
    pub fn render(&self, frame: &mut Frame, area: Rect, focused: bool) {
        let border_style = if focused {
            theme::border_focused()
        } else {
            theme::border()
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        // The placeholder stays visible while the field is focused and empty,
        // the way Textual's Input behaved — it is the only hint of what the
        // field wants.
        let line = if self.value.is_empty() {
            Line::from(Span::styled(self.placeholder.clone(), theme::muted()))
        } else {
            Line::from(Span::styled(self.value.clone(), theme::primary()))
        };
        frame.render_widget(Paragraph::new(line), inner);

        if focused && inner.width > 0 {
            let offset = (self.cursor as u16).min(inner.width.saturating_sub(1));
            frame.set_cursor_position((inner.x + offset, inner.y));
        }
    }
}

/// Render a label/value line with a section-header style label.
pub fn section(title: &str) -> Line<'static> {
    Line::from(Span::styled(title.to_string(), theme::section_header()))
}

/// Render a modal's box, returning the inner area to draw into.
///
/// `.modal-container { background: $bg-elevated; border: solid $border }` — the
/// dialog is raised off the page and keeps the resting border colour. It never
/// takes the focus accent: a dialog is always the focused thing, so an accent
/// border there would say nothing and compete with the button that has one.
pub fn framed(frame: &mut Frame, area: Rect, title: &str) -> Rect {
    boxed(
        frame,
        area,
        title,
        theme::border(),
        theme::active().bg_elevated,
    )
}

fn boxed(frame: &mut Frame, area: Rect, title: &str, border_style: Style, bg: Color) -> Rect {
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .style(Style::default().bg(bg));
    // An empty title means "no title in the border" — the modals put theirs on
    // the first content row instead, and a `" "` title would punch a hole in
    // the top border.
    if !title.is_empty() {
        block = block.title(Span::styled(format!(" {title} "), theme::title()));
    }
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    inner
}

/// Rows a [`button_box`] occupies — Textual's `Button { height: 3 }`.
pub const BUTTON_HEIGHT: u16 = 3;
/// Narrowest a button may render — Textual's `Button { min-width: 10 }`.
const BUTTON_MIN_WIDTH: u16 = 10;

/// Rendered width of a [`button_box`]: the label padded a cell either side plus
/// the two border cells, never under Textual's minimum. Measured rather than
/// counted, because a label can hold a glyph that is not one cell wide.
pub fn button_box_width(label: &str) -> u16 {
    boxed_width(label, BUTTON_MIN_WIDTH)
}

/// [`button_box_width`] with an explicit minimum. The detail pane's controls
/// pass 0: their boxes hug the label, which is how that pane laid out from the
/// start and what the committed sweep baselines show.
pub fn boxed_width(label: &str, min_width: u16) -> u16 {
    let label = u16::try_from(Span::raw(label).width()).unwrap_or(u16::MAX);
    label.saturating_add(4).max(min_width)
}

/// The three rows of a bordered button, in the order they are drawn. The fill
/// runs under the border cells too, which is what Textual's `border: solid` did.
/// A label the minimum width has padded out is centred, as Textual centred it.
pub fn button_box(label: &str, border: Style, text: Style) -> [Vec<Span<'static>>; 3] {
    boxed_rows(label, border, text, BUTTON_MIN_WIDTH)
}

/// [`button_box`] with an explicit minimum width; see [`boxed_width`].
pub fn boxed_rows(
    label: &str,
    border: Style,
    text: Style,
    min_width: u16,
) -> [Vec<Span<'static>>; 3] {
    let inner = boxed_width(label, min_width).saturating_sub(2) as usize;
    let pad = inner.saturating_sub(Span::raw(label).width());
    let left = pad / 2;
    let edge = |left: char, right: char| {
        vec![Span::styled(
            format!("{left}{}{right}", "─".repeat(inner)),
            border,
        )]
    };
    [
        edge('┌', '┐'),
        vec![
            Span::styled("│", border),
            Span::styled(
                format!("{}{label}{}", " ".repeat(left), " ".repeat(pad - left)),
                text,
            ),
            Span::styled("│", border),
        ],
        edge('└', '┘'),
    ]
}

/// Centre a fixed-size rect inside `area`, clamped to the available space.
///
/// The clamp leaves a margin rather than running to the very edge — Textual's
/// modals capped at `max-width: 95%`, and a dialog flush against both sides
/// stops reading as a dialog.
pub fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let w = width
        .min(area.width.saturating_mul(95) / 100)
        .max(1)
        .min(area.width);
    let h = height
        .min(area.height.saturating_mul(95) / 100)
        .max(1)
        .min(area.height);
    Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w,
        height: h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editing_moves_the_cursor() {
        let mut input = TextInput::new("");
        for ch in "abc".chars() {
            input.insert(ch);
        }
        assert_eq!(input.value(), "abc");
        assert_eq!(input.cursor(), 3);

        input.move_left();
        input.insert('X');
        assert_eq!(input.value(), "abXc");

        input.backspace();
        assert_eq!(input.value(), "abc");

        input.move_home();
        input.delete();
        assert_eq!(input.value(), "bc");
    }

    #[test]
    fn max_length_is_enforced() {
        let mut input = TextInput::new("").with_max_length(2);
        for ch in "abcd".chars() {
            input.insert(ch);
        }
        assert_eq!(input.value(), "ab");
    }

    #[test]
    fn kill_to_start_clears_prefix() {
        let mut input = TextInput::new("hello");
        input.move_left();
        input.kill_to_start();
        assert_eq!(input.value(), "o");
        assert_eq!(input.cursor(), 0);
    }

    #[test]
    fn handles_multibyte_text() {
        let mut input = TextInput::new("héllo");
        input.move_home();
        input.move_right();
        input.delete();
        assert_eq!(input.value(), "hllo");
    }

    #[test]
    fn placeholder_shows_while_focused_and_empty() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let input = TextInput::new("").with_placeholder("Enter path...");
        for focused in [true, false] {
            let mut terminal = Terminal::new(TestBackend::new(40, 3)).expect("terminal");
            terminal
                .draw(|frame| input.render(frame, frame.area(), focused))
                .expect("draw");
            let screen: String = terminal
                .backend()
                .buffer()
                .content
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            assert!(
                screen.contains("Enter path..."),
                "placeholder missing when focused={focused}"
            );
        }

        // Once there is a value, the value wins.
        let typed = TextInput::new("/tmp").with_placeholder("Enter path...");
        let mut terminal = Terminal::new(TestBackend::new(40, 3)).expect("terminal");
        terminal
            .draw(|frame| typed.render(frame, frame.area(), true))
            .expect("draw");
        let screen: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(screen.contains("/tmp"));
        assert!(!screen.contains("Enter path..."));
    }

    #[test]
    fn centered_rect_clamps_to_area() {
        let area = Rect::new(0, 0, 40, 10);

        // Oversized: clamped, but with a margin so it still reads as a dialog
        // rather than running edge to edge.
        let r = centered_rect(80, 30, area);
        assert_eq!(r.width, 38);
        assert_eq!(r.height, 9);
        // Horizontal margin is what stops it reading as a full-width pane; a
        // one-row vertical gap rounds away and does not matter.
        assert!(r.x > 0, "no margin left around the dialog");

        // Fits: placed exactly, centred.
        let r = centered_rect(20, 4, area);
        assert_eq!((r.x, r.y, r.width, r.height), (10, 3, 20, 4));

        // Degenerate areas must still produce something drawable.
        let tiny = centered_rect(80, 30, Rect::new(0, 0, 1, 1));
        assert_eq!((tiny.width, tiny.height), (1, 1));
    }
}
