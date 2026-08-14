//! The detail pane: repository view, worktree view, and the empty state.
//!
//! The pane is immediate-mode, so there is nothing that remembers where a
//! control was drawn. Instead every frame walks the controls in exactly the
//! order `App::detail_items()` produces them and records the cells each one
//! landed on. Renderer and key handler therefore agree on what item N is, which
//! is the whole reason `Enter` fires the action the cursor is sitting on, and
//! the recorded cells are what a click resolves against. Any new control here
//! needs a matching entry there, in the same position.

use crate::app::{App, Focus, HitTarget};
use crate::models::{ClaudeSession, GitHubIssue};
use crate::theme;
use crate::ui::widgets::{TextInput, centered_rect};
use crate::ui::{button, button_width};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

/// Frames of the issue-refresh spinner, carried over from the Textual build.
const SPINNER: [char; 4] = ['|', '/', '-', '\\'];

/// Width of the label column in the rename section.
const FIELD_LABEL_WIDTH: usize = 14;

/// Cells a card's border and padding take up on the left of its content, and
/// again on the right. Textual's cards were `border: solid` plus `padding: 1`.
const CARD_INSET: u16 = 2;

pub fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    // One column of padding on each side; the sidebar already draws the divider.
    let inner = Rect {
        x: area.x + 1,
        width: area.width.saturating_sub(2),
        ..area
    };

    let Some(pane) = build(app, inner.width) else {
        empty_state(frame, area);
        return;
    };
    let offset = pane.scroll_offset(inner.height);
    frame.render_widget(Paragraph::new(pane.lines).scroll((offset, 0)), inner);

    // The scroll offset decides which line ends up on which screen row, so this
    // is the earliest the clickable regions can be worked out.
    for (index, item) in pane.items.iter().enumerate() {
        let Some(row) = item.line.checked_sub(offset) else {
            continue;
        };
        if row >= inner.height {
            break;
        }
        app.push_hit(
            Rect {
                x: inner.x + item.x,
                y: inner.y + row,
                width: item.width,
                height: 1,
            },
            HitTarget::DetailItem(index),
        );
    }
}

/// Render the pane for the current selection, or `None` for the empty state.
fn build(app: &App, width: u16) -> Option<Pane> {
    let focused = match app.focus {
        Focus::Detail => Some(app.detail_index),
        Focus::Sidebar => None,
    };
    let mut pane = Pane::new(focused, width);

    if app.state.selection.is_worktree() {
        worktree(&mut pane, app);
    } else if app.state.selection.is_repository() {
        repository(&mut pane, app);
    } else {
        return None;
    }
    Some(pane)
}

/// Where a focusable item landed. The line alone would not do: `Editor`,
/// `Terminal` and `Files` share one, so a click needs the x extent too.
struct Item {
    line: u16,
    x: u16,
    width: u16,
}

/// A control ("button") waiting to be laid out.
struct Control {
    label: String,
    destructive: bool,
    /// A control that cannot run still occupies a slot in `detail_items()`.
    enabled: bool,
}

impl Control {
    fn new(label: impl Into<String>, destructive: bool) -> Self {
        Self {
            label: label.into(),
            destructive,
            enabled: true,
        }
    }

    fn disabled(label: impl Into<String>) -> Self {
        Self {
            enabled: false,
            ..Self::new(label, false)
        }
    }
}

/// Content under construction: the lines to render plus the cells each
/// focusable item claimed, in `App::detail_items()` order.
struct Pane {
    lines: Vec<Line<'static>>,
    items: Vec<Item>,
    /// Index of the focused item, or `None` while the sidebar owns focus.
    focused: Option<usize>,
    /// Width of the content area, so rules and cards can be filled out to it.
    width: u16,
    /// Width of the card currently open, if any. See [`Pane::card_start`].
    card: Option<u16>,
}

impl Pane {
    fn new(focused: Option<usize>, width: u16) -> Self {
        Self {
            lines: Vec::new(),
            items: Vec::new(),
            focused,
            width,
            card: None,
        }
    }

    fn text(&mut self, text: impl Into<String>, style: Style) {
        self.push_line(vec![Span::styled(text.into(), style)]);
    }

    fn blank(&mut self) {
        self.lines.push(Line::default());
    }

