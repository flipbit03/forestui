//! Modal overlays: one centred box per open dialog.
//!
//! `src/modal.rs` owns the state and the keys; this file draws, and records
//! where every control landed so a click can be routed back to it. Controls are
//! laid out in focus-index order so the picture and the key handling cannot
//! drift apart — both sides name the same `FOCUS_*` constants on the modal
//! struct rather than repeating a number, so there is one place to change.

use crate::app::{App, Direction, HitTarget, ModalClick};
use crate::modal::{
    AddRepositoryModal, AddWorktreeModal, ConfirmModal, CreateFromIssueModal, CustomButtonsModal,
    EDITORS, EditButtonModal, Modal, SPINNER, SettingsModal, ThemePickerModal,
};
use crate::theme;
use crate::ui::widgets::{self, BUTTON_HEIGHT, TextInput};
use crate::util;
use ratatui::Frame;
use ratatui::layout::{Margin, Rect};
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
/// Space between two controls sharing a row. Textual gave every button
/// `margin: 0 1` and then collapsed the pair, so neighbours sit one cell apart.
const GAP: u16 = 1;
/// Rows the box spends on chrome before any content: two borders, the two
/// `padding: 1 2` rows, the title, and `.modal-title { margin-bottom: 1 }`.
const CHROME: u16 = 6;

/// Where the controls of one modal landed: a rectangle and the focus index a
/// click inside it selects. The indices are the modal's own `FOCUS_*` constants,
/// which `Modal::set_focus` and `handle_key` also use — anything else would
/// focus one control and act on another.
pub type Hits = Vec<(Rect, usize, ModalClick)>;

