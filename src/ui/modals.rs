//! Modal overlays: one centred box per open dialog.
//!
//! `src/modal.rs` owns the state and the keys; this file only draws. Controls
//! are laid out in focus-index order so the picture and the key handling cannot
//! drift apart — read a `handle_key` there and the matching block here should
//! name the same indices.

use crate::app::App;
use crate::modal::{
    AddRepositoryModal, AddWorktreeModal, ConfirmModal, CreateFromIssueModal, CustomButtonsModal,
    EDITORS, EditButtonModal, Modal, SPINNER, SettingsModal, THEMES,
};
use crate::theme;
use crate::ui::widgets::{self, TextInput};
use crate::util;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

/// Width of a normal modal — Textual's `.modal-container { width: 80 }`.
const WIDTH: u16 = 80;
/// Width of the two wide modals — Textual's `.modal-wide { width: 140 }`.
const WIDE: u16 = 140;
/// Branch-dropdown rows kept on screen — Textual's `OptionList { max-height: 10 }`.
const DROPDOWN_ROWS: usize = 10;
/// Rows the custom-button list may occupy before it starts scrolling.
const BUTTON_LIST_ROWS: usize = 15;
/// Rows one custom-button block takes: label, prefix, command.
const BUTTON_BLOCK: usize = 3;

pub fn draw(frame: &mut Frame, app: &App, area: Rect) {
    // Only the top of the stack is drawn: Settings → Custom Buttons → Edit
    // Button nests, and a half-covered parent behind the child is just noise.
    if let Some(modal) = app.modals.last() {
        render_modal(frame, modal, area);
    }
}

/// Draw one modal. Split out from [`draw`] so tests can render a modal without
/// standing up a whole [`App`].
pub fn render_modal(frame: &mut Frame, modal: &Modal, area: Rect) {
    match modal {
        Modal::AddRepository(m) => add_repository(frame, m, area),
        Modal::AddWorktree(m) => add_worktree(frame, m, area),
        Modal::CreateFromIssue(m) => create_from_issue(frame, m, area),
        Modal::Settings(m) => settings(frame, m, area),
        Modal::CustomButtons(m) => custom_buttons(frame, m, area),
        Modal::EditButton(m) => edit_button(frame, m, area),
        Modal::Confirm(m) => confirm(frame, m, area),
    }
}

// ------------------------------------------------------------------- Layout

/// A top-down cursor over a modal's inner area.
///
/// Modals are a vertical stack of one- and three-row pieces, so a running `y`
/// beats a `Layout` with a constraint per element — and it puts the overflow
/// clamp in one place: once the box is full, the rest simply does not draw.
struct Column {
    area: Rect,
    y: u16,
}

impl Column {
    fn new(area: Rect) -> Self {
        Self { area, y: area.y }
    }

    fn remaining(&self) -> u16 {
        (self.area.y + self.area.height).saturating_sub(self.y)
    }

    /// Reserve `height` rows, or `None` once they no longer fit.
    fn take(&mut self, height: u16) -> Option<Rect> {
        if height == 0 || self.remaining() < height {
            return None;
        }
        let rect = Rect {
            x: self.area.x,
            y: self.y,
            width: self.area.width,
            height,
        };
        self.y += height;
        Some(rect)
    }

    fn gap(&mut self) {
        let _ = self.take(1);
    }

    fn line(&mut self, frame: &mut Frame, line: Line) {
        self.row(frame, line, Style::default());
    }

    /// A line whose whole row carries `style` — used for list selection, where
    /// the highlight has to run to the edge of the box.
    fn row(&mut self, frame: &mut Frame, line: Line, style: Style) {
        if let Some(rect) = self.take(1) {
            frame.render_widget(Paragraph::new(line).style(style), rect);
        }
    }

    fn text(&mut self, frame: &mut Frame, text: impl Into<String>, style: Style) {
        self.line(frame, Line::from(Span::styled(text.into(), style)));
    }