    /// A section header, spaced away from whatever came before it.
    fn section(&mut self, title: &str) {
        if !self.lines.is_empty() {
            self.blank();
        }
        self.text(title.to_string(), theme::section_header());
    }

    /// The horizontal `Rule()` Textual drew between the major sections, with the
    /// blank line above it that its `margin` produced. The blank below is left to
    /// [`Pane::section`], which already spaces a header away from what precedes it.
    fn rule(&mut self) {
        self.blank();
        self.text("─".repeat(self.width as usize), theme::border());
    }

    /// Push a line built from spans returned by [`Pane::control`].
    fn row(&mut self, spans: Vec<Span<'static>>) {
        self.push_line(spans);
    }

    /// Open a card — the bordered, elevated box Textual gave `.session-item` and
    /// `.issue-row`. Lines pushed until [`Pane::card_end`] land inside it.
    fn card_start(&mut self, width: u16) {
        self.lines.push(card_edge(width, '┌', '┐'));
        self.card = Some(width);
    }

    fn card_end(&mut self) {
        if let Some(width) = self.card.take() {
            self.lines.push(card_edge(width, '└', '┘'));
        }
    }

    /// Push one content line, wrapped in the open card's border and background.
    fn push_line(&mut self, mut spans: Vec<Span<'static>>) {
        let Some(width) = self.card else {
            self.lines.push(Line::from(spans));
            return;
        };
        let filled = Style::default().bg(theme::BG_ELEVATED);
        // Text styles carry a foreground only, so without this the page colour
        // shows through the card. Spans that set their own background — the
        // controls — keep it, which is what makes a pill read as raised.
        for span in &mut spans {
            if span.style.bg.is_none() {
                span.style = span.style.bg(theme::BG_ELEVATED);
            }
        }
        let content = as_u16(spans.iter().map(Span::width).sum());
        // Left border, left padding, content, then whatever is left over before
        // the padding and border on the right.
        let fill = width.saturating_sub(CARD_INSET + content + 1);
        let mut line = vec![
            Span::styled("│", theme::border().bg(theme::BG_ELEVATED)),
            Span::styled(" ", filled),
        ];
        line.append(&mut spans);
        line.push(Span::styled(" ".repeat(fill as usize), filled));
        line.push(Span::styled("│", theme::border().bg(theme::BG_ELEVATED)));
        self.lines.push(Line::from(line));
    }

    /// Claim the next focusable slot at `x` on the line that is about to be
    /// pushed, returning whether it is the focused one. Callers must push that
    /// line before adding any other, or the recorded cells drift from the
    /// content.
    fn claim(&mut self, x: u16, width: u16) -> bool {
        let focused = self.focused == Some(self.items.len());
        self.items.push(Item {
            line: as_u16(self.lines.len()),
            x,
            width,
        });
        focused
    }

    /// Lay out a row of controls after `lead`, which is whatever non-clickable
    /// text shares the line. Each control's extent is recorded as it is placed,
    /// because that running x offset is the only place it is ever known.
    fn controls(&mut self, lead: Vec<Span<'static>>, controls: &[Control]) {
        let mut spans = lead;
        let inset = self.card.map_or(0, |_| CARD_INSET);
        let mut x = inset.saturating_add(as_u16(spans.iter().map(Span::width).sum()));
        for (index, control) in controls.iter().enumerate() {
            if index > 0 {
                spans.push(Span::raw(" "));
                x = x.saturating_add(1);
            }
            let width = button_width(&control.label);
            let focused = self.claim(x, width);
            let mut pill = button(&control.label, focused, control.destructive);
            // A control that cannot run keeps the pill and its width, so neither
            // the layout nor the hit region shifts; it only loses its colour —
            // except under the cursor, which has to stay visible.
            if !control.enabled && !focused {
                for span in &mut pill {
                    span.style = theme::muted();
                }
            }
            spans.append(&mut pill);
            x = x.saturating_add(width);
        }
        self.push_line(spans);
    }

    /// Offset that keeps the focused control on screen. `draw` builds the pane
    /// fresh each frame, so this is derived rather than stored on the app.
    fn scroll_offset(&self, height: u16) -> u16 {
        let Some(row) = self
            .focused
            .and_then(|index| self.items.get(index))
            .map(|item| item.line)
        else {
            return 0;
        };
        let max = as_u16(self.lines.len()).saturating_sub(height);
        // Aim to keep two lines of context below the cursor where there is room.
        row.saturating_add(3).saturating_sub(height).min(max)
    }
}