pub fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    // Only the top of the stack is drawn: Settings → Custom Buttons → Edit
    // Button nests, and a half-covered parent behind the child is just noise.
    // Only the top is clickable either, for the same reason.
    let hovered = match app.hovered {
        Some(HitTarget::ModalControl { index, .. }) => Some(index),
        _ => None,
    };
    let hits = match app.modals.last() {
        Some(modal) => HOVERED.with(|cell| {
            cell.set(hovered);
            let hits = render_modal(frame, modal, area);
            cell.set(None);
            hits
        }),
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
        Modal::ThemePicker(m) => theme_picker(frame, m, area, &mut hits),
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
    /// `centre` groups them in the middle of the box, the way Textual's
    /// `.modal-buttons { align: center middle }` did with the dialog's buttons.
    fn controls(
        &mut self,
        frame: &mut Frame,
        hits: &mut Hits,
        controls: Vec<Control>,
        centre: bool,
    ) {
        // Boxing the buttons made every dialog several rows taller, and a
        // dialog is capped at the height of the terminal. Rather than let the
        // button row fall off the bottom — leaving a dialog that can only be
        // answered from the keyboard, with nothing on screen saying so — pin it
        // to the last three rows. The body above it loses those rows instead,
        // which is at least visible.
        let rect = match self.take(BUTTON_HEIGHT) {
            Some(rect) => rect,
            None => {
                let y = (self.area.y + self.area.height).saturating_sub(BUTTON_HEIGHT);
                self.y = self.area.y + self.area.height;
                let pinned = Rect {
                    x: self.area.x,
                    y,
                    width: self.area.width,
                    height: BUTTON_HEIGHT.min(self.area.height),
                };
                // Every row that overflows lands on this same strip, so an
                // earlier one is painted over by whatever comes after it.
                // Its hit regions have to go with it: a control nobody can see
                // must not answer a click, and the blank margin either side of
                // a centred button row is exactly where those stale regions
                // would still be sitting.
                hits.retain(|(recorded, _, _)| !recorded.intersects(pinned));
                pinned
            }
        };
        let total = controls
            .iter()
            .map(|(_, width, _, _)| width.saturating_add(GAP))
            .sum::<u16>()
            .saturating_sub(GAP);
        let indent = if centre {
            rect.width.saturating_sub(total) / 2
        } else {
            0
        };
        let right = rect.x + rect.width;
        let mut rows: [Vec<Span<'static>>; 3] = [Vec::new(), Vec::new(), Vec::new()];
        let mut x = rect.x.saturating_add(indent);
        for row in &mut rows {
            row.push(Span::raw(" ".repeat(indent as usize)));
        }
        for (control, width, index, click) in controls {
            // A control the box is too narrow to show must not be clickable.
            let visible = width.min(right.saturating_sub(x));
            if visible > 0 {
                hits.push((
                    Rect {
                        x,
                        y: rect.y,
                        width: visible,
                        height: BUTTON_HEIGHT,
                    },
                    index,
                    click,
                ));
            }
            for (row, spans) in rows.iter_mut().zip(control) {
                row.extend(spans);
                row.push(Span::raw(" ".repeat(GAP as usize)));
            }
            x = x.saturating_add(width).saturating_add(GAP);
        }
        frame.render_widget(Paragraph::new(rows.map(Line::from).to_vec()), rect);
    }
}

/// Draw the dialog box and return a column over its content.
///
/// Textual gave `.modal-container` `padding: 1 2` and put the title in a label
/// on the first content row — never in the border — followed by the blank row
/// `.modal-title { margin-bottom: 1 }` left behind. The leading space is the
/// one the Python labels carried literally (`Label(" Add Repository")`).
fn dialog(frame: &mut Frame, rect: Rect, title: &str, style: Style) -> Column {
    // `.modal-container { background: $bg-elevated; border: solid $border }` —
    // the box is raised off the page and its border is the resting colour, not
    // the focus accent. A dialog is always the focused thing, so an accent
    // border says nothing and competes with the button that does have one.
    let inner = widgets::framed(frame, rect, "").inner(Margin::new(2, 1));
    let mut column = Column::new(inner);
    column.text(frame, format!(" {title}"), style);
    column.gap();
    column
}

// ------------------------------------------------------------------ Controls

/// One piece of a control row: its three rows of spans, how wide it renders,
/// the focus index a click selects, and what that click does. Every piece is
/// the same height so a row of them lines up.
type Control = ([Vec<Span<'static>>; 3], u16, usize, ModalClick);

/// A button, drawn as the bordered box Textual drew.
fn control(label: &str, focused: bool, variant: theme::Variant, index: usize) -> Control {
    boxed(label, focused, variant, true, index)
}

/// The greyed-out form of [`control`], for a button Textual would `disable`.
/// Built from the same box so neither the layout nor the click region moves
/// when the button becomes enabled.
fn disabled(label: &str, index: usize) -> Control {
    boxed(label, false, theme::Variant::Normal, false, index)
}

thread_local! {
    /// Focus index the pointer is over, for the duration of one `draw`.
    ///
    /// Every control already resolves its own `focused` from its index, and
    /// hover is the same shape. Carrying it here rather than through all
    /// nineteen call sites keeps them readable; it is set and cleared inside a
    /// single synchronous `draw`, so nothing observes it between frames.
    static HOVERED: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

/// The shared body of [`control`] and [`disabled`]: the box, styled the way
/// `ui/detail.rs` styles its controls, so both panes read as one app.
fn boxed(
    label: &str,
    focused: bool,
    variant: theme::Variant,
    enabled: bool,
    index: usize,
) -> Control {
    // A disabled control must not light up under the pointer: an affordance
    // promising a click it will not honour is worse than none.
    let hovered = enabled && HOVERED.with(|cell| cell.get()) == Some(index);
    let fill = theme::action_bg(variant, hovered);
    let border = theme::action_border(focused, variant, hovered);
    // A control that cannot run keeps its box, so neither the layout nor the
    // hit region shifts; only the label goes grey — except under the cursor,
    // which has to stay legible.
    let text = if enabled || focused {
        theme::action(focused, variant, hovered)
    } else {
        theme::muted().bg(fill)
    };
    (
        widgets::button_box(label, border.bg(fill), text),
        widgets::button_box_width(label),
        index,
        ModalClick::Activate,
    )
}

fn checkbox(label: &str, checked: bool, focused: bool, index: usize) -> Control {
    let mark = if checked { 'x' } else { ' ' };
    control(
        &format!("[{mark}] {label}"),
        focused,
        theme::Variant::Normal,
        index,
    )
}

/// A value Left/Right cycles through — the stand-in for Textual's `Select`.
fn cycle(value: &str, focused: bool, index: usize) -> Control {
    let (spans, width, index, _) = control(
        &format!("◂ {value} ▸"),
        focused,
        theme::Variant::Normal,
        index,
    );
    (spans, width, index, ModalClick::Cycle(Direction::Next))
}

/// One segment of a two-option row, which selects itself rather than toggling
/// the row. `direction` is the key that lands on this option: the pair reads as
/// Left for the first and Right for the second, both idempotent.
fn option(label: &str, active: bool, index: usize, direction: Direction) -> Control {
    let (spans, width, index, _) = control(label, active, theme::Variant::Normal, index);
    (spans, width, index, ModalClick::Cycle(direction))
}

/// A bare arrow beside a control, standing for the key that drives it. Clicking
/// one does what that key does, so it carries the control's index too. It is one
/// cell wide on all three rows, so the buttons after it stay aligned.
fn arrow(glyph: char, direction: Direction, focused: bool, index: usize) -> Control {
    let style = if focused {
        theme::accent()
    } else {
        theme::muted()
    };
    // The arrows stand for Left/Right, so clicking one cycles the value.
    (
        [
            vec![Span::raw(" ")],
            vec![Span::styled(glyph.to_string(), style)],
            vec![Span::raw(" ")],
        ],
        1,
        index,
        ModalClick::Cycle(direction),
    )
}

// ----------------------------------------------------------- Add repository

fn add_repository(frame: &mut Frame, modal: &AddRepositoryModal, area: Rect, hits: &mut Hits) {
    // Label, input, status, gap, checkbox, gap, buttons.
    let rect = widgets::centered_rect(WIDTH, CHROME + 13, area);
    let mut column = dialog(frame, rect, "Add Repository", theme::title());

    column.line(frame, widgets::section("Repository Path"));
    column.input(
        frame,
        hits,
        &modal.path,
        modal.focus,
        AddRepositoryModal::FOCUS_PATH,
    );

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
            modal.focus == AddRepositoryModal::FOCUS_IMPORT,
            AddRepositoryModal::FOCUS_IMPORT,
        )],
        false,
    );
    column.gap();
    column.controls(
        frame,
        hits,
        vec![
            control(
                "Add Repository",
                modal.focus == AddRepositoryModal::FOCUS_ADD,
                theme::Variant::Primary,
                AddRepositoryModal::FOCUS_ADD,
            ),
            control(
                "Cancel",
                modal.focus == AddRepositoryModal::FOCUS_CANCEL,
                theme::Variant::Normal,
                AddRepositoryModal::FOCUS_CANCEL,
            ),
        ],
        true,
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
    // Repo, label, input, preview, label, toggle, input, error, gap, buttons.
    let rect = widgets::centered_rect(WIDTH, CHROME + 18 + dropdown as u16, area);
    let mut column = dialog(frame, rect, "Add Worktree", theme::title());

    column.text(frame, format!("to {}", modal.repo_name), theme::secondary());
    column.line(frame, widgets::section("Worktree Name"));
    column.input(
        frame,
        hits,
        &modal.name,
        modal.focus,
        AddWorktreeModal::FOCUS_NAME,
    );

    let preview = modal
        .path_preview()
        .map(|path| format!(" {}", path.display()))
        .unwrap_or_default();
    column.text(frame, preview, theme::muted());

    column.line(frame, widgets::section("Branch"));
    // Left/Right switch modes here, so the arrows are literal: they light up
    // only while the toggle holds the focus. Each option *selects* itself
    // rather than toggling the row — Left is "New Branch" and Right is
    // "Existing" whichever is current, so clicking the option that is already
    // active does nothing instead of switching away from it.
    let mode = AddWorktreeModal::FOCUS_MODE;
    let toggle_focused = modal.focus == mode;
    column.controls(
        frame,
        hits,
        vec![
            arrow('◂', Direction::Previous, toggle_focused, mode),
            option("New Branch", modal.new_branch, mode, Direction::Previous),
            option("Existing", !modal.new_branch, mode, Direction::Next),
            arrow('▸', Direction::Next, toggle_focused, mode),
        ],
        false,
    );

    // One input slot either way; only which TextInput fills it depends on
    // the mode.
    let branch_input = if modal.new_branch {
        &modal.branch
    } else {
        &modal.search
    };
    column.input(
        frame,
        hits,
        branch_input,
        modal.focus,
        AddWorktreeModal::FOCUS_BRANCH,
    );
    if !modal.new_branch {
        let query = modal.search.value();
        column.text(
            frame,
            match_count(query, matches.len(), modal.branches.len()),
            theme::muted(),
        );

        // Leave room for the error line, the gap and the buttons below the list.
        let space = column.remaining().saturating_sub(2 + BUTTON_HEIGHT) as usize;
        let visible = DROPDOWN_ROWS.min(space);
        let first = modal.search_index.saturating_sub(visible.saturating_sub(1));
        // The list is one focus stop, so every row carries the same index.
        let results = AddWorktreeModal::FOCUS_RESULTS;
        let focused = modal.focus == results;
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
                results,
                first + offset,
            );
        }
    }

    column.text(frame, modal.error.clone(), theme::destructive());
    column.gap();

    // Create and Cancel close the ring, so their indices follow the mode.
    let create = modal.create_index();
    let cancel = modal.cancel_index();
    let create_control = if modal.can_create() {
        control(
            "Create Worktree",
            modal.focus == create,
            theme::Variant::Primary,
            create,
        )
    } else {
        disabled("Create Worktree", create)
    };
    column.controls(
        frame,
        hits,
        vec![
            create_control,
            control(
                "Cancel",
                modal.focus == cancel,
                theme::Variant::Normal,
                cancel,
            ),
        ],
        true,
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
    // Issue, three labelled fields with a hint each, fetch, checkbox, gap, buttons.
    let rect = widgets::centered_rect(WIDTH, CHROME + 25, area);
    let title = format!("Create Worktree from Issue #{}", modal.issue_number);
    let mut column = dialog(frame, rect, &title, theme::title());

    column.text(frame, modal.issue_title.clone(), theme::muted());

    column.line(frame, widgets::section("Worktree Name"));
    column.input(
        frame,
        hits,
        &modal.name,
        modal.focus,
        CreateFromIssueModal::FOCUS_NAME,
    );
    column.text(
        frame,
        format!("Path: {}", modal.path_preview().display()),
        theme::muted(),
    );

    column.line(frame, widgets::section("Branch Name"));
    column.input(
        frame,
        hits,
        &modal.branch,
        modal.focus,
        CreateFromIssueModal::FOCUS_BRANCH,
    );

    column.line(frame, widgets::section("Base Branch"));
    column.input(
        frame,
        hits,
        &modal.base_branch,
        modal.focus,
        CreateFromIssueModal::FOCUS_BASE,
    );
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
        vec![control(
            &fetch,
            modal.focus == CreateFromIssueModal::FOCUS_FETCH,
            theme::Variant::Normal,
            CreateFromIssueModal::FOCUS_FETCH,
        )],
        false,
    );
    column.controls(
        frame,
        hits,
        vec![checkbox(
            "Pull repo before creating",
            modal.pull_first,
            modal.focus == CreateFromIssueModal::FOCUS_PULL,
            CreateFromIssueModal::FOCUS_PULL,
        )],
        false,
    );
    column.gap();

    let create = if modal.can_create() {
        control(
            "Create",
            modal.focus == CreateFromIssueModal::FOCUS_CREATE,
            theme::Variant::Primary,
            CreateFromIssueModal::FOCUS_CREATE,
        )
    } else {
        disabled("Create", CreateFromIssueModal::FOCUS_CREATE)
    };
    column.controls(
        frame,
        hits,
        vec![
            create,
            control(
                "Cancel",
                modal.focus == CreateFromIssueModal::FOCUS_CANCEL,
                theme::Variant::Normal,
                CreateFromIssueModal::FOCUS_CANCEL,
            ),
        ],
        true,
    );
}