    /// Text inputs draw their own border, so they are all-or-nothing.
    fn input(&mut self, frame: &mut Frame, input: &TextInput, focused: bool) {
        if let Some(rect) = self.take(3) {
            input.render(frame, rect, focused);
        }
    }
}

// ------------------------------------------------------------------ Controls

/// A "button": a padded label the focus ring can land on.
fn control(label: &str, focused: bool, destructive: bool) -> Span<'static> {
    Span::styled(format!(" {label} "), theme::action(focused, destructive))
}

/// The greyed-out form of [`control`], for a button Textual would `disable`.
fn disabled(label: &str) -> Span<'static> {
    Span::styled(format!(" {label} "), theme::muted())
}

fn checkbox(label: &str, checked: bool, focused: bool) -> Span<'static> {
    let mark = if checked { 'x' } else { ' ' };
    Span::styled(format!(" [{mark}] {label} "), theme::action(focused, false))
}

/// A value Left/Right cycles through — the stand-in for Textual's `Select`.
fn cycle(value: &str, focused: bool) -> Span<'static> {
    Span::styled(format!(" ◂ {value} ▸ "), theme::action(focused, false))
}

// ----------------------------------------------------------- Add repository

fn add_repository(frame: &mut Frame, modal: &AddRepositoryModal, area: Rect) {
    let rect = widgets::centered_rect(WIDTH, 11, area);
    let mut column = Column::new(widgets::framed(frame, rect, "Add Repository", true));

    column.line(frame, widgets::section("Repository Path"));
    column.input(frame, &modal.path, modal.focus == 0);

    let (status, valid) = modal.status();
    let style = if !valid && !status.is_empty() {
        theme::destructive()
    } else {
        theme::secondary()
    };
    column.text(frame, status, style);
    column.gap();

    column.line(
        frame,
        Line::from(checkbox(
            "Import existing worktrees",
            modal.import_worktrees,
            modal.focus == 1,
        )),
    );
    column.gap();
    column.line(
        frame,
        Line::from(vec![
            control("Add Repository", modal.focus == 2, false),
            Span::raw("  "),
            control("Cancel", modal.focus == 3, false),
        ]),
    );
}

// ------------------------------------------------------------- Add worktree

fn add_worktree(frame: &mut Frame, modal: &AddWorktreeModal, area: Rect) {
    let matches = modal.matches();
    // The dropdown only exists in existing-branch mode: a count line plus rows.
    let dropdown = if modal.new_branch {
        0
    } else {
        matches.len().min(DROPDOWN_ROWS) + 1
    };
    let rect = widgets::centered_rect(WIDTH, 16 + dropdown as u16, area);
    let mut column = Column::new(widgets::framed(frame, rect, "Add Worktree", true));

    column.text(frame, format!("to {}", modal.repo_name), theme::secondary());
    column.line(frame, widgets::section("Worktree Name"));
    column.input(frame, &modal.name, modal.focus == 0);

    let preview = modal
        .path_preview()
        .map(|path| format!(" {}", path.display()))
        .unwrap_or_default();
    column.text(frame, preview, theme::muted());

    column.line(frame, widgets::section("Branch"));
    // Left/Right switch modes here, so the arrows are literal: they light up
    // only while the toggle holds the focus.
    let arrows = if modal.focus == 1 {
        theme::accent()
    } else {
        theme::muted()
    };
    column.line(
        frame,
        Line::from(vec![
            Span::styled("◂", arrows),
            control("New Branch", modal.new_branch, false),
            Span::raw(" "),
            control("Existing", !modal.new_branch, false),
            Span::styled("▸", arrows),
        ]),
    );

    if modal.new_branch {
        column.input(frame, &modal.branch, modal.focus == 2);
    } else {
        column.input(frame, &modal.search, modal.focus == 2);
        let query = modal.search.value();
        column.text(
            frame,
            match_count(query, matches.len(), modal.branches.len()),
            theme::muted(),
        );

        // Leave room for the error line and the buttons below the list.
        let space = column.remaining().saturating_sub(3) as usize;
        let visible = DROPDOWN_ROWS.min(space);
        let first = modal.search_index.saturating_sub(visible.saturating_sub(1));
        let focused = modal.focus == 3;
        for (offset, (branch, _)) in matches.iter().skip(first).take(visible).enumerate() {
            let selected = first + offset == modal.search_index;
            let style = match (selected, focused) {
                (true, true) => theme::cursor(),
                (true, false) => theme::cursor_unfocused(),
                (false, _) => theme::primary(),
            };
            column.row(frame, branch_line(branch, query, style), style);
        }
    }

    column.text(frame, modal.error.clone(), theme::destructive());
    column.gap();

    let create = modal.field_count() - 2;
    let create_control = if modal.can_create() {
        control("Create Worktree", modal.focus == create, false)
    } else {
        disabled("Create Worktree")
    };
    column.line(
        frame,
        Line::from(vec![
            create_control,
            Span::raw("  "),
            control("Cancel", modal.focus == create + 1, false),
        ]),
    );
}

