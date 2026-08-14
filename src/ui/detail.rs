//! The detail pane: repository view, worktree view, and the empty state.
//!
//! The pane is immediate-mode, so there is nothing that remembers where a
//! control was drawn. Instead every frame walks the controls in exactly the
//! order `App::detail_items()` produces them and records the row each one
//! landed on. Renderer and key handler therefore agree on what item N is, which
//! is the whole reason `Enter` fires the action the cursor is sitting on. Any
//! new control here needs a matching entry there, in the same position.

use crate::app::{App, Focus};
use crate::models::{ClaudeSession, GitHubIssue};
use crate::theme;
use crate::ui::widgets::{TextInput, centered_rect};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

/// Frames of the issue-refresh spinner, carried over from the Textual build.
const SPINNER: [char; 4] = ['|', '/', '-', '\\'];

/// Width of the label column in the rename section.
const FIELD_LABEL_WIDTH: usize = 14;

pub fn draw(frame: &mut Frame, app: &App, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let Some(pane) = build(app) else {
        empty_state(frame, area);
        return;
    };

    // One column of padding on each side; the sidebar already draws the divider.
    let inner = Rect {
        x: area.x + 1,
        width: area.width.saturating_sub(2),
        ..area
    };
    let offset = pane.scroll_offset(inner.height);
    frame.render_widget(Paragraph::new(pane.lines).scroll((offset, 0)), inner);
}

/// Render the pane for the current selection, or `None` for the empty state.
fn build(app: &App) -> Option<Pane> {
    let focused = match app.focus {
        Focus::Detail => Some(app.detail_index),
        Focus::Sidebar => None,
    };
    let mut pane = Pane::new(focused);

    if app.state.selection.is_worktree() {
        worktree(&mut pane, app);
    } else if app.state.selection.is_repository() {
        repository(&mut pane, app);
    } else {
        return None;
    }
    Some(pane)
}

/// Content under construction: the lines to render plus the row each focusable
/// item claimed, in `App::detail_items()` order.
struct Pane {
    lines: Vec<Line<'static>>,
    item_rows: Vec<u16>,
    /// Index of the focused item, or `None` while the sidebar owns focus.
    focused: Option<usize>,
}

impl Pane {
    fn new(focused: Option<usize>) -> Self {
        Self {
            lines: Vec::new(),
            item_rows: Vec::new(),
            focused,
        }
    }

    fn text(&mut self, text: impl Into<String>, style: Style) {
        self.lines
            .push(Line::from(Span::styled(text.into(), style)));
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

    /// Push a line built from spans returned by [`Pane::control`].
    fn row(&mut self, spans: Vec<Span<'static>>) {
        self.lines.push(Line::from(spans));
    }

    /// Claim the next focusable slot for the line that is about to be pushed,
    /// returning whether it is the focused one. Callers must push that line
    /// before adding any other, or the recorded row drifts from the content.
    fn claim(&mut self) -> bool {
        let focused = self.focused == Some(self.item_rows.len());
        self.item_rows.push(row_index(self.lines.len()));
        focused
    }

    /// A control ("button"). The padding mirrors Textual's button chrome; the
    /// label itself is verbatim so the wording matches the Python build.
    fn control(&mut self, label: &str, destructive: bool) -> Span<'static> {
        let focused = self.claim();
        Span::styled(format!(" {label} "), theme::action(focused, destructive))
    }

    /// A control that cannot run. It still occupies a slot in `detail_items()`,
    /// so it keeps its place in the sequence and only loses its colour — except
    /// while focused, where the cursor has to stay visible.
    fn disabled_control(&mut self, label: &str) -> Span<'static> {
        let focused = self.claim();
        let style = if focused {
            theme::action(true, false)
        } else {
            theme::muted()
        };
        Span::styled(format!(" {label} "), style)
    }

    /// Offset that keeps the focused control on screen. `draw` only sees
    /// `&App`, so this is derived per frame rather than stored on the app.
    fn scroll_offset(&self, height: u16) -> u16 {
        let Some(row) = self
            .focused
            .and_then(|index| self.item_rows.get(index))
            .copied()
        else {
            return 0;
        };
        let max = row_index(self.lines.len()).saturating_sub(height);
        // Aim to keep two lines of context below the cursor where there is room.
        row.saturating_add(3).saturating_sub(height).min(max)
    }
}