// ------------------------------------------------------------------ Settings

fn settings(frame: &mut Frame, modal: &SettingsModal, area: Rect, hits: &mut Hits) {
    // Three labelled controls, then the buttons section, the gap and the buttons.
    let rect = widgets::centered_rect(WIDE, CHROME + 21, area);
    let mut column = dialog(frame, rect, "Settings", theme::title());

    let editor = EDITORS
        .get(modal.editor_index)
        .map_or("", |(name, _)| *name);
    column.line(frame, widgets::section("DEFAULT EDITOR"));
    column.controls(
        frame,
        hits,
        vec![cycle(
            editor,
            modal.focus == SettingsModal::FOCUS_EDITOR,
            SettingsModal::FOCUS_EDITOR,
        )],
        false,
    );

    column.line(frame, widgets::section("BRANCH PREFIX"));
    column.input(
        frame,
        hits,
        &modal.branch_prefix,
        modal.focus,
        SettingsModal::FOCUS_PREFIX,
    );

    let theme_name = crate::theme::by_slug(modal.theme_slug).map_or("", |t| t.name);
    column.line(frame, widgets::section("THEME"));
    column.controls(
        frame,
        hits,
        vec![control(
            &format!("{theme_name}…"),
            modal.focus == SettingsModal::FOCUS_THEME,
            theme::Variant::Normal,
            SettingsModal::FOCUS_THEME,
        )],
        false,
    );

    column.line(frame, widgets::section("CUSTOM CLAUDE BUTTONS"));
    column.text(frame, modal.buttons_summary(), theme::muted());
    column.controls(
        frame,
        hits,
        vec![control(
            "Manage Custom Buttons...",
            modal.focus == SettingsModal::FOCUS_MANAGE,
            theme::Variant::Normal,
            SettingsModal::FOCUS_MANAGE,
        )],
        false,
    );
    column.gap();
    column.controls(
        frame,
        hits,
        vec![
            control(
                "Save",
                modal.focus == SettingsModal::FOCUS_SAVE,
                theme::Variant::Primary,
                SettingsModal::FOCUS_SAVE,
            ),
            control(
                "Cancel",
                modal.focus == SettingsModal::FOCUS_CANCEL,
                theme::Variant::Normal,
                SettingsModal::FOCUS_CANCEL,
            ),
        ],
        true,
    );
}