/// Terminal geometry is `u16` while content lengths are `usize`.
fn as_u16(len: usize) -> u16 {
    u16::try_from(len).unwrap_or(u16::MAX)
}

/// Top or bottom edge of a card.
fn card_edge(width: u16, left: char, right: char) -> Line<'static> {
    let span = width.saturating_sub(2) as usize;
    Line::from(Span::styled(
        format!("{left}{}{right}", "─".repeat(span)),
        theme::border().bg(theme::BG_ELEVATED),
    ))
}

/// A path in the boxed, elevated `.path-display` Textual used. The box hugs the
/// path rather than filling the pane, which is how it laid out there.
fn path_box(pane: &mut Pane, path: String, style: Style) {
    // A forest path routinely outruns the pane. Cutting it here rather than
    // letting the paragraph clip it keeps the box's right edge on screen; the
    // tail was lost either way.
    let room = pane.width.saturating_sub(2 * CARD_INSET) as usize;
    // `truncate` adds an ellipsis, so it needs three of those cells to itself.
    let path = if path.chars().count() > room {
        crate::util::truncate(&path, room.saturating_sub(3))
    } else {
        path
    };
    let width = as_u16(path.chars().count()).saturating_add(2 * CARD_INSET);
    pane.card_start(width);
    pane.text(path, style);
    pane.card_end();
}

// --------------------------------------------------------------------- panes

fn repository(pane: &mut Pane, app: &App) {
    let repository = app.state.selected_repository();
    let name = repository.map(|r| r.name.as_str()).unwrap_or_default();
    let path = repository
        .map(|r| r.source_path.as_str())
        .unwrap_or_default();

    pane.section("MAIN REPOSITORY");
    pane.text(format!("Repository: {name}"), theme::title());
    if let Some(branch) = &app.meta.branch {
        pane.text(format!("Branch:     {branch}"), theme::accent());
    }
    commit_line(pane, app);

    pane.controls(
        Vec::new(),
        &[
            sync_control(app, false),
            Control::new("Add Worktree", false),
        ],
    );

    pane.rule();
    pane.section("LOCATION");
    path_box(pane, path.to_string(), theme::secondary());

    pane.rule();
    open_in(pane);
    // Textual ran CLAUDE straight into RECENT SESSIONS with no rule between them.
    pane.rule();
    claude(pane, app);
    sessions(pane, app);

    pane.rule();
    issues(pane, app);

    pane.rule();
    pane.section("MANAGE");
    pane.controls(Vec::new(), &[Control::new("Remove Repository", true)]);
}

fn worktree(pane: &mut Pane, app: &App) {
    // Read defensively: a selection can briefly outlive the worktree it points
    // at, and the sequence of focusable items must not change when it does.
    let selected = app.state.selected_worktree();
    let repository = selected
        .map(|(repo, _)| repo.name.as_str())
        .unwrap_or_default();
    let name = selected.map(|(_, w)| w.name.as_str()).unwrap_or_default();
    let branch = selected.map(|(_, w)| w.branch.as_str()).unwrap_or_default();
    let archived = selected.map(|(_, w)| w.is_archived).unwrap_or(false);

    pane.section("WORKTREE");
    pane.text(format!("Repository: {repository}"), theme::title());
    pane.text(format!("Worktree:   {name}"), theme::primary());
    pane.text(format!("Branch:     {branch}"), theme::accent());
    if !app.meta.path_exists {
        pane.text(
            "⚠ MISSING:   directory no longer exists on disk",
            theme::destructive(),
        );
    }
    if let Some(base) = selected.and_then(|(_, w)| w.base_branch.as_deref()) {
        let mut text = format!("Based on:   {base}");
        if let Some(reference) = selected.and_then(|(_, w)| w.created_from_ref.as_deref()) {
            text.push_str(&format!(" ({reference})"));
        }
        pane.text(text, theme::muted());
    }
    commit_line(pane, app);

    pane.controls(Vec::new(), &[sync_control(app, !app.meta.path_exists)]);

    pane.rule();
    pane.section("LOCATION");
    if app.meta.path_exists {
        path_box(pane, app.meta.path.clone(), theme::secondary());
    } else {
        path_box(
            pane,
            format!("{}  (missing)", app.meta.path),
            theme::destructive(),
        );
    }

    pane.rule();
    open_in(pane);
    pane.rule();
    claude(pane, app);
    sessions(pane, app);

    pane.rule();
    pane.section("RENAME");
    field(pane, "Worktree name", &app.name_input);
    field(pane, "Branch name", &app.branch_input);

    pane.rule();
    pane.section("MANAGE");
    pane.controls(
        Vec::new(),
        &[
            Control::new(if archived { "Unarchive" } else { "Archive" }, false),
            Control::new("Delete", true),
        ],
    );
}