fn row_index(len: usize) -> u16 {
    u16::try_from(len).unwrap_or(u16::MAX)
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

    let sync = sync_control(pane, app, false);
    let add = pane.control(" Add Worktree", false);
    pane.row(vec![sync, Span::raw(" "), add]);

    pane.section("LOCATION");
    pane.text(path.to_string(), theme::secondary());

    open_in(pane);
    claude(pane, app);
    sessions(pane, app);
    issues(pane, app);

    pane.section("MANAGE");
    let remove = pane.control(" Remove Repository", true);
    pane.row(vec![remove]);
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

    let sync = sync_control(pane, app, !app.meta.path_exists);
    pane.row(vec![sync]);

    pane.section("LOCATION");
    if app.meta.path_exists {
        pane.text(app.meta.path.clone(), theme::secondary());
    } else {
        pane.text(
            format!("{}  (missing)", app.meta.path),
            theme::destructive(),
        );
    }

    open_in(pane);
    claude(pane, app);
    sessions(pane, app);

    pane.section("RENAME");
    field(pane, "Worktree name", &app.name_input);
    field(pane, "Branch name", &app.branch_input);

    pane.section("MANAGE");
    let toggle = pane.control(if archived { " Unarchive" } else { " Archive" }, false);
    let delete = pane.control(" Delete", true);
    pane.row(vec![toggle, Span::raw(" "), delete]);
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
fn sync_control(pane: &mut Pane, app: &App, missing_directory: bool) -> Span<'static> {
    if missing_directory {
        pane.disabled_control("⟳ Git Pull (Directory missing)")
    } else if app.meta.has_remote {
        pane.control("⟳ Git Pull", false)
    } else {
        pane.disabled_control("⟳ Git Pull (No remote)")
    }
}

fn open_in(pane: &mut Pane) {
    pane.section("OPEN IN");
    let editor = pane.control(" Editor", false);
    let terminal = pane.control(" Terminal", false);
    let files = pane.control(" Files", false);
    pane.row(vec![
        editor,
        Span::raw(" "),
        terminal,
        Span::raw(" "),
        files,
    ]);
}

fn claude(pane: &mut Pane, app: &App) {
    pane.section("CLAUDE");
    let mut spans = vec![
        pane.control("New Session", false),
        Span::raw(" "),
        pane.control("New Session: YOLO", true),
    ];
    for button in &app.settings.custom_buttons {
        spans.push(Span::raw(" "));
        spans.push(pane.control(&button.label, button.is_yolo_style()));
    }
    pane.row(spans);
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

    let mut spans = vec![
        pane.control("Resume", false),
        Span::raw(" "),
        pane.control("YOLO", true),
    ];
    for button in &app.settings.custom_buttons {
        spans.push(Span::raw(" "));
        spans.push(pane.control(&button.label, button.is_yolo_style()));
    }
    pane.row(spans);
}

fn issues(pane: &mut Pane, app: &App) {
    // The refresh control sits on the header line and doubles as the loading
    // spinner, the same as the Textual button it replaces.
    pane.blank();
    let label = match app.issues {
        Some(_) => "↻".to_string(),
        None => SPINNER[app.spinner_index % SPINNER.len()].to_string(),
    };
    let header = Span::styled("MY OPEN GITHUB ISSUES", theme::section_header());
    let refresh = pane.control(&label, false);
    pane.row(vec![header, Span::raw(" "), refresh]);

    let Some(list) = app.issues.as_deref() else {
        pane.text("Loading...", theme::muted());
        return;
    };
    if list.is_empty() {
        pane.text("No issues found", theme::muted());
        return;
    }
    for issue in list {
        issue_item(pane, issue);
    }
}

fn issue_item(pane: &mut Pane, issue: &GitHubIssue) {
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
    let create = pane.control("Create WT", false);
    pane.row(vec![
        Span::styled(format!("{meta}  "), theme::muted()),
        create,
    ]);
}

/// A rename field. The caret is drawn in the text because the pane renders as
/// one scrolled paragraph and has no real terminal cursor to place.
fn field(pane: &mut Pane, label: &str, input: &TextInput) {
    let focused = pane.claim();
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
    let rect = centered_rect(area.width, row_index(lines.len()), area);
    frame.render_widget(Paragraph::new(lines).centered(), rect);
}

#[cfg(test)]
mod tests {
    use super::*;
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

    fn render(app: &App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(100, 40)).expect("test terminal");
        terminal
            .draw(|frame| draw(frame, app, frame.area()))
            .expect("draw");
        buffer_text(terminal.backend().buffer())
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

    #[tokio::test]
    async fn empty_state_is_visible() {
        let dir = tempfile::tempdir().expect("tempdir");
        let app = test_app(&dir);
        let screen = render(&app);

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
        let screen = render(&app);

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
        let screen = render(&app);

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
                let pane = build(&app).expect("repository pane");
                assert_eq!(
                    pane.item_rows.len(),
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
            let pane = build(&app).expect("worktree pane");
            assert_eq!(
                pane.item_rows.len(),
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

        let pane = build(&app).expect("repository pane");
        let offset = pane.scroll_offset(20);
        let row = pane.item_rows[app.detail_index];
        assert!(
            row >= offset && row < offset + 20,
            "row {row}, offset {offset}"
        );

        // The sidebar keeps the pane at the top.
        app.focus = Focus::Sidebar;
        let pane = build(&app).expect("repository pane");
        assert_eq!(pane.scroll_offset(20), 0);
    }
}