/// The dropdown's count line, worded exactly as `branch_search.py` worded it.
fn match_count(query: &str, shown: usize, total: usize) -> String {
    if query.trim().is_empty() {
        if shown < total {
            format!("{shown} of {total} branches")
        } else {
            format!("{total} branches")
        }
    } else if shown == 0 {
        "No matches".to_string()
    } else if shown == 1 {
        "1 match".to_string()
    } else {
        format!("{shown} matches")
    }
}

/// One dropdown row with the literal query run highlighted, as Rich did.
fn branch_line(branch: &str, query: &str, style: Style) -> Line<'static> {
    let mut spans = vec![Span::styled(" ", style)];
    // The offsets come from a lowercased copy, so they can land mid-character
    // for exotic casing; fall back to a plain row rather than panicking.
    match util::highlight_range(query, branch)
        .filter(|(start, end)| branch.is_char_boundary(*start) && branch.is_char_boundary(*end))
    {
        Some((start, end)) => {
            spans.push(Span::styled(branch[..start].to_string(), style));
            spans.push(Span::styled(
                branch[start..end].to_string(),
                style.add_modifier(Modifier::BOLD | Modifier::REVERSED),
            ));
            spans.push(Span::styled(branch[end..].to_string(), style));
        }
        None => spans.push(Span::styled(branch.to_string(), style)),
    }
    Line::from(spans)
}

// -------------------------------------------------------- Create from issue

fn create_from_issue(frame: &mut Frame, modal: &CreateFromIssueModal, area: Rect) {
    let rect = widgets::centered_rect(WIDTH, 21, area);
    let title = format!("Create Worktree from Issue #{}", modal.issue_number);
    let mut column = Column::new(widgets::framed(frame, rect, &title, true));

    column.text(frame, modal.issue_title.clone(), theme::muted());

    column.line(frame, widgets::section("Worktree Name"));
    column.input(frame, &modal.name, modal.focus == 0);
    column.text(
        frame,
        format!("Path: {}", modal.path_preview().display()),
        theme::muted(),
    );

    column.line(frame, widgets::section("Branch Name"));
    column.input(frame, &modal.branch, modal.focus == 1);

    column.line(frame, widgets::section("Base Branch"));
    column.input(frame, &modal.base_branch, modal.focus == 2);
    // The completion the Textual suggester used to ghost inside the input.
    // Right accepts it, so say so; the row is always reserved to stop the
    // controls below jumping as the user types.
    let hint = match modal.base_suggestion() {
        Some(suggestion) if suggestion != modal.base_branch.value() => {
            format!(" {suggestion}  (→ accepts)")
        }
        _ => String::new(),
    };
    column.text(frame, hint, theme::muted());

    let fetch = if modal.is_fetching {
        SPINNER[modal.spinner_index % SPINNER.len()].to_string()
    } else {
        "Fetch".to_string()
    };
    column.line(frame, Line::from(control(&fetch, modal.focus == 3, false)));
    column.line(
        frame,
        Line::from(checkbox(
            "Pull repo before creating",
            modal.pull_first,
            modal.focus == 4,
        )),
    );
    column.gap();

    let create = if modal.can_create() {
        control("Create", modal.focus == 5, false)
    } else {
        disabled("Create")
    };
    column.line(
        frame,
        Line::from(vec![
            create,
            Span::raw("  "),
            control("Cancel", modal.focus == 6, false),
        ]),
    );
}