// ------------------------------------------------------------ Custom buttons

fn custom_buttons(frame: &mut Frame, modal: &CustomButtonsModal, area: Rect, hits: &mut Hits) {
    let list_rows = if modal.buttons.is_empty() {
        1
    } else {
        (modal.buttons.len() * BUTTON_BLOCK).min(BUTTON_LIST_ROWS)
    };
    // Intro, gap, the list, gap, help footer.
    let rect = widgets::centered_rect(WIDE, CHROME + list_rows as u16 + 4, area);
    let mut column = dialog(frame, rect, "Custom Claude Buttons", theme::title());

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
    // Three labelled fields with a hint each, the error line, gap, buttons.
    let rect = widgets::centered_rect(WIDTH, CHROME + 20, area);
    let mut column = dialog(frame, rect, modal.title(), theme::title());

    column.line(frame, widgets::section("LABEL"));
    column.input(
        frame,
        hits,
        &modal.label,
        modal.focus,
        EditButtonModal::FOCUS_LABEL,
    );
    column.text(
        frame,
        "Shown on the button (e.g., 'New Session: YoloDisc')",
        theme::muted(),
    );

    column.line(frame, widgets::section("TMUX PREFIX"));
    column.input(
        frame,
        hits,
        &modal.prefix,
        modal.focus,
        EditButtonModal::FOCUS_PREFIX,
    );
    column.text(
        frame,
        "Window prefix: <prefix>:<worktree>. Auto-derived from label until you edit it.",
        theme::muted(),
    );

    column.line(frame, widgets::section("COMMAND"));
    column.input(
        frame,
        hits,
        &modal.command,
        modal.focus,
        EditButtonModal::FOCUS_COMMAND,
    );
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
            control(
                "Save",
                modal.focus == EditButtonModal::FOCUS_SAVE,
                theme::Variant::Primary,
                EditButtonModal::FOCUS_SAVE,
            ),
            control(
                "Cancel",
                modal.focus == EditButtonModal::FOCUS_CANCEL,
                theme::Variant::Normal,
                EditButtonModal::FOCUS_CANCEL,
            ),
        ],
        true,
    );
}