// ------------------------------------------------------------------ sections

fn commit_line(pane: &mut Pane, app: &App) {
    let Some(hash) = &app.meta.commit_hash else {
        return;
    };
    let mut text = format!("Commit:     {hash}");
    if let Some(when) = app.meta.commit_time {
        text.push_str(&format!(" ({})", crate::util::naturaltime(when)));
    }
    pane.text(text, theme::muted());
}

/// The `⟳ Git Pull` control. Disabled when there is no remote to pull from, or
/// — for worktrees — when the directory is gone.
fn sync_control(app: &App, missing_directory: bool) -> Control {
    // Two spaces after the glyph: ⟳ is double-width in many terminals, and with a
    // single space it eats the gap and the label reads as "⟳Git Pull".
    if missing_directory {
        Control::disabled("⟳  Git Pull (Directory missing)")
    } else if app.meta.has_remote {
        Control::new("⟳  Git Pull", false)
    } else {
        Control::disabled("⟳  Git Pull (No remote)")
    }
}

fn open_in(pane: &mut Pane) {
    pane.section("OPEN IN");
    pane.controls(
        Vec::new(),
        &[
            Control::new("Editor", false),
            Control::new("Terminal", false),
            Control::new("Files", false),
        ],
    );
}

fn claude(pane: &mut Pane, app: &App) {
    pane.section("CLAUDE");
    let mut controls = vec![
        Control::new("New Session", false),
        Control::new("New Session: YOLO", true),
    ];
    controls.extend(custom_controls(app));
    pane.controls(Vec::new(), &controls);
}

/// The user's own Claude buttons, which follow both the new-session and the
/// resume controls.
fn custom_controls(app: &App) -> impl Iterator<Item = Control> + '_ {
    app.settings
        .custom_buttons
        .iter()
        .map(|custom| Control::new(custom.label.as_str(), custom.is_yolo_style()))
}

fn sessions(pane: &mut Pane, app: &App) {
    pane.section("RECENT SESSIONS");
    let Some(list) = app.sessions.as_deref() else {
        pane.text("Loading...", theme::muted());
        return;
    };
    if list.is_empty() {
        pane.text("No sessions found", theme::muted());
        return;
    }
    for (index, session) in list.iter().enumerate() {
        if index > 0 {
            pane.blank();
        }
        session_item(pane, app, session);
    }
}

fn session_item(pane: &mut Pane, app: &App, session: &ClaudeSession) {
    pane.card_start(pane.width);
    pane.text(crate::util::truncate(&session.title, 60), theme::primary());
    if !session.last_message.is_empty() && session.last_message != session.title {
        pane.text(
            format!("> {}", crate::util::truncate(&session.last_message, 40)),
            theme::secondary(),
        );
    }
    pane.text(
        format!(
            "{} • {} msgs",
            session.relative_time(),
            session.message_count
        ),
        theme::muted(),
    );

    let mut controls = vec![Control::new("Resume", false), Control::new("YOLO", true)];
    controls.extend(custom_controls(app));
    pane.controls(Vec::new(), &controls);
    pane.card_end();
}

fn issues(pane: &mut Pane, app: &App) {
    // The refresh control sits on the header line and doubles as the loading
    // spinner, the same as the Textual button it replaces.
    pane.blank();
    let label = match app.issues {
        Some(_) => "↻".to_string(),
        None => SPINNER[app.spinner_index % SPINNER.len()].to_string(),
    };
    pane.controls(
        vec![
            Span::styled("MY OPEN GITHUB ISSUES", theme::section_header()),
            Span::raw(" "),
        ],
        &[Control::new(label, false)],
    );

    let Some(list) = app.issues.as_deref() else {
        pane.text("Loading...", theme::muted());
        return;
    };
    if list.is_empty() {
        pane.text("No issues found", theme::muted());
        return;
    }
    for (index, issue) in list.iter().enumerate() {
        if index > 0 {
            pane.blank();
        }
        issue_item(pane, issue);
    }
}