// ------------------------------------------------------------------ Settings

fn settings(frame: &mut Frame, modal: &SettingsModal, area: Rect) {
    let rect = widgets::centered_rect(WIDE, 15, area);
    let mut column = Column::new(widgets::framed(frame, rect, "Settings", true));

    let editor = EDITORS
        .get(modal.editor_index)
        .map_or("", |(name, _)| *name);
    column.line(frame, widgets::section("DEFAULT EDITOR"));
    column.line(frame, Line::from(cycle(editor, modal.focus == 0)));

    column.line(frame, widgets::section("BRANCH PREFIX"));
    column.input(frame, &modal.branch_prefix, modal.focus == 1);

    let theme_name = THEMES.get(modal.theme_index).map_or("", |(name, _)| *name);
    column.line(frame, widgets::section("THEME"));
    column.line(frame, Line::from(cycle(theme_name, modal.focus == 2)));

    column.line(frame, widgets::section("CUSTOM CLAUDE BUTTONS"));
    column.text(frame, modal.buttons_summary(), theme::muted());
    column.line(
        frame,
        Line::from(control("Manage Custom Buttons...", modal.focus == 3, false)),
    );
    column.gap();
    column.line(
        frame,
        Line::from(vec![
            control("Save", modal.focus == 4, false),
            Span::raw("  "),
            control("Cancel", modal.focus == 5, false),
        ]),
    );
}

// ------------------------------------------------------------ Custom buttons

fn custom_buttons(frame: &mut Frame, modal: &CustomButtonsModal, area: Rect) {
    let list_rows = if modal.buttons.is_empty() {
        1
    } else {
        (modal.buttons.len() * BUTTON_BLOCK).min(BUTTON_LIST_ROWS)
    };
    let rect = widgets::centered_rect(WIDE, list_rows as u16 + 6, area);
    let mut column = Column::new(widgets::framed(frame, rect, "Custom Claude Buttons", true));

    column.text(
        frame,
        "Order here matches display order in the Claude section.",
        theme::muted(),
    );
    column.gap();

    if modal.buttons.is_empty() {
        column.text(frame, "No buttons yet. Press a to add.", theme::muted());
    } else {
        // Leave the gap and the help footer their rows before filling the list.
        let space = column.remaining().saturating_sub(2) as usize;
        let visible = (space / BUTTON_BLOCK).max(1);
        let first = modal.selected.saturating_sub(visible.saturating_sub(1));
        for (offset, button) in modal.buttons.iter().skip(first).take(visible).enumerate() {
            let selected = first + offset == modal.selected;
            let style = if selected {
                theme::cursor()
            } else {
                theme::primary()
            };
            let marker = if selected { "▸" } else { " " };
            let yolo = if button.is_yolo_style() {
                " (YOLO)"
            } else {
                ""
            };
            column.row(
                frame,
                Line::from(Span::styled(
                    format!("{marker} {}{yolo}", button.label),
                    style,
                )),
                style,
            );
            column.text(
                frame,
                format!("   prefix: {}", button.prefix),
                theme::muted(),
            );
            column.text(frame, format!("   $ {}", button.command), theme::muted());
        }
    }

    column.gap();
    column.text(
        frame,
        "a add · e/Enter edit · d delete · K/J move · s save · Esc cancel",
        theme::muted(),
    );
}

// -------------------------------------------------------------- Edit button