// -------------------------------------------------------------- Theme picker

/// Theme rows kept on screen at once; the list scrolls around the highlight.
const THEME_ROWS: usize = 14;

fn theme_picker(frame: &mut Frame, modal: &ThemePickerModal, area: Rect, hits: &mut Hits) {
    let themes = crate::theme::THEMES;
    // Deliberately narrow: the app behind the dialog is the preview, so the
    // dialog covers as little of it as a readable list allows.
    let rect = widgets::centered_rect(44, CHROME + THEME_ROWS.min(themes.len()) as u16 + 2, area);
    let mut column = dialog(frame, rect, "Theme", theme::title());

    // The window is sized from the rows that actually fit — `centered_rect`
    // clamps the dialog on a short terminal, and a window computed from the
    // ideal height would scroll the highlighted row right out of the box.
    let rows = (column.remaining().saturating_sub(2) as usize)
        .min(THEME_ROWS)
        .min(themes.len())
        .max(1);

    // Keep the highlight centred while the window stays inside the list.
    let first = modal
        .index
        .saturating_sub(rows / 2)
        .min(themes.len().saturating_sub(rows));

    for (row, candidate) in themes.iter().enumerate().skip(first).take(rows) {
        let Some(line_rect) = column.take(1) else {
            break;
        };
        let selected = row == modal.index;
        let row_style = if selected {
            theme::cursor()
        } else {
            theme::secondary()
        };
        // The swatch shows each candidate's accent without applying it, so the
        // list itself hints at what a row would look like before it is chosen.
        let swatch_style = row_style.fg(candidate.accent);
        let name = format!(
            "{:<width$}",
            candidate.name,
            width = (line_rect.width as usize).saturating_sub(5)
        );
        let line = Line::from(vec![
            Span::styled(if selected { " ▸ " } else { "   " }.to_string(), row_style),
            Span::styled("●", swatch_style),
            Span::styled(" ", row_style),
            Span::styled(name, row_style),
        ]);
        frame.render_widget(Paragraph::new(line), line_rect);
        // A click selects the row — which previews it — and commits, the way
        // Enter does from the keyboard.
        hits.push((line_rect, row, ModalClick::Activate));
    }
    column.gap();
    column.text(
        frame,
        "↑/↓ preview · Enter apply · Esc revert",
        theme::muted(),
    );
}

