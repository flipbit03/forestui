//! Modal overlays: one centred box per open dialog.
//!
//! `src/modal.rs` owns the state and the keys; this file draws, and records
//! where every control landed so a click can be routed back to it. Controls are
//! laid out in focus-index order so the picture and the key handling cannot
//! drift apart — read a `handle_key` there and the matching block here should
//! name the same indices.

use crate::app::{App, HitTarget, ModalClick};
use crate::modal::{
    AddRepositoryModal, AddWorktreeModal, ConfirmModal, CreateFromIssueModal, CustomButtonsModal,
    EDITORS, EditButtonModal, Modal, SPINNER, SettingsModal, THEMES,
};
use crate::theme;
use crate::ui::widgets::{self, TextInput};
use crate::ui::{button, button_width};
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
/// Space between two controls sharing a row.
const GAP: &str = "  ";

/// Where the controls of one modal landed: a rectangle and the focus index a
/// click inside it selects. The indices are the ones `Modal::set_focus` and the
/// modal's `handle_key` use — anything else would focus one control and act on
/// another.
pub type Hits = Vec<(Rect, usize, ModalClick)>;

pub fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    // Only the top of the stack is drawn: Settings → Custom Buttons → Edit
    // Button nests, and a half-covered parent behind the child is just noise.
    // Only the top is clickable either, for the same reason.
    let hits = match app.modals.last() {
        Some(modal) => render_modal(frame, modal, area),
        None => return,
    };
    for (rect, index, click) in hits {
        app.push_hit(rect, HitTarget::ModalControl { index, click });
    }
}

/// Draw one modal and report where its controls landed. Split out from [`draw`]
/// so tests can render a modal without standing up a whole [`App`].
pub fn render_modal(frame: &mut Frame, modal: &Modal, area: Rect) -> Hits {
    let mut hits = Hits::new();
    match modal {
        Modal::AddRepository(m) => add_repository(frame, m, area, &mut hits),
        Modal::AddWorktree(m) => add_worktree(frame, m, area, &mut hits),
        Modal::CreateFromIssue(m) => create_from_issue(frame, m, area, &mut hits),
        Modal::Settings(m) => settings(frame, m, area, &mut hits),
        Modal::CustomButtons(m) => custom_buttons(frame, m, area, &mut hits),
        Modal::EditButton(m) => edit_button(frame, m, area, &mut hits),
        Modal::Confirm(m) => confirm(frame, m, area, &mut hits),
    }
    hits
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
        if let Some(rect) = self.take(1) {
            frame.render_widget(Paragraph::new(line), rect);
        }
    }

    /// A line whose whole row carries `style` — used for list selection, where
    /// the highlight has to run to the edge of the box. Both lists are pickable,
    /// so the whole row is the click target for `index`.
    fn row(
        &mut self,
        frame: &mut Frame,
        hits: &mut Hits,
        line: Line,
        style: Style,
        index: usize,
        row: usize,
    ) {
        if let Some(rect) = self.take(1) {
            frame.render_widget(Paragraph::new(line).style(style), rect);
            hits.push((rect, index, ModalClick::Row(row)));
        }
    }

    fn text(&mut self, frame: &mut Frame, text: impl Into<String>, style: Style) {
        self.line(frame, Line::from(Span::styled(text.into(), style)));
    }

    /// Text inputs draw their own border, so they are all-or-nothing.
    fn input(
        &mut self,
        frame: &mut Frame,
        hits: &mut Hits,
        input: &TextInput,
        focus: usize,
        index: usize,
    ) {
        if let Some(rect) = self.take(3) {
            input.render(frame, rect, focus == index);
            // Focus only: activating here would submit the whole dialog.
            hits.push((rect, index, ModalClick::Focus));
        }
    }

    /// Draw a row of controls left to right, recording where each one landed.
    fn controls(&mut self, frame: &mut Frame, hits: &mut Hits, controls: Vec<Control>) {
        let Some(rect) = self.take(1) else {
            return;
        };
        let right = rect.x + rect.width;
        let mut spans = Vec::new();
        let mut x = rect.x;
        for (control, width, index, click) in controls {
            // A control the box is too narrow to show must not be clickable.
            let visible = width.min(right.saturating_sub(x));
            if visible > 0 {
                hits.push((
                    Rect {
                        x,
                        y: rect.y,
                        width: visible,
                        height: 1,
                    },
                    index,
                    click,
                ));
            }
            spans.extend(control);
            spans.push(Span::raw(GAP));
            x = x.saturating_add(width).saturating_add(GAP.len() as u16);
        }
        frame.render_widget(Paragraph::new(Line::from(spans)), rect);
    }
}

