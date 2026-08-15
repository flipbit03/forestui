//! Application state and the event loop's core: startup, the sidebar, the
//! background loads a selection kicks off, and event folding.
//!
//! The input handling lives beside it in focused modules: [`keys`] for the
//! keyboard, [`mouse`] for hit testing and scrolling, [`actions`] for what a
//! control does when activated, and [`detail`] for what the detail pane
//! contains.

mod actions;
pub mod detail;
mod keys;
mod mouse;

pub use detail::{Action, DetailItem, Field};
pub use mouse::{Hit, HitTarget, ModalClick, ScrollbarGeom};

use crate::event::{AppEvent, DetailMeta, EventTx, Severity};
use crate::modal::Modal;
use crate::models::{ClaudeSession, GitHubIssue, Settings};
use crate::services::{claude_session, git, github, settings as settings_service, tmux};
use crate::state::AppState;
use crate::ui::widgets::TextInput;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// Textual's `App.NOTIFICATION_TIMEOUT`, which every `notify()` here inherited.
pub const NOTIFICATION_TTL: Duration = Duration::from_secs(5);
pub const ISSUE_REFRESH_INTERVAL: Duration = Duration::from_secs(300);
pub const SESSION_LIMIT: usize = 5;
pub const ISSUE_LIMIT: usize = 10;
/// Lines the detail pane moves per wheel notch, matching Textual's
/// `scroll_sensitivity_y` of 3.
pub(super) const SCROLL_STEP: isize = 3;
/// Rows `PageUp` / `PageDown` move.
pub(super) const PAGE_STEP: isize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    Detail,
}

/// One visible line in the sidebar tree.
#[derive(Debug, Clone)]
pub enum SidebarRow {
    Repository {
        id: Uuid,
        name: String,
        /// Whether the row has anything to fold away, so a leaf repository does
        /// not grow a twisty that would do nothing.
        has_worktrees: bool,
        collapsed: bool,
    },
    Worktree {
        repo_id: Uuid,
        id: Uuid,
        name: String,
        branch: String,
        is_last: bool,
    },
    ArchivedHeader,
    ArchivedWorktree {
        repo_id: Uuid,
        id: Uuid,
        name: String,
        repo_name: String,
    },
}