// ------------------------------------------------------------------ Confirm

fn confirm(frame: &mut Frame, modal: &ConfirmModal, area: Rect, hits: &mut Hits) {
    let message: Vec<&str> = modal.message.split('\n').collect();
    // The message, gap, buttons, gap, key hints.
    let rect = widgets::centered_rect(WIDTH, CHROME + message.len() as u16 + 6, area);
    // Python gave this one title `label-destructive`, so it reads as a warning.
    let mut column = dialog(frame, rect, &modal.title, theme::destructive());

    for line in message {
        column.text(frame, line.to_string(), theme::secondary());
    }
    column.gap();
    // `ConfirmModal::set_focus` reads 0 as Cancel and 1 as Delete.
    column.controls(
        frame,
        hits,
        vec![
            control("Cancel", !modal.confirm_focused, theme::Variant::Normal, 0),
            control(
                modal.action.confirm_label(),
                modal.confirm_focused,
                theme::Variant::Destructive,
                1,
            ),
        ],
        true,
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

    /// What a click on a cell does, resolved the way `App::hit_at` does.
    fn index_click(hits: &Hits, (column, row): (u16, u16)) -> Option<ModalClick> {
        hits.iter()
            .rev()
            .find(|(rect, _, _)| {
                column >= rect.x
                    && column < rect.x + rect.width
                    && row >= rect.y
                    && row < rect.y + rect.height
            })
            .map(|(_, _, click)| *click)
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
                Modal::ThemePicker(crate::modal::ThemePickerModal::new()),
                "Theme",
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

    /// A dialog taller than the terminal must still show the row you act on.
    /// Boxing the buttons made every modal 6-10 rows taller, and an 80x24
    /// terminal is not unusual — dropping the button row there would leave a
    /// dialog you can only dismiss from the keyboard, with nothing on screen
    /// saying so.
    #[test]
    fn buttons_survive_a_short_terminal() {
        let mut checked = 0;
        for (modal, title) in every_modal() {
            // Not every dialog has a button row — the list editors drive
            // themselves from a key hint instead. Ask a roomy render which ones
            // do, so this cannot quietly pass by checking nothing.
            if !render(&modal, 160, 40).contains("Cancel") {
                continue;
            }
            checked += 1;
            let (screen, hits) = render_with_hits(&modal, 80, 24);
            assert!(
                screen.contains("Cancel"),
                "{title}: button row fell off an 80x24 terminal:\n{screen}"
            );
            let cell = cell_of(&screen, "Cancel");
            assert!(
                index_at(&hits, cell).is_some(),
                "{title}: Cancel drawn but not clickable at 80x24:\n{screen}"
            );
        }
        assert!(checked >= 4, "expected several modals to have buttons");
    }

    /// The other half of the short-terminal contract: what is *not* drawn must
    /// not be clickable.
    ///
    /// Every row that overflows the dialog is pinned to the same three rows at
    /// the bottom, so each one is painted over by whatever is drawn after it.
    /// Their hit regions used to survive that, stacked on top of each other —
    /// an invisible control answering clicks from the margin around the row
    /// that replaced it. Two controls claiming one cell is the signature, and
    /// no dialog that fits has any business producing it either.
    #[test]
    fn controls_never_claim_the_same_cell_twice() {
        let mut checked = 0;
        for (modal, title) in every_modal() {
            for height in [8, 10, 12, 14, 24] {
                let (screen, hits) = render_with_hits(&modal, 80, height);
                checked += 1;

                for (a, first) in hits.iter().enumerate() {
                    for second in hits.iter().skip(a + 1) {
                        assert!(
                            !first.0.intersects(second.0),
                            "{title} at 80x{height}: focus {} and focus {} both \
                             claim {:?} ∩ {:?} — one of them is not on screen:\n{screen}",
                            first.1,
                            second.1,
                            first.0,
                            second.0
                        );
                    }
                }
            }
        }
        assert!(checked >= 20, "expected every modal at every height");
    }

    /// Clicking the mode option that is already active must not switch away
    /// from it. Both segments used to carry the row's Enter, which toggles.
    #[test]
    fn clicking_the_active_mode_option_keeps_it() {
        let modal = worktree_modal(true);
        let (screen, hits) = render_with_hits(&modal, 100, 30);

        let new_branch = index_click(&hits, cell_of(&screen, "New Branch"));
        let existing = index_click(&hits, cell_of(&screen, "Existing"));

        // Left selects "New Branch", Right selects "Existing" — the same key
        // whichever mode is current, so re-clicking the active one is a no-op.
        assert_eq!(
            new_branch,
            Some(ModalClick::Cycle(Direction::Previous)),
            "the New Branch pill does not select New Branch"
        );
        assert_eq!(
            existing,
            Some(ModalClick::Cycle(Direction::Next)),
            "the Existing pill does not select Existing"
        );
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
        modal.focus = AddWorktreeModal::FOCUS_RESULTS;
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
        // Every index below is the constant the modal in `src/modal.rs` acts on;
        // a click that focused anything else would act on the wrong control.
        let settings = Modal::Settings(Box::new(SettingsModal::new(&Settings::default())));
        let (screen, hits) = render_with_hits(&settings, 160, 40);
        for (needle, index) in [
            ("│ ◂ Vim (tmux) ▸ │", SettingsModal::FOCUS_EDITOR),
            ("feat/", SettingsModal::FOCUS_PREFIX),
            ("│ Forest Dark… │", SettingsModal::FOCUS_THEME),
            ("│ Manage Custom Buttons... │", SettingsModal::FOCUS_MANAGE),
            // Short labels are padded out to Textual's `min-width: 10`.
            ("│  Save  │", SettingsModal::FOCUS_SAVE),
            ("│ Cancel │", SettingsModal::FOCUS_CANCEL),
        ] {
            assert_eq!(
                index_at(&hits, cell_of(&screen, needle)),
                Some(index),
                "{needle} in:\n{screen}"
            );
        }

        // A button is three rows tall, and every one of them has to answer the
        // mouse — its border is part of the button.
        let (column, row) = cell_of(&screen, "│  Save  │");
        for offset in [-1i16, 0, 1] {
            let y = row.saturating_add_signed(offset);
            assert_eq!(
                index_at(&hits, (column, y)),
                Some(SettingsModal::FOCUS_SAVE),
                "row {y}:\n{screen}"
            );
        }

        let add = Modal::AddRepository(AddRepositoryModal::new());
        let (screen, hits) = render_with_hits(&add, 100, 20);
        for (needle, index) in [
            ("Enter path or paste", AddRepositoryModal::FOCUS_PATH),
            (
                "│ [ ] Import existing worktrees │",
                AddRepositoryModal::FOCUS_IMPORT,
            ),
            ("│ Add Repository │", AddRepositoryModal::FOCUS_ADD),
            ("│ Cancel │", AddRepositoryModal::FOCUS_CANCEL),
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
            index_at(&hits, cell_of(&screen, "│ Cancel │")),
            Some(0),
            "{screen}"
        );
        assert_eq!(
            index_at(&hits, cell_of(&screen, "│ Delete │")),
            Some(1),
            "{screen}"
        );
    }

    #[test]
    fn branch_rows_are_clickable() {
        let (screen, hits) = render_with_hits(&worktree_modal(false), 100, 40);
        // The dropdown is a single focus stop, so every visible row records a
        // region against the results-list index.
        let results = AddWorktreeModal::FOCUS_RESULTS;
        let rows = hits
            .iter()
            .filter(|(_, index, _)| *index == results)
            .count();
        assert_eq!(rows, 3, "one region per visible branch:\n{screen}");
        assert_eq!(
            index_at(&hits, cell_of(&screen, "origin/main")),
            Some(results),
            "{screen}"
        );
    }
}