// ------------------------------------------------------------------ Controls

/// One piece of a control row: what to draw, how wide it renders, the focus
/// index a click selects, and what that click does.
type Control = (Vec<Span<'static>>, u16, usize, ModalClick);

/// A button, drawn as the shared pill so it reads as one.
fn control(label: &str, focused: bool, destructive: bool, index: usize) -> Control {
    (
        button(label, focused, destructive),
        button_width(label),
        index,
        ModalClick::Activate,
    )
}

/// The greyed-out form of [`control`], for a button Textual would `disable`.
/// Built from the same pill so neither the layout nor the click region moves
/// when the button becomes enabled.
fn disabled(label: &str, index: usize) -> Control {
    let spans = button(label, false, false)
        .into_iter()
        .map(|span| span.style(theme::muted()))
        .collect();
    (spans, button_width(label), index, ModalClick::Activate)
}

fn checkbox(label: &str, checked: bool, focused: bool, index: usize) -> Control {
    let mark = if checked { 'x' } else { ' ' };
    control(&format!("[{mark}] {label}"), focused, false, index)
}

/// A value Left/Right cycles through — the stand-in for Textual's `Select`.
fn cycle(value: &str, focused: bool, index: usize) -> Control {
    let (spans, width, index, _) = control(&format!("◂ {value} ▸"), focused, false, index);
    (spans, width, index, ModalClick::Cycle)
}

/// A bare arrow beside a control, standing for the key that drives it. Clicking
/// one does what that key does, so it carries the control's index too.
fn arrow(glyph: char, focused: bool, index: usize) -> Control {
    let style = if focused {
        theme::accent()
    } else {
        theme::muted()
    };
    // The arrows stand for Left/Right, so clicking one cycles the value.
    (
        vec![Span::styled(glyph.to_string(), style)],
        1,
        index,
        ModalClick::Cycle,
    )
}

// ----------------------------------------------------------- Add repository

fn add_repository(frame: &mut Frame, modal: &AddRepositoryModal, area: Rect, hits: &mut Hits) {
    let rect = widgets::centered_rect(WIDTH, 11, area);
    let mut column = Column::new(widgets::framed(frame, rect, "Add Repository", true));

    column.line(frame, widgets::section("Repository Path"));
    column.input(frame, hits, &modal.path, modal.focus, 0);

    let (status, valid) = modal.status();
    let style = if !valid && !status.is_empty() {
        theme::destructive()
    } else {
        theme::secondary()
    };
    column.text(frame, status, style);
    column.gap();

    column.controls(
        frame,
        hits,
        vec![checkbox(
            "Import existing worktrees",
            modal.import_worktrees,
            modal.focus == 1,
            1,
        )],
    );
    column.gap();
    column.controls(
        frame,
        hits,
        vec![
            control("Add Repository", modal.focus == 2, false, 2),
            control("Cancel", modal.focus == 3, false, 3),
        ],
    );
}

// ------------------------------------------------------------- Add worktree

fn add_worktree(frame: &mut Frame, modal: &AddWorktreeModal, area: Rect, hits: &mut Hits) {
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
    column.input(frame, hits, &modal.name, modal.focus, 0);

    let preview = modal
        .path_preview()
        .map(|path| format!(" {}", path.display()))
        .unwrap_or_default();
    column.text(frame, preview, theme::muted());

    column.line(frame, widgets::section("Branch"));
    // Left/Right switch modes here, so the arrows are literal: they light up
    // only while the toggle holds the focus. The whole row is one control, so
    // every part of it toggles — which is what Enter on it does too.
    let toggle_focused = modal.focus == 1;
    column.controls(
        frame,
        hits,
        vec![
            arrow('◂', toggle_focused, 1),
            control("New Branch", modal.new_branch, false, 1),
            control("Existing", !modal.new_branch, false, 1),
            arrow('▸', toggle_focused, 1),
        ],
    );

    if modal.new_branch {
        column.input(frame, hits, &modal.branch, modal.focus, 2);
    } else {
        column.input(frame, hits, &modal.search, modal.focus, 2);
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
        // The list is one focus stop, index 3, so every row carries that index.
        let focused = modal.focus == 3;
        for (offset, (branch, _)) in matches.iter().skip(first).take(visible).enumerate() {
            let selected = first + offset == modal.search_index;
            let style = match (selected, focused) {
                (true, true) => theme::cursor(),
                (true, false) => theme::cursor_unfocused(),
                (false, _) => theme::primary(),
            };
            column.row(
                frame,
                hits,
                branch_line(branch, query, style),
                style,
                3,
                first + offset,
            );
        }
    }

    column.text(frame, modal.error.clone(), theme::destructive());
    column.gap();

    let create = modal.field_count() - 2;
    let create_control = if modal.can_create() {
        control("Create Worktree", modal.focus == create, false, create)
    } else {
        disabled("Create Worktree", create)
    };
    column.controls(
        frame,
        hits,
        vec![
            create_control,
            control("Cancel", modal.focus == create + 1, false, create + 1),
        ],
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

fn create_from_issue(frame: &mut Frame, modal: &CreateFromIssueModal, area: Rect, hits: &mut Hits) {
    let rect = widgets::centered_rect(WIDTH, 21, area);
    let title = format!("Create Worktree from Issue #{}", modal.issue_number);
    let mut column = Column::new(widgets::framed(frame, rect, &title, true));

    column.text(frame, modal.issue_title.clone(), theme::muted());

    column.line(frame, widgets::section("Worktree Name"));
    column.input(frame, hits, &modal.name, modal.focus, 0);
    column.text(
        frame,
        format!("Path: {}", modal.path_preview().display()),
        theme::muted(),
    );

    column.line(frame, widgets::section("Branch Name"));
    column.input(frame, hits, &modal.branch, modal.focus, 1);

    column.line(frame, widgets::section("Base Branch"));
    column.input(frame, hits, &modal.base_branch, modal.focus, 2);
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
    column.controls(
        frame,
        hits,
        vec![control(&fetch, modal.focus == 3, false, 3)],
    );
    column.controls(
        frame,
        hits,
        vec![checkbox(
            "Pull repo before creating",
            modal.pull_first,
            modal.focus == 4,
            4,
        )],
    );
    column.gap();

    let create = if modal.can_create() {
        control("Create", modal.focus == 5, false, 5)
    } else {
        disabled("Create", 5)
    };
    column.controls(
        frame,
        hits,
        vec![create, control("Cancel", modal.focus == 6, false, 6)],
    );
}

// ------------------------------------------------------------------ Settings

fn settings(frame: &mut Frame, modal: &SettingsModal, area: Rect, hits: &mut Hits) {
    let rect = widgets::centered_rect(WIDE, 15, area);
    let mut column = Column::new(widgets::framed(frame, rect, "Settings", true));

    let editor = EDITORS
        .get(modal.editor_index)
        .map_or("", |(name, _)| *name);
    column.line(frame, widgets::section("DEFAULT EDITOR"));
    column.controls(frame, hits, vec![cycle(editor, modal.focus == 0, 0)]);

    column.line(frame, widgets::section("BRANCH PREFIX"));
    column.input(frame, hits, &modal.branch_prefix, modal.focus, 1);

    let theme_name = THEMES.get(modal.theme_index).map_or("", |(name, _)| *name);
    column.line(frame, widgets::section("THEME"));
    column.controls(frame, hits, vec![cycle(theme_name, modal.focus == 2, 2)]);

    column.line(frame, widgets::section("CUSTOM CLAUDE BUTTONS"));
    column.text(frame, modal.buttons_summary(), theme::muted());
    column.controls(
        frame,
        hits,
        vec![control(
            "Manage Custom Buttons...",
            modal.focus == 3,
            false,
            3,
        )],
    );
    column.gap();
    column.controls(
        frame,
        hits,
        vec![
            control("Save", modal.focus == 4, false, 4),
            control("Cancel", modal.focus == 5, false, 5),
        ],
    );
}

// ------------------------------------------------------------ Custom buttons

fn custom_buttons(frame: &mut Frame, modal: &CustomButtonsModal, area: Rect, hits: &mut Hits) {
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
        for (offset, entry) in modal.buttons.iter().skip(first).take(visible).enumerate() {
            let index = first + offset;
            let selected = index == modal.selected;
            let style = if selected {
                theme::cursor()
            } else {
                theme::primary()
            };
            let marker = if selected { "▸" } else { " " };
            let yolo = if entry.is_yolo_style() { " (YOLO)" } else { "" };
            column.row(
                frame,
                hits,
                Line::from(Span::styled(
                    format!("{marker} {}{yolo}", entry.label),
                    style,
                )),
                style,
                index,
                index,
            );
            // The whole three-row block picks the entry, so its description
            // rows are click targets for the same index.
            column.row(
                frame,
                hits,
                Line::from(format!("   prefix: {}", entry.prefix)),
                theme::muted(),
                index,
                index,
            );
            column.row(
                frame,
                hits,
                Line::from(format!("   $ {}", entry.command)),
                theme::muted(),
                index,
                index,
            );
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

fn edit_button(frame: &mut Frame, modal: &EditButtonModal, area: Rect, hits: &mut Hits) {
    let rect = widgets::centered_rect(WIDTH, 20, area);
    let mut column = Column::new(widgets::framed(frame, rect, modal.title(), true));

    column.line(frame, widgets::section("LABEL"));
    column.input(frame, hits, &modal.label, modal.focus, 0);
    column.text(
        frame,
        "Shown on the button (e.g., 'New Session: YoloDisc')",
        theme::muted(),
    );

    column.line(frame, widgets::section("TMUX PREFIX"));
    column.input(frame, hits, &modal.prefix, modal.focus, 1);
    column.text(
        frame,
        "Window prefix: <prefix>:<worktree>. Auto-derived from label until you edit it.",
        theme::muted(),
    );

    column.line(frame, widgets::section("COMMAND"));
    column.input(frame, hits, &modal.command, modal.focus, 2);
    column.text(
        frame,
        "Run as-is. If it contains --dangerously-skip-permissions the button is styled red.",
        theme::muted(),
    );

    column.text(frame, modal.error.clone(), theme::destructive());
    column.gap();
    column.controls(
        frame,
        hits,
        vec![
            control("Save", modal.focus == 3, false, 3),
            control("Cancel", modal.focus == 4, false, 4),
        ],
    );
}

// ------------------------------------------------------------------ Confirm

fn confirm(frame: &mut Frame, modal: &ConfirmModal, area: Rect, hits: &mut Hits) {
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
    // `ConfirmModal::set_focus` reads 0 as Cancel and 1 as Delete.
    column.controls(
        frame,
        hits,
        vec![
            control("Cancel", !modal.confirm_focused, false, 0),
            control("Delete", modal.confirm_focused, true, 1),
        ],
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

    /// Render a modal into a fixed buffer, returning it as text (one row per
    /// line) alongside the clickable regions it recorded.
    fn render_with_hits(modal: &Modal, width: u16, height: u16) -> (String, Hits) {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        let mut hits = Hits::new();
        terminal
            .draw(|frame| hits = render_modal(frame, modal, frame.area()))
            .expect("draw");
        let buffer = terminal.backend().buffer().clone();
        let screen = (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .filter_map(|x| buffer.cell((x, y)).map(|cell| cell.symbol().to_string()))
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        (screen, hits)
    }

    fn render(modal: &Modal, width: u16, height: u16) -> String {
        render_with_hits(modal, width, height).0
    }

    /// The cell where `needle` starts on screen, as (column, row).
    fn cell_of(screen: &str, needle: &str) -> (u16, u16) {
        let (row, line) = screen
            .lines()
            .enumerate()
            .find(|(_, line)| line.contains(needle))
            .unwrap_or_else(|| panic!("{needle} missing from:\n{screen}"));
        let byte = line.find(needle).unwrap_or_default();
        // Columns are cells, not bytes: the pills and borders are multi-byte.
        let column = line[..byte].chars().count() as u16;
        (column, row as u16)
    }

    /// The focus index recorded for a cell, resolved the way `App::hit_at` does.
    fn index_at(hits: &Hits, (column, row): (u16, u16)) -> Option<usize> {
        hits.iter()
            .rev()
            .find(|(rect, _, _)| {
                column >= rect.x
                    && column < rect.x + rect.width
                    && row >= rect.y
                    && row < rect.y + rect.height
            })
            .map(|(_, index, _)| *index)
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

    #[test]
    fn clicking_a_modal_control_maps_to_its_focus_index() {
        // Every index below is the one documented on the modal in `src/modal.rs`;
        // a click that focused anything else would act on the wrong control.
        let settings = Modal::Settings(Box::new(SettingsModal::new(&Settings::default())));
        let (screen, hits) = render_with_hits(&settings, 160, 40);
        for (needle, index) in [
            ("▐ ◂ Vim (tmux) ▸ ▌", 0),
            ("feat/", 1),
            ("▐ ◂ System ▸ ▌", 2),
            ("▐ Manage Custom Buttons... ▌", 3),
            ("▐ Save ▌", 4),
            ("▐ Cancel ▌", 5),
        ] {
            assert_eq!(
                index_at(&hits, cell_of(&screen, needle)),
                Some(index),
                "{needle} in:\n{screen}"
            );
        }

        let add = Modal::AddRepository(AddRepositoryModal::new());
        let (screen, hits) = render_with_hits(&add, 100, 20);
        for (needle, index) in [
            ("Enter path or paste", 0),
            ("▐ [ ] Import existing worktrees ▌", 1),
            ("▐ Add Repository ▌", 2),
            ("▐ Cancel ▌", 3),
        ] {
            assert_eq!(
                index_at(&hits, cell_of(&screen, needle)),
                Some(index),
                "{needle} in:\n{screen}"
            );
        }
    }

    #[test]
    fn confirm_modal_records_cancel_and_delete() {
        let modal = Modal::Confirm(ConfirmModal::new(
            "Delete Worktree",
            "Remove wt-one?",
            ConfirmAction::DeleteWorktree(Uuid::new_v4()),
        ));
        let (screen, hits) = render_with_hits(&modal, 100, 20);
        // `Modal::set_focus` reads 0 as Cancel and 1 as the destructive choice.
        assert_eq!(
            index_at(&hits, cell_of(&screen, "▐ Cancel ▌")),
            Some(0),
            "{screen}"
        );
        assert_eq!(
            index_at(&hits, cell_of(&screen, "▐ Delete ▌")),
            Some(1),
            "{screen}"
        );
    }

    #[test]
    fn branch_rows_are_clickable() {
        let (screen, hits) = render_with_hits(&worktree_modal(false), 100, 40);
        // The dropdown is a single focus stop, so every visible row records a
        // region against index 3 — the results list.
        let rows = hits.iter().filter(|(_, index, _)| *index == 3).count();
        assert_eq!(rows, 3, "one region per visible branch:\n{screen}");
        assert_eq!(
            index_at(&hits, cell_of(&screen, "origin/main")),
            Some(3),
            "{screen}"
        );
    }
}