fn edit_button(frame: &mut Frame, modal: &EditButtonModal, area: Rect) {
    let rect = widgets::centered_rect(WIDTH, 20, area);
    let mut column = Column::new(widgets::framed(frame, rect, modal.title(), true));

    column.line(frame, widgets::section("LABEL"));
    column.input(frame, &modal.label, modal.focus == 0);
    column.text(
        frame,
        "Shown on the button (e.g., 'New Session: YoloDisc')",
        theme::muted(),
    );

    column.line(frame, widgets::section("TMUX PREFIX"));
    column.input(frame, &modal.prefix, modal.focus == 1);
    column.text(
        frame,
        "Window prefix: <prefix>:<worktree>. Auto-derived from label until you edit it.",
        theme::muted(),
    );

    column.line(frame, widgets::section("COMMAND"));
    column.input(frame, &modal.command, modal.focus == 2);
    column.text(
        frame,
        "Run as-is. If it contains --dangerously-skip-permissions the button is styled red.",
        theme::muted(),
    );

    column.text(frame, modal.error.clone(), theme::destructive());
    column.gap();
    column.line(
        frame,
        Line::from(vec![
            control("Save", modal.focus == 3, false),
            Span::raw("  "),
            control("Cancel", modal.focus == 4, false),
        ]),
    );
}

// ------------------------------------------------------------------ Confirm