fn issue_item(pane: &mut Pane, issue: &GitHubIssue) {
    pane.card_start(pane.width);
    pane.text(
        format!(
            "#{} {}",
            issue.number,
            crate::util::truncate(&issue.title, 45)
        ),
        theme::primary(),
    );

    let mut meta = issue.relative_time();
    let labels: Vec<&str> = issue
        .labels
        .iter()
        .take(2)
        .map(|label| label.name.as_str())
        .collect();
    if !labels.is_empty() {
        meta.push_str(&format!(" • {}", labels.join(", ")));
    }
    pane.controls(
        vec![Span::styled(format!("{meta}  "), theme::muted())],
        &[Control::new("Create WT", false)],
    );
    pane.card_end();
}

/// A rename field. The caret is drawn in the text because the pane renders as
/// one scrolled paragraph and has no real terminal cursor to place.
fn field(pane: &mut Pane, label: &str, input: &TextInput) {
    // Measured without consulting focus: the caret only ever occupies the one
    // trailing cell already counted here, so the click target is stable.
    let width = as_u16(FIELD_LABEL_WIDTH + 2 + input.value().chars().count());
    let focused = pane.claim(0, width);
    let mut spans = vec![Span::styled(
        format!("{label:<FIELD_LABEL_WIDTH$} "),
        theme::secondary(),
    )];

    if focused {
        let chars: Vec<char> = input.value().chars().collect();
        let cursor = input.cursor().min(chars.len());
        let before: String = chars[..cursor].iter().collect();
        let after: String = chars
            .get(cursor + 1..)
            .map(|rest| rest.iter().collect())
            .unwrap_or_default();
        // A cursor past the last character needs a cell of its own to sit in.
        let at = chars.get(cursor).copied().unwrap_or(' ');
        spans.push(Span::styled(before, theme::primary()));
        spans.push(Span::styled(at.to_string(), theme::cursor()));
        spans.push(Span::styled(after, theme::primary()));
    } else {
        spans.push(Span::styled(input.value().to_string(), theme::primary()));
    }
    pane.row(spans);
}