impl SidebarRow {
    pub fn ids(&self) -> Option<(Uuid, Option<Uuid>)> {
        match self {
            SidebarRow::Repository { id, .. } => Some((*id, None)),
            SidebarRow::Worktree { repo_id, id, .. }
            | SidebarRow::ArchivedWorktree { repo_id, id, .. } => Some((*repo_id, Some(*id))),
            SidebarRow::ArchivedHeader => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Notification {
    pub text: String,
    pub severity: Severity,
    pub created: Instant,
}

pub struct App {
    pub state: AppState,
    pub settings: Settings,
    pub tx: EventTx,
    pub version: String,

    pub focus: Focus,
    pub rows: Vec<SidebarRow>,
    pub sidebar_index: usize,
    pub detail_index: usize,

    /// `None` while the background load is still running.
    pub sessions: Option<Vec<ClaudeSession>>,
    pub issues: Option<Vec<GitHubIssue>>,
    pub meta: DetailMeta,

    pub gh_status: String,
    pub modals: Vec<Modal>,
    pub notifications: Vec<Notification>,
    pub spinner_index: usize,
    pub last_issue_refresh: Instant,
    pub should_quit: bool,

    /// Clickable regions recorded by the renderers for the current frame.
    pub hits: Vec<Hit>,
    /// What the pointer is currently over, so controls can light up under it.
    pub hovered: Option<HitTarget>,
    /// Set by anything that changes what the next frame looks like. Pointer
    /// motion that does not change `hovered` leaves it alone, which is what
    /// keeps a moving mouse from repainting the app on every report.
    pub redraw: bool,

    /// Where the two panes were drawn, so the wheel can be routed to whichever
    /// one the pointer is over rather than to whichever one has focus.
    pub sidebar_rect: ratatui::layout::Rect,
    pub detail_rect: ratatui::layout::Rect,

    /// First visible line of the detail pane.
    pub detail_scroll: u16,
    /// Set when the cursor moves by keyboard, so the next frame scrolls the
    /// focused control back into view. The wheel and the scrollbar deliberately
    /// do not set it.
    pub detail_follow_focus: bool,
    /// The scrollbar as the last frame drew it, or `None` when the content fits.
    pub scrollbar: Option<ScrollbarGeom>,
    /// Rows between the top of the thumb and where the pointer grabbed it, held
    /// for the duration of a drag so the thumb does not jump under the cursor.
    pub scroll_drag: Option<u16>,
    /// Repositories folded shut in the sidebar.
    pub collapsed: std::collections::HashSet<Uuid>,

    pub name_input: TextInput,
    pub branch_input: TextInput,
    /// Worktree the rename inputs belong to, so a selection change resets them.
    pub(crate) rename_target: Option<Uuid>,
    /// Issue whose modal is waiting on a branch list to finish loading.
    pub(crate) pending_issue: Option<usize>,
}

impl App {
    pub fn new(tx: EventTx, version: String) -> Self {
        let state = AppState::load();
        let settings = settings_service::load_settings();
        let mut app = Self::with_state(tx, state, settings);
        app.version = version;
        app
    }

    /// Build an app around explicit state instead of reading disk.
    pub fn with_state(tx: EventTx, state: AppState, settings: Settings) -> Self {
        let version = crate::cli::VERSION.to_string();
        let mut app = Self {
            state,
            settings,
            tx,
            version,
            focus: Focus::Sidebar,
            rows: Vec::new(),
            sidebar_index: 0,
            detail_index: 0,
            sessions: None,
            issues: None,
            meta: DetailMeta::default(),
            gh_status: "...".to_string(),
            modals: Vec::new(),
            notifications: Vec::new(),
            spinner_index: 0,
            last_issue_refresh: Instant::now(),
            should_quit: false,
            hits: Vec::new(),
            hovered: None,
            redraw: true,
            sidebar_rect: ratatui::layout::Rect::default(),
            detail_rect: ratatui::layout::Rect::default(),
            detail_scroll: 0,
            detail_follow_focus: true,
            scrollbar: None,
            scroll_drag: None,
            collapsed: std::collections::HashSet::new(),
            name_input: TextInput::new(""),
            branch_input: TextInput::new(""),
            rename_target: None,
            pending_issue: None,
        };
        app.rebuild_rows();
        app
    }

    pub fn title(&self) -> String {
        format!("forestui v{}", self.version)
    }

    // ------------------------------------------------------------------ startup

    /// Work kicked off once the terminal is up.
    pub fn on_start(&mut self) {
        if !tmux::ensure_focus_events() {
            self.notify("Could not enable focus events", Severity::Warning);
        }
        if self.state.selection.repository_id.is_none()
            && let Some(first) = self.state.repositories().first()
        {
            let id = first.id;
            self.state.select_repository(id);
            self.rebuild_rows();
            self.sync_sidebar_index();
        }
        self.reload_detail();
        self.load_gh_status();
    }

    fn load_gh_status(&self) {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let (status, username) = github::get_auth_status().await;
            tx.send(AppEvent::GhStatus(status, username));
        });
    }

    // ------------------------------------------------------------------ sidebar

    pub fn rebuild_rows(&mut self) {
        let mut rows = Vec::new();
        for repo in self.state.repositories() {
            let active = repo.active_worktrees();
            rows.push(SidebarRow::Repository {
                id: repo.id,
                name: repo.name.clone(),
                has_worktrees: !active.is_empty(),
                collapsed: self.collapsed.contains(&repo.id),
            });
            if self.collapsed.contains(&repo.id) {
                continue;
            }
            let last_index = active.len().saturating_sub(1);
            for (index, worktree) in active.iter().enumerate() {
                rows.push(SidebarRow::Worktree {
                    repo_id: repo.id,
                    id: worktree.id,
                    name: worktree.name.clone(),
                    branch: worktree.branch.clone(),
                    is_last: index == last_index,
                });
            }
        }

        if self.state.show_archived && self.state.has_archived_worktrees() {
            rows.push(SidebarRow::ArchivedHeader);
            for repo in self.state.repositories() {
                for worktree in repo.archived_worktrees() {
                    rows.push(SidebarRow::ArchivedWorktree {
                        repo_id: repo.id,
                        id: worktree.id,
                        name: worktree.name.clone(),
                        repo_name: repo.name.clone(),
                    });
                }
            }
        }

        self.rows = rows;
        if self.sidebar_index >= self.rows.len() {
            self.sidebar_index = self.rows.len().saturating_sub(1);
        }
    }

    /// Move the sidebar cursor onto whatever is currently selected.
    fn sync_sidebar_index(&mut self) {
        let selection = self.state.selection;
        if let Some(index) = self.rows.iter().position(|row| match row.ids() {
            Some((repo_id, worktree_id)) => {
                selection.repository_id == Some(repo_id) && selection.worktree_id == worktree_id
            }
            None => false,
        }) {
            self.sidebar_index = index;
        }
    }

    fn move_sidebar(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let len = self.rows.len() as isize;
        let mut next = self.sidebar_index as isize + delta;
        next = next.clamp(0, len - 1);
        if next == self.sidebar_index as isize {
            return;
        }
        self.sidebar_index = next as usize;
        self.select_current_row();
    }

    /// Selecting follows the cursor, matching the Textual tree's highlight
    /// behaviour where moving the cursor loads the detail pane.
    fn select_current_row(&mut self) {
        let Some(row) = self.rows.get(self.sidebar_index).cloned() else {
            return;
        };
        match row.ids() {
            Some((repo_id, Some(worktree_id))) => self.state.select_worktree(repo_id, worktree_id),
            Some((repo_id, None)) => self.state.select_repository(repo_id),
            None => return,
        }
        self.reload_detail();
    }

    // ------------------------------------------------------------------- detail

    /// The list of focusable items currently rendered in the detail pane.
    ///
    /// Derived from the same [`detail::content`] walk the renderer draws, so
    /// item N here is control N on screen by construction.
    pub fn detail_items(&self) -> Vec<DetailItem> {
        detail::items(&detail::content(self))
    }

    /// Reload everything the detail pane shows for the current selection.
    pub fn reload_detail(&mut self) {
        self.detail_index = 0;
        // A new selection starts at the top; keeping the old offset would open
        // the pane part-way down a different repository's content.
        self.detail_scroll = 0;
        self.detail_follow_focus = true;
        self.sessions = None;
        self.issues = None;

        let Some(path) = self.state.selected_path() else {
            self.meta = DetailMeta::default();
            return;
        };
        let is_repository = self.state.selection.is_repository();

        // Reset the rename fields whenever the selected worktree changes.
        if let Some((_repo, worktree)) = self.state.selected_worktree() {
            if self.rename_target != Some(worktree.id) {
                self.rename_target = Some(worktree.id);
                self.name_input = TextInput::new(worktree.name.clone());
                self.branch_input = TextInput::new(worktree.branch.clone());
            }
        } else {
            self.rename_target = None;
        }

        self.meta = DetailMeta {
            path: path.clone(),
            path_exists: crate::util::path_exists(&path),
            ..DetailMeta::default()
        };

        let tx = self.tx.clone();
        let meta_path = path.clone();
        tokio::spawn(async move {
            let mut meta = DetailMeta {
                path: meta_path.clone(),
                path_exists: crate::util::path_exists(&meta_path),
                ..DetailMeta::default()
            };
            if is_repository {
                meta.branch = git::get_current_branch(&meta_path).await.ok();
            }
            if let Ok(commit) = git::get_latest_commit(&meta_path).await {
                meta.commit_hash = Some(commit.short_hash);
                meta.commit_time = Some(commit.timestamp);
                meta.has_remote = git::has_remote_tracking(&meta_path).await.unwrap_or(false);
            }
            tx.send(AppEvent::Meta(Box::new(meta)));
        });

        let tx = self.tx.clone();
        let session_path = path.clone();
        tokio::spawn(async move {
            let sessions = tokio::task::spawn_blocking(move || {
                claude_session::get_sessions_for_path(&session_path, SESSION_LIMIT)
            })
            .await
            .unwrap_or_default();
            tx.send(AppEvent::Sessions {
                path: path.clone(),
                sessions,
            });
        });

        if is_repository {
            self.fetch_issues();
        }
    }

    fn fetch_issues(&mut self) {
        let Some(repo) = self.state.selected_repository() else {
            return;
        };
        let path = repo.source_path.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let issues = github::list_issues(&path, ISSUE_LIMIT).await;
            tx.send(AppEvent::Issues { path, issues });
        });
    }

    // ------------------------------------------------------------- notifications

    pub fn notify(&mut self, text: impl Into<String>, severity: Severity) {
        self.notifications.push(Notification {
            text: text.into(),
            severity,
            created: Instant::now(),
        });
        if self.notifications.len() > 4 {
            self.notifications.remove(0);
        }
    }

    fn expire_notifications(&mut self) {
        self.notifications
            .retain(|n| n.created.elapsed() < NOTIFICATION_TTL);
    }

    /// The 100ms heartbeat. It earns a repaint only when something on screen
    /// actually moved — a spinner frame, a toast expiring — so an idle app
    /// paints nothing at all rather than ten frames a second.
    fn on_tick(&mut self) {
        self.spinner_index = self.spinner_index.wrapping_add(1);
        if self.issue_spinner_visible() {
            self.redraw = true;
        }

        let before = self.notifications.len();
        self.expire_notifications();
        if self.notifications.len() != before {
            self.redraw = true;
        }

        if let Some(modal) = self.modals.last_mut()
            && modal.tick()
        {
            self.redraw = true;
        }

        if self.last_issue_refresh.elapsed() >= ISSUE_REFRESH_INTERVAL {
            self.last_issue_refresh = Instant::now();
            github::invalidate_cache(None);
            if self.state.selection.is_repository() {
                // No repaint yet: nothing changes on screen until the result
                // arrives, and that event pays for its own.
                self.fetch_issues();
            }
        }
    }

    /// Whether the issue-refresh spinner — the only consumer of
    /// `spinner_index` — is on screen this frame.
    fn issue_spinner_visible(&self) -> bool {
        self.modals.is_empty() && self.state.selection.is_repository() && self.issues.is_none()
    }

    // ------------------------------------------------------------------- events

    pub fn handle_event(&mut self, event: AppEvent) {
        // Everything except a bare pointer move or a tick changes the frame.
        // Motion is asked about rather than cleared afterwards so that a batch
        // of [Notify, Moved] still repaints: a no-op move must never cancel a
        // repaint some other event in the same drain already earned. The tick
        // decides for itself in `on_tick`: at ten a second, repainting an idle
        // frame on every one keeps the terminal permanently busy for nothing.
        if !mouse::is_pointer_motion(&event) && !matches!(event, AppEvent::Tick) {
            self.redraw = true;
        }
        match event {
            AppEvent::Term(term_event) => self.handle_term_event(term_event),
            AppEvent::Tick => self.on_tick(),
            AppEvent::GhStatus(status, username) => {
                self.gh_status = status.display(username.as_deref());
            }
            AppEvent::Sessions { path, sessions } => {
                if path == self.meta.path {
                    self.sessions = Some(sessions);
                }
            }
            AppEvent::Issues { path, issues } => {
                if Some(path.as_str())
                    == self
                        .state
                        .selected_repository()
                        .map(|r| r.source_path.as_str())
                {
                    self.issues = Some(issues);
                }
            }
            AppEvent::Meta(meta) => {
                if meta.path == self.meta.path {
                    self.meta = *meta;
                }
            }
            AppEvent::Branches {
                repo_id,
                branches,
                remotes,
                current_branch,
                target,
            } => self.on_branches(repo_id, branches, remotes, current_branch, target),
            AppEvent::FetchFailed(error) => {
                if let Some(Modal::CreateFromIssue(modal)) = self.modals.last_mut() {
                    modal.fetch_failed();
                }
                self.notify(format!("Fetch failed: {error}"), Severity::Error);
            }
            AppEvent::Notify(text, severity) => self.notify(text, severity),
            AppEvent::WorktreeAdded { repo_id, worktree } => {
                // The repository can be removed while git runs; dropping the
                // result beats resurrecting an entry nothing owns.
                if self.state.find_repository(repo_id).is_none() {
                    return;
                }
                let worktree_id = worktree.id;
                self.state.add_worktree(repo_id, *worktree);
                self.state.select_worktree(repo_id, worktree_id);
                self.rebuild_rows();
                self.sync_sidebar_index();
                self.reload_detail();
            }
            AppEvent::WorktreesImported { repo_id, worktrees } => {
                if self.state.find_repository(repo_id).is_none() {
                    return;
                }
                self.state.add_worktrees(repo_id, worktrees);
                self.state.select_repository(repo_id);
                self.rebuild_rows();
                self.sync_sidebar_index();
                self.reload_detail();
            }
            AppEvent::ReloadDetail => self.reload_detail(),
        }
    }

    fn handle_term_event(&mut self, event: ratatui::crossterm::event::Event) {
        use ratatui::crossterm::event::{Event, KeyEventKind};
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => self.handle_key(key),
            Event::Mouse(mouse) => self.handle_mouse(mouse),
            // tmux focus-events let the app refresh when the user comes back.
            Event::FocusGained => self.reload_detail(),
            _ => {}
        }
    }

    /// Handle everything already queued, and report how many events that was.
    ///
    /// The loop draws once per iteration, so without this a burst — a tick plus
    /// a click's press and release, or several background results landing
    /// together — repaints the screen once per event. That reads as the whole
    /// app flickering for a single user action.
    pub fn drain(&mut self, rx: &mut tokio::sync::mpsc::UnboundedReceiver<AppEvent>) -> usize {
        let mut handled = 0;
        while let Ok(queued) = rx.try_recv() {
            self.handle_event(queued);
            handled += 1;
            if self.should_quit {
                break;
            }
        }
        handled
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use crate::models::{Repository, Selection, Worktree};

    /// An app backed by a throwaway forest directory, with one repository that
    /// has two active worktrees and one archived worktree.
    pub fn app_with_fixture() -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut state = AppState::load_from(dir.path().join(".forestui-config.json"));

        let repo = Repository::new(
            "demo".into(),
            dir.path().join("demo").to_string_lossy().to_string(),
        );
        let repo_id = repo.id;
        state.add_repository(repo);

        for name in ["alpha", "beta"] {
            let worktree = Worktree::new(
                name.into(),
                format!("feat/{name}"),
                dir.path()
                    .join("forest")
                    .join(name)
                    .to_string_lossy()
                    .to_string(),
            );
            state.add_worktree(repo_id, worktree);
        }
        let archived = Worktree::new(
            "old".into(),
            "feat/old".into(),
            dir.path()
                .join("forest")
                .join("old")
                .to_string_lossy()
                .to_string(),
        );
        let archived_id = archived.id;
        state.add_worktree(repo_id, archived);
        state.set_archived(archived_id, true);
        state.selection = Selection {
            repository_id: Some(repo_id),
            worktree_id: None,
        };

        let (tx, _rx) = crate::event::start();
        let app = App::with_state(tx, state, Settings::default());
        (dir, app)
    }

    pub fn key(code: ratatui::crossterm::event::KeyCode) -> ratatui::crossterm::event::KeyEvent {
        use ratatui::crossterm::event::{KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{app_with_fixture, key};
    use super::*;
    use crate::models::{CustomClaudeButton, Repository, Worktree};
    use ratatui::crossterm::event::KeyCode;
    use std::path::PathBuf;

    #[tokio::test]
    async fn sidebar_lists_repository_then_active_worktrees() {
        let (_dir, mut app) = app_with_fixture();
        app.rebuild_rows();

        assert_eq!(app.rows.len(), 3, "one repo + two active worktrees");
        assert!(matches!(app.rows[0], SidebarRow::Repository { .. }));
        assert!(matches!(app.rows[1], SidebarRow::Worktree { .. }));

        // The archived section only appears once it is toggled on.
        app.handle_key(key(KeyCode::Char('A')));
        assert_eq!(app.rows.len(), 5, "plus the archived header and entry");
        assert!(matches!(app.rows[3], SidebarRow::ArchivedHeader));
        assert!(matches!(app.rows[4], SidebarRow::ArchivedWorktree { .. }));
    }

    #[tokio::test]
    async fn moving_the_sidebar_cursor_changes_the_selection() {
        let (_dir, mut app) = app_with_fixture();
        assert!(app.state.selection.is_repository());

        app.handle_key(key(KeyCode::Down));
        assert!(app.state.selection.is_worktree());
        assert_eq!(app.sidebar_index, 1);

        app.handle_key(key(KeyCode::Up));
        assert!(app.state.selection.is_repository());
    }

    #[tokio::test]
    async fn detail_items_differ_between_repository_and_worktree() {
        let (_dir, mut app) = app_with_fixture();
        app.sessions = Some(Vec::new());
        app.issues = Some(Vec::new());

        let repo_items = app.detail_items();
        assert!(repo_items.contains(&DetailItem::Action(Action::AddWorktree)));
        assert!(repo_items.contains(&DetailItem::Action(Action::RemoveRepository)));
        assert!(!repo_items.contains(&DetailItem::Field(Field::WorktreeName)));

        app.handle_key(key(KeyCode::Down));
        app.sessions = Some(Vec::new());
        let worktree_items = app.detail_items();
        assert!(worktree_items.contains(&DetailItem::Field(Field::WorktreeName)));
        assert!(worktree_items.contains(&DetailItem::Field(Field::BranchName)));
        assert!(worktree_items.contains(&DetailItem::Action(Action::Archive)));
        assert!(!worktree_items.contains(&DetailItem::Action(Action::AddWorktree)));
    }

    #[tokio::test]
    async fn custom_buttons_add_one_control_per_session_and_one_at_the_top() {
        let (_dir, mut app) = app_with_fixture();
        app.issues = Some(Vec::new());
        app.sessions = Some(Vec::new());
        let baseline = app.detail_items().len();

        app.settings.custom_buttons.push(CustomClaudeButton {
            label: "Opus".into(),
            prefix: "opus".into(),
            command: "claude --model opus".into(),
        });
        assert_eq!(app.detail_items().len(), baseline + 1);

        app.sessions = Some(vec![ClaudeSession {
            id: "s1".into(),
            title: "t".into(),
            last_message: String::new(),
            last_timestamp: chrono::Utc::now(),
            message_count: 1,
        }]);
        // Resume + YOLO + one per custom button.
        assert_eq!(app.detail_items().len(), baseline + 1 + 3);
    }

    #[tokio::test]
    async fn typing_in_a_rename_field_does_not_trigger_hotkeys() {
        let (_dir, mut app) = app_with_fixture();
        app.handle_key(key(KeyCode::Down));
        app.sessions = Some(Vec::new());

        app.focus = Focus::Detail;
        let index = app
            .detail_items()
            .iter()
            .position(|item| *item == DetailItem::Field(Field::WorktreeName))
            .expect("rename field present");
        app.detail_index = index;
        let original = app.name_input.value().to_string();

        // 'q' would otherwise quit.
        app.handle_key(key(KeyCode::Char('q')));
        assert!(!app.should_quit);
        assert!(app.name_input.value().ends_with('q'));

        // Escape undoes the edit and hands focus back, so hotkeys work again.
        app.handle_key(key(KeyCode::Esc));
        assert_eq!(app.name_input.value(), original);
        assert_eq!(app.focus, Focus::Sidebar);
        app.handle_key(key(KeyCode::Char('q')));
        assert!(app.should_quit);
    }

    #[tokio::test]
    async fn quit_and_help_hotkeys() {
        let (_dir, mut app) = app_with_fixture();
        app.handle_key(key(KeyCode::Char('?')));
        assert_eq!(app.notifications.len(), 1);

        app.handle_key(key(KeyCode::Char('q')));
        assert!(app.should_quit);
    }

    #[tokio::test]
    async fn opening_a_modal_captures_keys() {
        let (_dir, mut app) = app_with_fixture();
        app.handle_key(key(KeyCode::Char('a')));
        assert_eq!(app.modals.len(), 1);

        // 'q' now types into the modal instead of quitting.
        app.handle_key(key(KeyCode::Char('q')));
        assert!(!app.should_quit);

        app.handle_key(key(KeyCode::Esc));
        assert!(app.modals.is_empty());
    }

    #[tokio::test]
    async fn delete_asks_for_confirmation_first() {
        let (_dir, mut app) = app_with_fixture();
        app.handle_key(key(KeyCode::Down));
        let worktree_id = app.state.selection.worktree_id.expect("worktree selected");

        app.handle_key(key(KeyCode::Char('d')));
        assert!(matches!(app.modals.last(), Some(Modal::Confirm(_))));
        // Still there: nothing is destroyed before the user says yes.
        assert!(app.state.find_worktree(worktree_id).is_some());

        app.handle_key(key(KeyCode::Char('y')));
        assert!(app.modals.is_empty());
        assert!(app.state.find_worktree(worktree_id).is_none());
    }

    #[tokio::test]
    async fn archive_toggle_moves_the_worktree_out_of_the_active_list() {
        let (_dir, mut app) = app_with_fixture();
        app.handle_key(key(KeyCode::Down));
        let worktree_id = app.state.selection.worktree_id.expect("worktree selected");

        app.handle_key(key(KeyCode::Char('h')));
        let (_repo, worktree) = app.state.find_worktree(worktree_id).expect("still tracked");
        assert!(worktree.is_archived);
        assert_eq!(
            app.rows.len(),
            2,
            "one repo + one remaining active worktree"
        );
    }

    /// A finished background creation lands in state on the main loop — the
    /// single writer — and moves the selection onto the new worktree.
    #[tokio::test]
    async fn a_created_worktree_is_folded_into_state_and_selected() {
        let (_dir, mut app) = app_with_fixture();
        let repo_id = app.state.repositories()[0].id;
        let worktree = Worktree::new("fresh".into(), "feat/fresh".into(), "/tmp/fresh".into());
        let worktree_id = worktree.id;

        app.handle_event(AppEvent::WorktreeAdded {
            repo_id,
            worktree: Box::new(worktree),
        });

        assert!(app.state.find_worktree(worktree_id).is_some());
        assert_eq!(app.state.selection.worktree_id, Some(worktree_id));
        assert!(
            app.rows
                .iter()
                .any(|row| matches!(row, SidebarRow::Worktree { id, .. } if *id == worktree_id)),
            "the sidebar was not rebuilt"
        );
    }

    /// If the repository is removed while git runs, the late result is dropped
    /// rather than resurrected as an entry nothing owns.
    #[tokio::test]
    async fn a_worktree_for_a_removed_repository_is_dropped() {
        let (_dir, mut app) = app_with_fixture();
        let repo_id = app.state.repositories()[0].id;
        app.state.remove_repository(repo_id);
        let worktree = Worktree::new("late".into(), "feat/late".into(), "/tmp/late".into());
        let worktree_id = worktree.id;

        app.handle_event(AppEvent::WorktreeAdded {
            repo_id,
            worktree: Box::new(worktree),
        });

        assert!(app.state.find_worktree(worktree_id).is_none());
    }

    /// An import scan delivers its batch as one event, folded in with one save,
    /// and leaves the repository selected.
    #[tokio::test]
    async fn imported_worktrees_are_folded_into_state() {
        let (_dir, mut app) = app_with_fixture();
        let repo_id = app.state.repositories()[0].id;
        let before = app.state.repositories()[0].worktrees.len();

        app.handle_event(AppEvent::WorktreesImported {
            repo_id,
            worktrees: vec![
                Worktree::new("i1".into(), "main".into(), "/tmp/i1".into()),
                Worktree::new("i2".into(), "main".into(), "/tmp/i2".into()),
            ],
        });

        assert_eq!(app.state.repositories()[0].worktrees.len(), before + 2);
        assert_eq!(app.state.selection.repository_id, Some(repo_id));
        assert_eq!(app.state.selection.worktree_id, None);
    }

    #[tokio::test]
    async fn stale_results_for_a_previous_selection_are_ignored() {
        let (_dir, mut app) = app_with_fixture();
        app.meta.path = "/current".into();

        app.handle_event(AppEvent::Sessions {
            path: "/stale".into(),
            sessions: vec![],
        });
        assert!(app.sessions.is_none(), "a late result must not land");

        app.handle_event(AppEvent::Sessions {
            path: "/current".into(),
            sessions: vec![],
        });
        assert!(app.sessions.is_some());
    }

    fn click(column: u16, row: u16) -> ratatui::crossterm::event::MouseEvent {
        use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn rect(x: u16, y: u16, width: u16, height: u16) -> ratatui::layout::Rect {
        ratatui::layout::Rect {
            x,
            y,
            width,
            height,
        }
    }

    #[tokio::test]
    async fn hit_at_prefers_the_last_recorded_region() {
        let (_dir, mut app) = app_with_fixture();
        app.push_hit(rect(0, 0, 10, 1), HitTarget::SidebarRow(3));
        // A modal drawn afterwards covers the same cell and must win.
        app.push_hit(
            rect(0, 0, 10, 1),
            HitTarget::ModalControl {
                index: 1,
                click: ModalClick::Activate,
            },
        );

        assert_eq!(
            app.hit_at(5, 0),
            Some(HitTarget::ModalControl {
                index: 1,
                click: ModalClick::Activate
            })
        );
        assert_eq!(app.hit_at(50, 0), None, "outside every region");
        assert_eq!(app.hit_at(5, 9), None, "wrong row");
    }

    #[tokio::test]
    async fn clicking_a_sidebar_row_selects_it() {
        let (_dir, mut app) = app_with_fixture();
        assert!(app.state.selection.is_repository());

        app.push_hit(rect(0, 1, 30, 1), HitTarget::SidebarRow(1));
        app.handle_mouse(click(4, 1));

        assert_eq!(app.sidebar_index, 1);
        assert!(app.state.selection.is_worktree());
        assert_eq!(app.focus, Focus::Sidebar);
    }

    #[tokio::test]
    async fn clicking_a_detail_control_focuses_and_runs_it() {
        let (_dir, mut app) = app_with_fixture();
        app.sessions = Some(Vec::new());
        app.issues = Some(Vec::new());

        let index = app
            .detail_items()
            .iter()
            .position(|item| *item == DetailItem::Action(Action::Terminal))
            .expect("terminal control present");
        app.push_hit(rect(40, 5, 12, 1), HitTarget::DetailItem(index));
        app.handle_mouse(click(45, 5));

        assert_eq!(app.focus, Focus::Detail);
        assert_eq!(app.detail_index, index);
        // The fixture's directories do not exist, so the action refuses loudly
        // rather than silently opening a window somewhere else.
        assert_eq!(app.notifications.len(), 1);
        assert!(app.notifications[0].text.contains("no longer exists"));
    }

    #[tokio::test]
    async fn clicking_a_detail_field_only_moves_focus() {
        let (_dir, mut app) = app_with_fixture();
        app.handle_key(key(KeyCode::Down));
        app.sessions = Some(Vec::new());

        let index = app
            .detail_items()
            .iter()
            .position(|item| *item == DetailItem::Field(Field::BranchName))
            .expect("rename field present");
        app.push_hit(rect(40, 9, 20, 1), HitTarget::DetailItem(index));
        app.handle_mouse(click(42, 9));

        assert_eq!(app.detail_index, index);
        assert!(app.notifications.is_empty(), "a field is not an action");
    }

    #[tokio::test]
    async fn a_modal_swallows_clicks_on_the_panes_behind_it() {
        let (_dir, mut app) = app_with_fixture();
        app.handle_key(key(KeyCode::Char('a')));
        assert_eq!(app.modals.len(), 1);

        let before = app.sidebar_index;
        app.push_hit(rect(0, 1, 30, 1), HitTarget::SidebarRow(1));
        app.handle_mouse(click(4, 1));

        assert_eq!(app.sidebar_index, before, "the modal keeps the click");
        assert_eq!(app.modals.len(), 1);
    }

    #[tokio::test]
    async fn clicking_confirm_delete_runs_the_deletion() {
        let (_dir, mut app) = app_with_fixture();
        app.handle_key(key(KeyCode::Down));
        let worktree_id = app.state.selection.worktree_id.expect("worktree selected");
        app.handle_key(key(KeyCode::Char('d')));
        assert!(matches!(app.modals.last(), Some(Modal::Confirm(_))));

        // Index 1 is Delete; index 0 would be Cancel.
        app.push_hit(
            rect(10, 10, 10, 1),
            HitTarget::ModalControl {
                index: 1,
                click: ModalClick::Activate,
            },
        );
        app.handle_mouse(click(12, 10));

        assert!(app.modals.is_empty());
        assert!(app.state.find_worktree(worktree_id).is_none());
    }

    #[tokio::test]
    async fn clicking_a_field_or_cycle_does_not_submit_the_modal() {
        let (_dir, mut app) = app_with_fixture();
        app.handle_key(key(KeyCode::Char('s')));
        assert_eq!(app.modals.len(), 1);

        // Focus index 1 is the branch-prefix input. Clicking it must focus, not save.
        app.push_hit(
            rect(0, 0, 10, 3),
            HitTarget::ModalControl {
                index: 1,
                click: ModalClick::Focus,
            },
        );
        app.handle_mouse(click(2, 1));
        assert_eq!(app.modals.len(), 1, "clicking a field closed the dialog");

        // Index 0 is the editor cycle: clicking advances it, still without saving.
        let before = match app.modals.last() {
            Some(Modal::Settings(m)) => m.editor_index,
            _ => panic!("settings modal expected"),
        };
        app.push_hit(
            rect(0, 5, 20, 1),
            HitTarget::ModalControl {
                index: 0,
                click: ModalClick::Cycle,
            },
        );
        app.handle_mouse(click(3, 5));
        assert_eq!(app.modals.len(), 1, "clicking a cycle closed the dialog");
        match app.modals.last() {
            Some(Modal::Settings(m)) => assert_ne!(m.editor_index, before, "cycle did not advance"),
            _ => panic!("settings modal expected"),
        }
    }

    #[tokio::test]
    async fn clicking_a_branch_row_selects_that_row() {
        use crate::modal::AddWorktreeModal;
        let (_dir, mut app) = app_with_fixture();
        let repo = app.state.repositories()[0].clone();
        let branches = vec!["main".into(), "release-2".into(), "feat/other".into()];
        let mut modal = AddWorktreeModal::new(
            &repo,
            branches,
            vec![],
            PathBuf::from("/forest"),
            "feat/".into(),
        );
        modal.new_branch = false;
        app.modals.push(Modal::AddWorktree(Box::new(modal)));

        app.push_hit(
            rect(0, 7, 40, 1),
            HitTarget::ModalControl {
                index: 3,
                click: ModalClick::Row(2),
            },
        );
        app.handle_mouse(click(5, 7));

        match app.modals.last() {
            Some(Modal::AddWorktree(m)) => {
                assert_eq!(m.search_index, 2, "the clicked row was not selected");
                assert_eq!(m.focus, 3, "focus did not move to the results list");
            }
            _ => panic!("add-worktree modal expected"),
        }
    }

    #[tokio::test]
    async fn a_burst_of_events_is_handled_before_one_repaint() {
        // The draw loop redraws once per iteration, so everything already
        // queued has to be consumed in a single pass or a click repaints the
        // screen several times over — which is what flickering looked like.
        let dir = tempfile::tempdir().expect("tempdir");
        let mut state = AppState::load_from(dir.path().join(".forestui-config.json"));
        state.add_repository(Repository::new("demo".into(), "/tmp/demo".into()));

        let (tx, mut rx) = crate::event::start();
        let mut app = App::with_state(tx.clone(), state, Settings::default());

        for _ in 0..5 {
            tx.info("burst");
        }
        // Let the sends land in the channel before draining.
        tokio::time::sleep(Duration::from_millis(50)).await;

        let handled = app.drain(&mut rx);
        assert!(handled >= 5, "drained only {handled} events");
        assert!(rx.try_recv().is_err(), "events left queued after draining");
    }

    #[tokio::test]
    async fn pointer_motion_is_ignored() {
        use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let (_dir, mut app) = app_with_fixture();
        app.push_hit(rect(0, 1, 30, 1), HitTarget::SidebarRow(1));

        let at = |kind| MouseEvent {
            kind,
            column: 4,
            row: 1,
            modifiers: KeyModifiers::NONE,
        };
        // Motion and release must not act; only the press does.
        app.handle_mouse(at(MouseEventKind::Moved));
        app.handle_mouse(at(MouseEventKind::Drag(MouseButton::Left)));
        app.handle_mouse(at(MouseEventKind::Up(MouseButton::Left)));
        assert_eq!(app.sidebar_index, 0, "motion changed the selection");

        app.handle_mouse(at(MouseEventKind::Down(MouseButton::Left)));
        assert_eq!(app.sidebar_index, 1);
    }

    /// The wheel follows the pointer, not the keyboard focus. Scrolling the
    /// sidebar because the mouse happened to be over the detail pane was the
    /// reported bug.
    #[tokio::test]
    async fn scroll_wheel_follows_the_pointer_not_the_focus() {
        use ratatui::crossterm::event::{KeyModifiers, MouseEvent, MouseEventKind};
        let (_dir, mut app) = app_with_fixture();
        app.sidebar_rect = rect(0, 0, 35, 40);
        app.detail_rect = rect(35, 0, 115, 40);
        let wheel = |kind, column| MouseEvent {
            kind,
            column,
            row: 10,
            modifiers: KeyModifiers::NONE,
        };

        // Over the sidebar: the sidebar moves.
        app.handle_mouse(wheel(MouseEventKind::ScrollDown, 5));
        assert_eq!(app.sidebar_index, 1);
        app.handle_mouse(wheel(MouseEventKind::ScrollUp, 5));
        assert_eq!(app.sidebar_index, 0);

        // Over the detail pane, while the sidebar still has focus: the detail
        // pane scrolls and the selection is left alone.
        assert_eq!(app.focus, Focus::Sidebar);
        app.handle_mouse(wheel(MouseEventKind::ScrollDown, 90));
        assert_eq!(app.sidebar_index, 0, "the wheel moved the wrong pane");
        assert_eq!(app.detail_scroll, SCROLL_STEP as u16);

        // And it never scrolls above the top.
        app.handle_mouse(wheel(MouseEventKind::ScrollUp, 90));
        app.handle_mouse(wheel(MouseEventKind::ScrollUp, 90));
        assert_eq!(app.detail_scroll, 0);
    }

    /// Dragging the scrollbar thumb scrolls the pane, and picking the thumb up
    /// part-way down must not snap it to the pointer.
    #[tokio::test]
    async fn dragging_the_scrollbar_thumb_scrolls_the_pane() {
        use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let (_dir, mut app) = app_with_fixture();
        // A 20-row track over 120 lines of content: thumb 3 rows, travel 17,
        // max offset 100.
        let geom = ScrollbarGeom::new(rect(148, 2, 2, 20), 120).expect("content overflows");
        assert_eq!((geom.thumb, geom.travel, geom.max_offset), (3, 17, 100));
        app.scrollbar = Some(geom);
        app.detail_scroll = 0;

        let at = |kind, row| MouseEvent {
            kind,
            column: 148,
            row,
            modifiers: KeyModifiers::NONE,
        };

        // Grab the thumb one row below its top; the offset must not move yet.
        app.handle_mouse(at(MouseEventKind::Down(MouseButton::Left), 3));
        assert_eq!(app.scroll_drag, Some(1));
        assert_eq!(app.detail_scroll, 0, "grabbing the thumb moved the pane");

        // Drag down ten rows: the thumb top follows the grab point, not the
        // pointer, so it lands at row 10 of the track.
        app.handle_mouse(at(MouseEventKind::Drag(MouseButton::Left), 13));
        assert_eq!(app.detail_scroll, geom.offset_at(10));

        // Past the bottom of the track it pins to the end rather than running on.
        app.handle_mouse(at(MouseEventKind::Drag(MouseButton::Left), 200));
        assert_eq!(app.detail_scroll, geom.max_offset);

        // Above the top it pins to the start.
        app.handle_mouse(at(MouseEventKind::Drag(MouseButton::Left), 0));
        assert_eq!(app.detail_scroll, 0);

        // The release applies its own position before ending the gesture, so a
        // flick that ends past the last drag report is not thrown away.
        app.handle_mouse(at(MouseEventKind::Up(MouseButton::Left), 9));
        assert_eq!(app.detail_scroll, geom.offset_at(9 - 2 - 1));
        assert_eq!(app.scroll_drag, None);
        app.detail_scroll = 0;
        app.handle_mouse(at(MouseEventKind::Drag(MouseButton::Left), 18));
        assert_eq!(app.detail_scroll, 0, "the pane scrolled after the release");
    }

    /// Pressing the bare track pages to the pointer instead of doing nothing.
    #[tokio::test]
    async fn pressing_the_scrollbar_track_pages_to_the_pointer() {
        use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let (_dir, mut app) = app_with_fixture();
        let geom = ScrollbarGeom::new(rect(148, 2, 2, 20), 120).expect("content overflows");
        app.scrollbar = Some(geom);
        app.detail_scroll = 0;
        let focus_before = app.focus;

        // Row 14 of the track, well below the resting thumb.
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 148,
            row: 16,
            modifiers: KeyModifiers::NONE,
        });

        assert_eq!(app.detail_scroll, geom.offset_at(14 - geom.thumb / 2));
        assert!(app.scroll_drag.is_some(), "the press did not begin a drag");
        assert_eq!(app.focus, focus_before, "the scrollbar stole focus");
    }

    /// A drag that began somewhere else must not move the pane.
    #[tokio::test]
    async fn a_drag_outside_the_scrollbar_does_not_scroll() {
        use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let (_dir, mut app) = app_with_fixture();
        app.scrollbar = Some(ScrollbarGeom::new(rect(148, 2, 2, 20), 120).expect("overflows"));
        app.detail_scroll = 7;

        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: 40,
            row: 10,
            modifiers: KeyModifiers::NONE,
        });

        assert_eq!(app.detail_scroll, 7);
    }

    /// The thumb the renderer draws and the offset a drag computes have to be
    /// inverses, or the bar drifts under the pointer over a long drag.
    #[tokio::test]
    async fn thumb_position_and_offset_round_trip() {
        let geom = ScrollbarGeom::new(rect(0, 0, 2, 20), 120).expect("content overflows");
        for top in 0..=geom.travel {
            let offset = geom.offset_at(top);
            assert_eq!(
                geom.thumb_top(offset),
                top,
                "thumb top {top} did not survive"
            );
        }
    }

    /// Hover is what makes a control look clickable; it also must not repaint
    /// the app for every pointer report, which is why motion is tracked at all.
    #[tokio::test]
    async fn hover_tracks_the_pointer_and_only_repaints_on_a_change() {
        use ratatui::crossterm::event::{KeyModifiers, MouseEvent, MouseEventKind};
        let (_dir, mut app) = app_with_fixture();
        app.push_hit(rect(0, 0, 10, 1), HitTarget::SidebarRow(0));
        let moved = |column, row| MouseEvent {
            kind: MouseEventKind::Moved,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        };

        app.redraw = false;
        app.handle_mouse(moved(2, 0));
        assert_eq!(app.hovered, Some(HitTarget::SidebarRow(0)));
        assert!(app.redraw, "entering a control has to repaint it");

        // Moving within the same control changes nothing on screen.
        app.redraw = false;
        app.handle_mouse(moved(4, 0));
        assert!(!app.redraw, "a no-op move repainted the app");

        app.redraw = false;
        app.handle_mouse(moved(50, 30));
        assert_eq!(app.hovered, None);
        assert!(app.redraw, "leaving a control has to repaint it");
    }

    /// A no-op pointer move must not cancel a repaint another event in the same
    /// drain already earned.
    #[tokio::test]
    async fn a_no_op_move_does_not_swallow_another_events_repaint() {
        use ratatui::crossterm::event::{Event, KeyModifiers, MouseEvent, MouseEventKind};
        let (_dir, mut app) = app_with_fixture();
        app.redraw = false;

        app.handle_event(AppEvent::Notify("hi".into(), Severity::Information));
        app.handle_event(AppEvent::Term(Event::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: 200,
            row: 200,
            modifiers: KeyModifiers::NONE,
        })));

        assert!(app.redraw, "the notify's repaint was swallowed by the move");
    }

    /// Ticks arrive ten times a second; an idle frame must not repaint on them,
    /// or the app keeps the terminal permanently busy doing nothing.
    #[tokio::test]
    async fn an_idle_tick_does_not_repaint() {
        let (_dir, mut app) = app_with_fixture();
        // A worktree selection has no spinner on screen.
        app.handle_key(key(KeyCode::Down));
        app.sessions = Some(Vec::new());
        app.redraw = false;

        app.handle_event(AppEvent::Tick);
        assert!(!app.redraw, "an idle tick repainted the app");
    }

    /// While the issue spinner is on screen the tick is what animates it.
    #[tokio::test]
    async fn a_tick_repaints_while_the_issue_spinner_is_visible() {
        let (_dir, mut app) = app_with_fixture();
        assert!(app.state.selection.is_repository());
        app.issues = None;
        app.redraw = false;

        app.handle_event(AppEvent::Tick);
        assert!(app.redraw, "the spinner froze");

        // Once the issues have landed there is nothing animating.
        app.issues = Some(Vec::new());
        app.sessions = Some(Vec::new());
        app.notifications.clear();
        app.redraw = false;
        app.handle_event(AppEvent::Tick);
        assert!(!app.redraw, "a loaded pane repainted on the tick");
    }

    /// A toast leaving the screen is a frame change, and the tick that expires
    /// it has to pay for the repaint.
    #[tokio::test]
    async fn a_tick_repaints_when_a_notification_expires() {
        let (_dir, mut app) = app_with_fixture();
        app.handle_key(key(KeyCode::Down));
        app.sessions = Some(Vec::new());
        app.notifications.push(Notification {
            text: "old".into(),
            severity: Severity::Information,
            created: Instant::now() - NOTIFICATION_TTL - Duration::from_secs(1),
        });
        app.redraw = false;

        app.handle_event(AppEvent::Tick);
        assert!(app.notifications.is_empty());
        assert!(app.redraw, "the expired toast was left on screen");
    }

    /// Folding a repository hides its worktrees without moving the selection.
    #[tokio::test]
    async fn collapsing_a_repository_hides_its_worktrees() {
        let (_dir, mut app) = app_with_fixture();
        let before = app.rows.len();
        assert!(
            app.rows
                .iter()
                .any(|row| matches!(row, SidebarRow::Worktree { .. })),
            "fixture has no worktree to fold away"
        );
        let selected = app.state.selection;

        app.toggle_collapsed(0);
        assert!(
            !app.rows
                .iter()
                .any(|row| matches!(row, SidebarRow::Worktree { .. }))
        );
        assert_eq!(
            app.state.selection, selected,
            "folding changed the selection"
        );

        app.toggle_collapsed(0);
        assert_eq!(app.rows.len(), before);
    }

    /// Clicking a footer entry is the same as pressing its key.
    #[tokio::test]
    async fn clicking_the_footer_runs_the_binding() {
        use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let (_dir, mut app) = app_with_fixture();
        app.push_hit(rect(0, 39, 10, 1), HitTarget::FooterKey('a'));

        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 3,
            row: 39,
            modifiers: KeyModifiers::NONE,
        });

        assert!(
            matches!(app.modals.last(), Some(Modal::AddRepository(_))),
            "clicking `a` in the footer did not open Add Repository"
        );
    }

    #[tokio::test]
    async fn actions_refuse_to_run_against_a_missing_directory() {
        let (_dir, mut app) = app_with_fixture();
        app.handle_key(key(KeyCode::Down));

        // The fixture never creates the worktree directories on disk.
        app.run_action(Action::Terminal);
        assert_eq!(app.notifications.len(), 1);
        assert!(app.notifications[0].text.contains("no longer exists"));
        assert_eq!(app.notifications[0].severity, Severity::Error);
    }
}
