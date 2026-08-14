//! Small building blocks the panes and modals share.

use crate::theme;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
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

/// Render a bordered box, returning the inner area to draw into.
pub fn framed(frame: &mut Frame, area: Rect, title: &str, focused: bool) -> Rect {
    let border_style = if focused {
        theme::border_focused()
    } else {
        theme::border()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(Span::styled(format!(" {title} "), theme::title()))
        .style(Style::default().bg(theme::BG));
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    inner
}

/// Centre a fixed-size rect inside `area`, clamped to the available space.
pub fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
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
        let r = centered_rect(80, 30, area);
        assert_eq!(r.width, 40);
        assert_eq!(r.height, 10);
        let r = centered_rect(20, 4, area);
        assert_eq!((r.x, r.y, r.width, r.height), (10, 3, 20, 4));
    }
}