/// Nothing selected. The Textual version put this inside a zero-height
/// container, so it never appeared; centring it is what that build intended.
fn empty_state(frame: &mut Frame, area: Rect) {
    let lines = vec![
        Line::from(Span::styled(" forestui", theme::accent())),
        Line::from(Span::styled("Git Worktree Manager", theme::secondary())),
        Line::default(),
        Line::from(Span::styled(
            "Select a repository or worktree",
            theme::muted(),
        )),
        Line::from(Span::styled(
            "or press [a] to add a repository",
            theme::muted(),
        )),
    ];
    let rect = centered_rect(area.width, as_u16(lines.len()), area);
    frame.render_widget(Paragraph::new(lines).centered(), rect);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{Action, DetailItem, Field};
    use crate::event;
    use crate::models::{CustomClaudeButton, Repository, Settings, Worktree};
    use crate::services::settings as settings_service;
    use crate::state::AppState;
    use chrono::Utc;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    /// An app whose state file lives in a throwaway forest directory.
    ///
    /// `App::new` reads the real settings and forest path, so both are pointed
    /// at temporary locations first and then replaced outright.
    fn test_app(dir: &tempfile::TempDir) -> App {
        settings_service::set_forest_path(dir.path().to_str());
        // Dropping the receiver is fine: sends are best-effort and no test reads
        // events back.
        let (tx, _rx) = event::start();
        let mut app = App::new(tx, "test".into());
        app.state = AppState::load_from(dir.path().join(".forestui-config.json"));
        app.settings = Settings::default();
        app
    }

    fn with_repository(app: &mut App) {
        let repository = Repository::new("demo".into(), "/tmp/demo".into());
        let repo_id = repository.id;
        app.state.add_repository(repository);
        app.state.select_repository(repo_id);
    }

    fn with_worktree(app: &mut App) {
        let repository = Repository::new("demo".into(), "/tmp/demo".into());
        let repo_id = repository.id;
        app.state.add_repository(repository);
        let worktree = Worktree::new("wt".into(), "feat/x".into(), "/tmp/forest/demo/wt".into());
        let worktree_id = worktree.id;
        app.state.add_worktree(repo_id, worktree);
        app.state.select_worktree(repo_id, worktree_id);
    }

    fn a_session() -> ClaudeSession {
        ClaudeSession {
            id: "abc".into(),
            title: "Refactor the detail pane".into(),
            last_message: "and then some".into(),
            last_timestamp: Utc::now(),
            message_count: 12,
        }
    }

    fn an_issue() -> GitHubIssue {
        serde_json::from_str(
            r#"{"number":42,"title":"Fix login bug","state":"OPEN","url":"u",
                "createdAt":"2026-01-01T00:00:00Z","updatedAt":"2026-01-01T00:00:00Z",
                "author":{"login":"me"},"assignees":[],
                "labels":[{"name":"bug","color":"ff0000"}]}"#,
        )
        .expect("issue fixture parses")
    }

    /// One frame into a throwaway terminal. `crate::ui::draw` is what clears
    /// hits in the real app, so the test does it here.
    fn render_buffer(app: &mut App, width: u16, height: u16) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
        app.clear_hits();
        terminal
            .draw(|frame| draw(frame, app, frame.area()))
            .expect("draw");
        terminal.backend().buffer().clone()
    }

    fn render(app: &mut App) -> String {
        buffer_text(&render_buffer(app, 100, 40))
    }

    /// Cell of the first occurrence of `needle`. Byte offsets would not do: the
    /// pill caps around every control are multi-byte.
    fn find_cell(buffer: &Buffer, needle: &str) -> (u16, u16) {
        let width = buffer.area.width as usize;
        for (y, row) in buffer.content.chunks(width).enumerate() {
            let line: String = row.iter().map(|cell| cell.symbol()).collect();
            if let Some(byte) = line.find(needle) {
                return (as_u16(line[..byte].chars().count()), as_u16(y));
            }
        }
        panic!("{needle:?} is not on screen");
    }

    fn detail_hits(app: &App) -> usize {
        app.hits
            .iter()
            .filter(|hit| matches!(hit.target, HitTarget::DetailItem(_)))
            .count()
    }

    fn buffer_text(buffer: &Buffer) -> String {
        let width = buffer.area.width as usize;
        buffer
            .content
            .chunks(width)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Textual put a `Rule()` between the major sections, and the pane read as
    /// one flat list without them.
    #[tokio::test]
    async fn sections_are_separated_by_rules() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = test_app(&dir);
        with_repository(&mut app);
        // Loaded but empty, so the whole pane fits on screen unscrolled.
        app.sessions = Some(vec![]);
        app.issues = Some(vec![]);

        /// Whether a rule sits between two lines. A rule is the only line that is
        /// nothing but the horizontal glyph; a card's edges start with a corner.
        fn rule_between(screen: &str, from: &str, to: &str) -> bool {
            let rows: Vec<&str> = screen.lines().collect();
            let row_of = |needle: &str| {
                rows.iter()
                    .position(|row| row.contains(needle))
                    .unwrap_or_else(|| panic!("{needle:?} is not on screen:\n{screen}"))
            };
            (row_of(from)..row_of(to)).any(|y| rows[y].trim().starts_with("──"))
        }

        let screen = render(&mut app);
        assert!(
            rule_between(&screen, "Repository: demo", "LOCATION"),
            "{screen}"
        );
        assert!(rule_between(&screen, "LOCATION", "OPEN IN"), "{screen}");
        assert!(rule_between(&screen, "OPEN IN", "CLAUDE"), "{screen}");
        assert!(
            rule_between(&screen, "RECENT SESSIONS", "MY OPEN GITHUB ISSUES"),
            "{screen}"
        );
        assert!(
            rule_between(&screen, "MY OPEN GITHUB ISSUES", "MANAGE"),
            "{screen}"
        );
        // CLAUDE ran straight into RECENT SESSIONS there, and does here too.
        assert!(
            !rule_between(&screen, "CLAUDE", "RECENT SESSIONS"),
            "{screen}"
        );

        let dir = tempfile::tempdir().expect("tempdir");
        app.state = AppState::load_from(dir.path().join(".forestui-config.json"));
        with_worktree(&mut app);
        let screen = render(&mut app);
        assert!(
            rule_between(&screen, "RECENT SESSIONS", "RENAME"),
            "{screen}"
        );
        assert!(rule_between(&screen, "Branch name", "MANAGE"), "{screen}");
    }

    /// Sessions and issues were `.session-item` / `.issue-row` boxes, not bare
    /// lines: a border in the border colour over an elevated background.
    #[tokio::test]
    async fn session_and_issue_items_render_as_cards() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = test_app(&dir);
        with_repository(&mut app);
        app.sessions = Some(vec![a_session()]);
        app.issues = Some(vec![an_issue()]);
        let buffer = render_buffer(&mut app, 100, 60);
        // The pane keeps a column of padding on each side, so a full-width card
        // ends one cell in from the right edge of the screen.
        let right = buffer.area.width - 2;

        for label in ["Refactor the detail pane", "#42 Fix login bug"] {
            let (x, y) = find_cell(&buffer, label);
            let cell = |x: u16, y: u16| {
                buffer
                    .cell((x, y))
                    .unwrap_or_else(|| panic!("{label}: no cell at {x},{y}"))
            };

            let left = x - CARD_INSET;
            assert_eq!(cell(left, y - 1).symbol(), "┌", "{label}: top left");
            assert_eq!(cell(right, y - 1).symbol(), "┐", "{label}: top right");
            assert_eq!(cell(left, y).symbol(), "│", "{label}: left border");
            assert_eq!(cell(right, y).symbol(), "│", "{label}: right border");
            assert_eq!(cell(left, y).fg, theme::BORDER, "{label}: border colour");
            // The background has to reach past the text, or the card is a frame
            // around the page colour rather than a filled box.
            for probe in [x, right - 1] {
                assert_eq!(
                    cell(probe, y).bg,
                    theme::BG_ELEVATED,
                    "{label}: background at {probe}",
                );
            }
        }
    }

    #[tokio::test]
    async fn empty_state_is_visible() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = test_app(&dir);
        let screen = render(&mut app);

        // The Textual build laid this out into zero height and showed nothing.
        assert!(screen.contains("forestui"), "{screen}");
        assert!(screen.contains("Git Worktree Manager"), "{screen}");
        assert!(
            screen.contains("Select a repository or worktree"),
            "{screen}"
        );
        assert!(
            screen.contains("or press [a] to add a repository"),
            "{screen}"
        );
    }

    #[tokio::test]
    async fn repository_detail_renders_its_sections() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = test_app(&dir);
        with_repository(&mut app);
        let screen = render(&mut app);

        assert!(screen.contains("MAIN REPOSITORY"), "{screen}");
        assert!(screen.contains("Repository: demo"), "{screen}");
        assert!(screen.contains("LOCATION"), "{screen}");
        assert!(screen.contains("/tmp/demo"), "{screen}");
        assert!(screen.contains("CLAUDE"), "{screen}");
        assert!(screen.contains("New Session"), "{screen}");
    }

    #[tokio::test]
    async fn missing_worktree_directory_is_flagged() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = test_app(&dir);
        with_worktree(&mut app);
        app.meta = event::DetailMeta {
            path: "/tmp/forest/demo/wt".into(),
            path_exists: false,
            ..event::DetailMeta::default()
        };
        let screen = render(&mut app);

        assert!(screen.contains("⚠ MISSING"), "{screen}");
        assert!(screen.contains("(missing)"), "{screen}");
        assert!(screen.contains("Git Pull (Directory missing)"), "{screen}");
    }

    /// The invariant: rendered control N is `detail_items()[N]`, so the counts
    /// must agree for every selection and every combination of loaded data.
    #[tokio::test]
    async fn rendered_controls_match_detail_items() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = test_app(&dir);
        app.settings.custom_buttons = vec![
            CustomClaudeButton {
                label: "Opus".into(),
                prefix: "opus".into(),
                command: "claude --model opus".into(),
            },
            CustomClaudeButton {
                label: "Disc".into(),
                prefix: "disc".into(),
                command: "claude --dangerously-skip-permissions".into(),
            },
        ];

        with_repository(&mut app);
        for sessions in [None, Some(vec![]), Some(vec![a_session(), a_session()])] {
            for issues in [None, Some(vec![]), Some(vec![an_issue()])] {
                app.sessions = sessions.clone();
                app.issues = issues.clone();
                let pane = build(&app, 100).expect("repository pane");
                assert_eq!(
                    pane.items.len(),
                    app.detail_items().len(),
                    "repository, {} sessions, {} issues",
                    app.sessions.as_ref().map_or(-1, |s| s.len() as i64),
                    app.issues.as_ref().map_or(-1, |i| i.len() as i64),
                );
            }
        }

        let dir = tempfile::tempdir().expect("tempdir");
        app.state = AppState::load_from(dir.path().join(".forestui-config.json"));
        with_worktree(&mut app);
        for sessions in [None, Some(vec![]), Some(vec![a_session()])] {
            app.sessions = sessions.clone();
            let pane = build(&app, 100).expect("worktree pane");
            assert_eq!(
                pane.items.len(),
                app.detail_items().len(),
                "worktree, {} sessions",
                app.sessions.as_ref().map_or(-1, |s| s.len() as i64),
            );
        }
    }

    #[tokio::test]
    async fn focused_item_is_scrolled_into_view() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = test_app(&dir);
        with_repository(&mut app);
        app.sessions = Some(vec![a_session(), a_session(), a_session()]);
        app.issues = Some(vec![an_issue(), an_issue()]);
        app.focus = Focus::Detail;
        app.detail_index = app.detail_items().len() - 1;

        let pane = build(&app, 100).expect("repository pane");
        let offset = pane.scroll_offset(20);
        let row = pane.items[app.detail_index].line;
        assert!(
            row >= offset && row < offset + 20,
            "row {row}, offset {offset}"
        );

        // The sidebar keeps the pane at the top.
        app.focus = Focus::Sidebar;
        let pane = build(&app, 100).expect("repository pane");
        assert_eq!(pane.scroll_offset(20), 0);
    }

    /// Clicking a control has to reach the item drawn under the pointer, which
    /// is what the bug report said was broken.
    #[tokio::test]
    async fn clicking_a_control_maps_to_its_item() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = test_app(&dir);
        with_worktree(&mut app);
        app.sessions = Some(vec![a_session()]);
        let buffer = render_buffer(&mut app, 100, 60);

        let index_of = |item: &DetailItem| {
            app.detail_items()
                .iter()
                .position(|candidate| candidate == item)
                .expect("item is rendered")
        };

        // Editor, Terminal and Files share a line, so getting these three right
        // is what proves the x extents are, and not just the rows.
        for (label, item) in [
            ("Editor", DetailItem::Action(Action::Editor)),
            ("Terminal", DetailItem::Action(Action::Terminal)),
            ("Files", DetailItem::Action(Action::Files)),
            ("Resume", DetailItem::Action(Action::ResumeSession(0))),
            ("Delete", DetailItem::Action(Action::Delete)),
            ("Worktree name", DetailItem::Field(Field::WorktreeName)),
        ] {
            let (x, y) = find_cell(&buffer, label);
            assert_eq!(
                app.hit_at(x, y),
                Some(HitTarget::DetailItem(index_of(&item))),
                "{label} at {x},{y}"
            );
        }
    }

    #[tokio::test]
    async fn every_focusable_item_is_clickable_when_visible() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = test_app(&dir);
        app.settings.custom_buttons = vec![CustomClaudeButton {
            label: "Opus".into(),
            prefix: "opus".into(),
            command: "claude --model opus".into(),
        }];
        app.sessions = Some(vec![a_session()]);
        app.issues = Some(vec![an_issue()]);

        with_worktree(&mut app);
        // Tall and wide enough that nothing is scrolled or clipped away.
        render_buffer(&mut app, 120, 60);
        assert_eq!(detail_hits(&app), app.detail_items().len(), "worktree");

        let dir = tempfile::tempdir().expect("tempdir");
        app.state = AppState::load_from(dir.path().join(".forestui-config.json"));
        with_repository(&mut app);
        render_buffer(&mut app, 120, 60);
        assert_eq!(detail_hits(&app), app.detail_items().len(), "repository");
    }
}