fn confirm(frame: &mut Frame, modal: &ConfirmModal, area: Rect) {
    let message: Vec<&str> = modal.message.split('\n').collect();
    let rect = widgets::centered_rect(WIDTH, message.len() as u16 + 8, area);
    // The title belongs in the body, not the border: `framed` paints its title
    // with `theme::title()`, and this one has to read as destructive.
    let mut column = Column::new(widgets::framed(frame, rect, "", true));

    column.text(frame, modal.title.clone(), theme::destructive());
    column.gap();
    for line in message {
        column.text(frame, line.to_string(), theme::secondary());
    }
    column.gap();
    column.line(
        frame,
        Line::from(vec![
            control("Cancel", !modal.confirm_focused, false),
            Span::raw("  "),
            control("Delete", modal.confirm_focused, true),
        ]),
    );
    column.gap();
    column.text(frame, "y confirm · n / Esc cancel", theme::muted());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modal::{ConfirmAction, CustomButtonsModal};
    use crate::models::{CustomClaudeButton, GitHubIssue, Repository, Settings};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::path::PathBuf;
    use uuid::Uuid;

    /// Render a modal into a fixed buffer and return it as text, one row per line.
    fn render(modal: &Modal, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal
            .draw(|frame| render_modal(frame, modal, frame.area()))
            .expect("draw");
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .filter_map(|x| buffer.cell((x, y)).map(|cell| cell.symbol().to_string()))
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn repo() -> Repository {
        Repository::new("demo".into(), "/tmp/demo".into())
    }

    fn worktree_modal(new_branch: bool) -> Modal {
        let mut modal = AddWorktreeModal::new(
            &repo(),
            vec!["main".into(), "feat/login".into(), "origin/main".into()],
            vec!["origin".into()],
            PathBuf::from("/forest"),
            "feat/".into(),
        );
        modal.new_branch = new_branch;
        Modal::AddWorktree(Box::new(modal))
    }

    fn issue() -> GitHubIssue {
        serde_json::from_str(
            r#"{"number":42,"title":"Fix login bug","state":"OPEN","url":"u",
                "createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z",
                "author":{"login":"me"},"assignees":[],"labels":[]}"#,
        )
        .expect("issue fixture")
    }

    fn button(label: &str) -> CustomClaudeButton {
        CustomClaudeButton {
            label: label.into(),
            prefix: label.to_lowercase(),
            command: "claude --dangerously-skip-permissions".into(),
        }
    }

    fn every_modal() -> Vec<(Modal, &'static str)> {
        vec![
            (
                Modal::AddRepository(AddRepositoryModal::new()),
                "Add Repository",
            ),
            (worktree_modal(true), "Add Worktree"),
            (
                Modal::CreateFromIssue(Box::new(CreateFromIssueModal::new(
                    &repo(),
                    &issue(),
                    vec!["main".into()],
                    vec![],
                    PathBuf::from("/forest"),
                    "feat/",
                    "main",
                ))),
                "Create Worktree from Issue #42",
            ),
            (
                Modal::Settings(Box::new(SettingsModal::new(&Settings::default()))),
                "Settings",
            ),
            (
                Modal::CustomButtons(CustomButtonsModal::new(vec![button("Opus")])),
                "Custom Claude Buttons",
            ),
            (
                Modal::EditButton(Box::new(EditButtonModal::new(None, &[], None))),
                "Add Button",
            ),
            (
                Modal::Confirm(ConfirmModal::new(
                    "Delete Worktree",
                    "This cannot be undone.",
                    ConfirmAction::DeleteWorktree(Uuid::new_v4()),
                )),
                "Delete Worktree",
            ),
        ]
    }

    #[test]
    fn every_modal_renders_its_title() {
        for (modal, title) in every_modal() {
            let screen = render(&modal, 160, 40);
            assert!(screen.contains(title), "missing {title} in:\n{screen}");
        }
    }

    #[test]
    fn add_repository_shows_the_validation_message() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut modal = AddRepositoryModal::new();
        modal
            .path
            .set_value(dir.path().to_string_lossy().to_string());
        let screen = render(&Modal::AddRepository(modal), 100, 20);
        assert!(screen.contains("Not a git repository"), "{screen}");
        assert!(screen.contains("[ ] Import existing worktrees"), "{screen}");
    }

    #[test]
    fn add_worktree_dropdown_lists_matching_branches() {
        let Modal::AddWorktree(mut modal) = worktree_modal(false) else {
            panic!("wrong modal");
        };
        modal.search.set_value("main");
        modal.focus = 3;
        let screen = render(&Modal::AddWorktree(modal), 100, 40);
        // Two of the three branches match "main", so the count line says so.
        assert!(screen.contains("2 matches"), "{screen}");
        assert!(screen.contains("origin/main"), "{screen}");
        assert!(screen.contains("Existing"), "{screen}");
    }

    #[test]
    fn add_worktree_counts_every_branch_with_an_empty_query() {
        let screen = render(&worktree_modal(false), 100, 40);
        assert!(screen.contains("3 branches"), "{screen}");
    }

    #[test]
    fn confirm_shows_the_message_and_both_choices() {
        let modal = Modal::Confirm(ConfirmModal::new(
            "Delete Worktree",
            "Remove wt-one?\nThis cannot be undone.",
            ConfirmAction::DeleteWorktree(Uuid::new_v4()),
        ));
        let screen = render(&modal, 100, 20);
        assert!(screen.contains("Remove wt-one?"), "{screen}");
        assert!(screen.contains("This cannot be undone."), "{screen}");
        assert!(screen.contains("Cancel"), "{screen}");
        assert!(screen.contains("Delete"), "{screen}");
        assert!(screen.contains("y confirm"), "{screen}");
    }

    #[test]
    fn custom_buttons_scroll_to_keep_the_selection_visible() {
        let mut modal = CustomButtonsModal::new(vec![
            button("Alpha"),
            button("Bravo"),
            button("Charlie"),
            button("Delta"),
        ]);
        modal.selected = 3;
        let screen = render(&Modal::CustomButtons(modal), 100, 14);
        assert!(screen.contains("Delta"), "{screen}");
        assert!(screen.contains("(YOLO)"), "{screen}");
        assert!(screen.contains("a add"), "{screen}");
    }

    #[test]
    fn a_tiny_area_clamps_instead_of_panicking() {
        for (modal, _) in every_modal() {
            render(&modal, 20, 6);
            render(&modal, 4, 2);
        }
    }
}
