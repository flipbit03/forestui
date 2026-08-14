//! Application state and behaviour: focus, key handling, and background work.

use crate::event::{AppEvent, BranchTarget, DetailMeta, EventTx, Severity};
use crate::modal::{
    AddRepositoryModal, AddWorktreeModal, ConfirmAction, ConfirmModal, CreateFromIssueModal, Modal,
    ModalEffect, ModalOutcome, ModalResult, SettingsModal,
};
use crate::models::{
    ClaudeSession, CustomClaudeButton, GitHubIssue, Repository, Settings, Worktree,
};
use crate::services::{claude_session, git, github, settings as settings_service, tmux};
use crate::state::AppState;
use crate::ui::widgets::TextInput;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use uuid::Uuid;

pub const NOTIFICATION_TTL: Duration = Duration::from_secs(4);
pub const ISSUE_REFRESH_INTERVAL: Duration = Duration::from_secs(300);
pub const SESSION_LIMIT: usize = 5;
pub const ISSUE_LIMIT: usize = 10;

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

/// An actionable control in the detail pane — the immediate-mode stand-in for
/// Textual's buttons.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Sync,
    AddWorktree,
    Editor,
    Terminal,
    Files,
    ClaudeNew,
    ClaudeYolo,
    ClaudeCustom(usize),
    ResumeSession(usize),
    ResumeYolo(usize),
    ResumeCustom { button: usize, session: usize },
    RefreshIssues,
    CreateFromIssue(usize),
    RemoveRepository,
    Archive,
    Unarchive,
    Delete,
}

/// An editable field in the detail pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    WorktreeName,
    BranchName,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetailItem {
    Action(Action),
    Field(Field),
}

#[derive(Debug, Clone)]
pub struct Notification {
    pub text: String,
    pub severity: Severity,
    pub created: Instant,
}

/// Something a mouse click can land on.
///
/// Immediate mode keeps no widget tree, so there is nothing to ask "what is at
/// this cell?". Each frame the renderers record the rectangle of every clickable
/// thing, and clicks are resolved against that list.
/// What clicking a modal control should do.
///
/// Not every control activates: a click that focuses a text field must not also
/// submit the modal, and clicking `◂ value ▸` should advance the value rather
/// than accept the dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModalClick {
    /// Buttons and checkboxes: focus, then activate.
    Activate,
    /// Text inputs: focus only.
    Focus,
    /// A `◂ value ▸` cycle: focus, then advance one step.
    Cycle,
    /// A row of a list: focus the list and select that row.
    Row(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitTarget {
    SidebarRow(usize),
    DetailItem(usize),
    /// Focus index within the modal on top of the stack, and what a click does.
    ModalControl {
        index: usize,
        click: ModalClick,
    },
}

#[derive(Debug, Clone, Copy)]
pub struct Hit {
    pub rect: ratatui::layout::Rect,
    pub target: HitTarget,
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
            rows.push(SidebarRow::Repository {
                id: repo.id,
                name: repo.name.clone(),
            });
            let active = repo.active_worktrees();
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
    pub fn detail_items(&self) -> Vec<DetailItem> {
        let mut items = Vec::new();
        let buttons = self.settings.custom_buttons.len();
        let sessions = self.sessions.as_ref().map(Vec::len).unwrap_or(0);

        if self.state.selection.is_worktree() {
            items.push(DetailItem::Action(Action::Sync));
            items.extend([Action::Editor, Action::Terminal, Action::Files].map(DetailItem::Action));
            items.push(DetailItem::Action(Action::ClaudeNew));
            items.push(DetailItem::Action(Action::ClaudeYolo));
            for index in 0..buttons {
                items.push(DetailItem::Action(Action::ClaudeCustom(index)));
            }
            for session in 0..sessions {
                items.push(DetailItem::Action(Action::ResumeSession(session)));
                items.push(DetailItem::Action(Action::ResumeYolo(session)));
                for button in 0..buttons {
                    items.push(DetailItem::Action(Action::ResumeCustom { button, session }));
                }
            }
            items.push(DetailItem::Field(Field::WorktreeName));
            items.push(DetailItem::Field(Field::BranchName));
            let archived = self
                .state
                .selected_worktree()
                .map(|(_, w)| w.is_archived)
                .unwrap_or(false);
            items.push(DetailItem::Action(if archived {
                Action::Unarchive
            } else {
                Action::Archive
            }));
            items.push(DetailItem::Action(Action::Delete));
        } else if self.state.selection.is_repository() {
            items.push(DetailItem::Action(Action::Sync));
            items.push(DetailItem::Action(Action::AddWorktree));
            items.extend([Action::Editor, Action::Terminal, Action::Files].map(DetailItem::Action));
            items.push(DetailItem::Action(Action::ClaudeNew));
            items.push(DetailItem::Action(Action::ClaudeYolo));
            for index in 0..buttons {
                items.push(DetailItem::Action(Action::ClaudeCustom(index)));
            }
            for session in 0..sessions {
                items.push(DetailItem::Action(Action::ResumeSession(session)));
                items.push(DetailItem::Action(Action::ResumeYolo(session)));
                for button in 0..buttons {
                    items.push(DetailItem::Action(Action::ResumeCustom { button, session }));
                }
            }
            items.push(DetailItem::Action(Action::RefreshIssues));
            let issues = self.issues.as_ref().map(Vec::len).unwrap_or(0);
            for index in 0..issues {
                items.push(DetailItem::Action(Action::CreateFromIssue(index)));
            }
            items.push(DetailItem::Action(Action::RemoveRepository));
        }
        items
    }

    /// Reload everything the detail pane shows for the current selection.
    pub fn reload_detail(&mut self) {
        self.detail_index = 0;
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

    // ------------------------------------------------------------------- events

    pub fn handle_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::Term(term_event) => self.handle_term_event(term_event),
            AppEvent::Tick => {
                self.spinner_index = self.spinner_index.wrapping_add(1);
                self.expire_notifications();
                if let Some(modal) = self.modals.last_mut() {
                    modal.tick();
                }
                if self.last_issue_refresh.elapsed() >= ISSUE_REFRESH_INTERVAL {
                    self.last_issue_refresh = Instant::now();
                    github::invalidate_cache(None);
                    if self.state.selection.is_repository() {
                        self.fetch_issues();
                    }
                }
            }
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
            AppEvent::StateChanged { select } => {
                // Reloading from disk drops the in-memory selection, so carry it over.
                let previous = self.state.selection;
                let show_archived = self.state.show_archived;
                self.state = AppState::load();
                self.state.show_archived = show_archived;
                self.state.selection = match select {
                    Some((repo_id, Some(worktree_id))) => crate::models::Selection {
                        repository_id: Some(repo_id),
                        worktree_id: Some(worktree_id),
                    },
                    Some((repo_id, None)) => crate::models::Selection {
                        repository_id: Some(repo_id),
                        worktree_id: None,
                    },
                    None => previous,
                };
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

    // -------------------------------------------------------------------- mouse

    /// Drop the previous frame's clickable regions. Called at the start of draw.
    pub fn clear_hits(&mut self) {
        self.hits.clear();
    }

    pub fn push_hit(&mut self, rect: ratatui::layout::Rect, target: HitTarget) {
        self.hits.push(Hit { rect, target });
    }

    /// Topmost target at a cell. Later hits win, so a modal drawn over the panes
    /// takes the click rather than whatever it covers.
    pub fn hit_at(&self, column: u16, row: u16) -> Option<HitTarget> {
        self.hits
            .iter()
            .rev()
            .find(|hit| {
                let r = hit.rect;
                column >= r.x && column < r.x + r.width && row >= r.y && row < r.y + r.height
            })
            .map(|hit| hit.target)
    }

    pub fn handle_mouse(&mut self, mouse: ratatui::crossterm::event::MouseEvent) {
        use ratatui::crossterm::event::{MouseButton, MouseEventKind};

        match mouse.kind {
            MouseEventKind::ScrollDown => {
                self.scroll_focused(1);
                return;
            }
            MouseEventKind::ScrollUp => {
                self.scroll_focused(-1);
                return;
            }
            MouseEventKind::Down(MouseButton::Left) => {}
            _ => return,
        }

        let Some(target) = self.hit_at(mouse.column, mouse.row) else {
            return;
        };

        // A modal owns every click while it is open, including the ones that
        // land on the panes behind it.
        if !self.modals.is_empty() {
            if let HitTarget::ModalControl { index, click } = target {
                self.click_modal_control(index, click);
            }
            return;
        }

        match target {
            HitTarget::SidebarRow(index) => {
                self.focus = Focus::Sidebar;
                self.sidebar_index = index;
                self.select_current_row();
            }
            HitTarget::DetailItem(index) => {
                self.focus = Focus::Detail;
                self.detail_index = index;
                if let Some(DetailItem::Action(action)) = self.detail_items().get(index).cloned() {
                    self.run_action(action);
                }
            }
            HitTarget::ModalControl { .. } => {}
        }
    }

    /// Move the focus ring of whichever pane has focus, for the scroll wheel.
    fn scroll_focused(&mut self, delta: isize) {
        match self.focus {
            Focus::Sidebar => self.move_sidebar(delta),
            Focus::Detail => {
                let len = self.detail_items().len();
                if len == 0 {
                    return;
                }
                let next = (self.detail_index as isize + delta).clamp(0, len as isize - 1);
                self.detail_index = next as usize;
            }
        }
    }

    /// Apply a click to a modal control: always focus it, then do whatever that
    /// kind of control does when driven from the keyboard.
    fn click_modal_control(&mut self, index: usize, click: ModalClick) {
        use ratatui::crossterm::event::KeyCode;

        if let Some(modal) = self.modals.last_mut() {
            modal.set_focus(index);
            if let ModalClick::Row(row) = click {
                modal.set_row(row);
            }
        }
        match click {
            ModalClick::Activate => self.send_modal_key(KeyCode::Enter),
            ModalClick::Cycle => self.send_modal_key(KeyCode::Right),
            // Focusing a field must not submit the dialog, and selecting a row
            // is already done above.
            ModalClick::Focus | ModalClick::Row(_) => {}
        }
    }

    fn send_modal_key(&mut self, code: ratatui::crossterm::event::KeyCode) {
        use ratatui::crossterm::event::{KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        self.handle_modal_key(KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        });
    }

    // --------------------------------------------------------------------- keys

    fn handle_key(&mut self, key: ratatui::crossterm::event::KeyEvent) {
        use ratatui::crossterm::event::{KeyCode, KeyModifiers};

        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            self.should_quit = true;
            return;
        }

        if !self.modals.is_empty() {
            self.handle_modal_key(key);
            return;
        }

        // A focused text field owns every printable key.
        if self.focus == Focus::Detail
            && let Some(DetailItem::Field(field)) =
                self.detail_items().get(self.detail_index).cloned()
            && self.handle_field_key(field, key)
        {
            return;
        }

        match key.code {
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Sidebar => Focus::Detail,
                    Focus::Detail => Focus::Sidebar,
                };
                return;
            }
            KeyCode::Up => {
                match self.focus {
                    Focus::Sidebar => self.move_sidebar(-1),
                    Focus::Detail => self.detail_index = self.detail_index.saturating_sub(1),
                }
                return;
            }
            KeyCode::Down => {
                match self.focus {
                    Focus::Sidebar => self.move_sidebar(1),
                    Focus::Detail => {
                        let len = self.detail_items().len();
                        if len > 0 {
                            self.detail_index = (self.detail_index + 1).min(len - 1);
                        }
                    }
                }
                return;
            }
            KeyCode::Enter => {
                match self.focus {
                    Focus::Sidebar => self.select_current_row(),
                    Focus::Detail => {
                        if let Some(DetailItem::Action(action)) =
                            self.detail_items().get(self.detail_index).cloned()
                        {
                            self.run_action(action);
                        }
                    }
                }
                return;
            }
            _ => {}
        }

        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('a') => self
                .modals
                .push(Modal::AddRepository(AddRepositoryModal::new())),
            KeyCode::Char('w') => self.action_add_worktree(),
            KeyCode::Char('e') => self.run_action(Action::Editor),
            KeyCode::Char('t') => self.run_action(Action::Terminal),
            KeyCode::Char('o') => self.run_action(Action::Files),
            KeyCode::Char('n') => self.run_action(Action::ClaudeNew),
            KeyCode::Char('y') => self.run_action(Action::ClaudeYolo),
            KeyCode::Char('h') => self.action_toggle_archive(),
            KeyCode::Char('d') => self.action_delete(),
            KeyCode::Char('s') => self
                .modals
                .push(Modal::Settings(Box::new(SettingsModal::new(
                    &self.settings,
                )))),
            KeyCode::Char('r') => {
                self.rebuild_rows();
                self.reload_detail();
            }
            KeyCode::Char('A') => {
                self.state.show_archived = !self.state.show_archived;
                self.rebuild_rows();
            }
            KeyCode::Char('?') => self.notify(
                "a: Add Repo | w: Add Worktree | e: Editor | t: Terminal | \
                 n: Claude | h: Archive | d: Delete | s: Settings | q: Quit",
                Severity::Information,
            ),
            _ => {}
        }
    }

    /// Route a key to a focused rename field. Returns true when consumed.
    fn handle_field_key(&mut self, field: Field, key: ratatui::crossterm::event::KeyEvent) -> bool {
        use ratatui::crossterm::event::{KeyCode, KeyModifiers};

        if matches!(
            key.code,
            KeyCode::Tab | KeyCode::BackTab | KeyCode::Up | KeyCode::Down
        ) {
            return false;
        }
        if key.code == KeyCode::Enter {
            self.submit_rename(field);
            return true;
        }

        let input = match field {
            Field::WorktreeName => &mut self.name_input,
            Field::BranchName => &mut self.branch_input,
        };
        match key.code {
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                input.kill_to_start();
                true
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                input.insert(c);
                true
            }
            KeyCode::Backspace => {
                input.backspace();
                true
            }
            KeyCode::Delete => {
                input.delete();
                true
            }
            KeyCode::Left => {
                input.move_left();
                true
            }
            KeyCode::Right => {
                input.move_right();
                true
            }
            KeyCode::Home => {
                input.move_home();
                true
            }
            KeyCode::End => {
                input.move_end();
                true
            }
            // A focused field swallows every printable key, so Escape has to
            // hand focus back as well as undo the edit — otherwise the global
            // hotkeys are unreachable from here.
            KeyCode::Esc => {
                self.reset_rename_fields();
                self.focus = Focus::Sidebar;
                true
            }
            _ => false,
        }
    }

    fn reset_rename_fields(&mut self) {
        if let Some((_repo, worktree)) = self.state.selected_worktree() {
            self.name_input = TextInput::new(worktree.name.clone());
            self.branch_input = TextInput::new(worktree.branch.clone());
        }
    }

    // ------------------------------------------------------------------- modals

    fn handle_modal_key(&mut self, key: ratatui::crossterm::event::KeyEvent) {
        let Some(modal) = self.modals.last_mut() else {
            return;
        };
        match modal.handle_key(key) {
            ModalOutcome::None => {}
            ModalOutcome::Close => {
                self.modals.pop();
            }
            ModalOutcome::Push(child) => self.modals.push(*child),
            ModalOutcome::Effect(effect) => self.run_modal_effect(effect),
            ModalOutcome::Submit(result) => {
                self.modals.pop();
                if let Some(parent) = self.modals.last_mut() {
                    parent.receive_child(&result);
                }
                self.apply_modal_result(result);
            }
        }
    }

    fn run_modal_effect(&mut self, effect: ModalEffect) {
        match effect {
            ModalEffect::Fetch(repo_path) => {
                let repo_id = self.state.selection.repository_id.unwrap_or_default();
                let tx = self.tx.clone();
                tokio::spawn(async move {
                    if let Err(error) = git::fetch(&repo_path).await {
                        tx.send(AppEvent::FetchFailed(error.to_string()));
                        return;
                    }
                    let branches = git::list_branches(&repo_path, true)
                        .await
                        .unwrap_or_default();
                    let remotes = git::list_remotes(&repo_path).await.unwrap_or_default();
                    let current_branch = git::get_current_branch(&repo_path)
                        .await
                        .unwrap_or_else(|_| "main".to_string());
                    tx.send(AppEvent::Branches {
                        repo_id,
                        branches,
                        remotes,
                        current_branch,
                        target: BranchTarget::RefetchOpenModal,
                    });
                });
            }
        }
    }

    fn apply_modal_result(&mut self, result: ModalResult) {
        match result {
            ModalResult::RepositoryAdded {
                path,
                import_worktrees,
            } => self.add_repository(path, import_worktrees),
            ModalResult::WorktreeCreated {
                repo_id,
                name,
                branch,
                new_branch,
            } => self.create_worktree(repo_id, name, branch, new_branch, None, false),
            ModalResult::WorktreeFromIssue {
                repo_id,
                name,
                branch,
                pull_first,
                base_branch,
            } => self.create_worktree(repo_id, name, branch, true, base_branch, pull_first),
            ModalResult::SettingsSaved(settings) => {
                self.settings = *settings;
                if let Err(error) = settings_service::save_settings(&self.settings) {
                    self.notify(format!("Could not save settings: {error}"), Severity::Error);
                } else {
                    self.notify("Settings saved", Severity::Information);
                }
                self.detail_index = 0;
            }
            ModalResult::CustomButtonsSaved(_) | ModalResult::CustomButtonSaved(_) => {}
            ModalResult::Confirmed(action) => self.apply_confirmed(action),
        }
    }

    fn apply_confirmed(&mut self, action: ConfirmAction) {
        match action {
            ConfirmAction::RemoveRepository(repo_id) => {
                self.state.remove_repository(repo_id);
                self.rebuild_rows();
                self.sync_sidebar_index();
                self.reload_detail();
            }
            ConfirmAction::DeleteWorktree(worktree_id) => {
                let Some((repo, worktree)) = self.state.find_worktree(worktree_id) else {
                    return;
                };
                let repo_path = repo.source_path.clone();
                let worktree_path = worktree.path.clone();
                let tx = self.tx.clone();
                tokio::spawn(async move {
                    // A failed git removal must not strand the entry in state.
                    let _ = git::remove_worktree(&repo_path, &worktree_path).await;
                    tx.send(AppEvent::ReloadDetail);
                });
                self.state.remove_worktree(worktree_id);
                self.rebuild_rows();
                self.sync_sidebar_index();
                self.reload_detail();
            }
        }
    }

    fn on_branches(
        &mut self,
        repo_id: Uuid,
        branches: Vec<String>,
        remotes: Vec<String>,
        current_branch: String,
        target: BranchTarget,
    ) {
        match target {
            BranchTarget::RefetchOpenModal => {
                if let Some(Modal::CreateFromIssue(modal)) = self.modals.last_mut() {
                    modal.update_branches(branches, remotes, &current_branch);
                }
            }
            BranchTarget::AddWorktree => {
                let Some(repo) = self.state.find_repository(repo_id) else {
                    return;
                };
                self.modals
                    .push(Modal::AddWorktree(Box::new(AddWorktreeModal::new(
                        repo,
                        branches,
                        remotes,
                        settings_service::get_forest_path(),
                        self.settings.branch_prefix.clone(),
                    ))));
            }
            BranchTarget::CreateFromIssue => {
                let Some(index) = self.pending_issue.take() else {
                    return;
                };
                let Some(issue) = self.issues.as_ref().and_then(|i| i.get(index)).cloned() else {
                    return;
                };
                let Some(repo) = self.state.find_repository(repo_id) else {
                    return;
                };
                self.modals
                    .push(Modal::CreateFromIssue(Box::new(CreateFromIssueModal::new(
                        repo,
                        &issue,
                        branches,
                        remotes,
                        settings_service::get_forest_path(),
                        &self.settings.branch_prefix,
                        &current_branch,
                    ))));
            }
        }
    }

    // ------------------------------------------------------------------ actions

    fn load_branches_then(&mut self, repo_id: Uuid, target: BranchTarget) {
        let Some(repo) = self.state.find_repository(repo_id) else {
            return;
        };
        let path = repo.source_path.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let branches = git::list_branches(&path, true).await.unwrap_or_default();
            let remotes = git::list_remotes(&path).await.unwrap_or_default();
            let current_branch = git::get_current_branch(&path)
                .await
                .unwrap_or_else(|_| "main".to_string());
            tx.send(AppEvent::Branches {
                repo_id,
                branches,
                remotes,
                current_branch,
                target,
            });
        });
    }

    fn action_add_worktree(&mut self) {
        match self.state.selection.repository_id {
            Some(repo_id) => self.load_branches_then(repo_id, BranchTarget::AddWorktree),
            None => self.notify("Select a repository first", Severity::Warning),
        }
    }

    fn action_toggle_archive(&mut self) {
        let Some(worktree_id) = self.state.selection.worktree_id else {
            return;
        };
        let archived = self
            .state
            .find_worktree(worktree_id)
            .map(|(_, w)| w.is_archived)
            .unwrap_or(false);
        self.state.set_archived(worktree_id, !archived);
        self.rebuild_rows();
        self.sync_sidebar_index();
        self.reload_detail();
    }

    fn action_delete(&mut self) {
        if let Some((_repo, worktree)) = self.state.selected_worktree() {
            let name = worktree.name.clone();
            let id = worktree.id;
            self.modals.push(Modal::Confirm(ConfirmModal::new(
                "Delete Worktree",
                format!("Permanently delete '{name}'?"),
                ConfirmAction::DeleteWorktree(id),
            )));
        } else if let Some(repo) = self.state.selected_repository() {
            let name = repo.name.clone();
            let id = repo.id;
            self.modals.push(Modal::Confirm(ConfirmModal::new(
                "Remove Repository",
                format!("Remove '{name}' from forestui?"),
                ConfirmAction::RemoveRepository(id),
            )));
        }
    }

    fn custom_button(&self, index: usize) -> Option<CustomClaudeButton> {
        self.settings.custom_buttons.get(index).cloned()
    }

    fn session_id(&self, index: usize) -> Option<String> {
        self.sessions
            .as_ref()
            .and_then(|s| s.get(index))
            .map(|s| s.id.clone())
    }

    pub fn run_action(&mut self, action: Action) {
        let Some(path) = self.state.selected_path() else {
            return;
        };

        // Opening a window in a directory that is gone silently lands the shell
        // in $HOME and lies about where it is, so refuse instead.
        let needs_directory = matches!(
            action,
            Action::Sync
                | Action::Editor
                | Action::Terminal
                | Action::Files
                | Action::ClaudeNew
                | Action::ClaudeYolo
                | Action::ClaudeCustom(_)
                | Action::ResumeSession(_)
                | Action::ResumeYolo(_)
                | Action::ResumeCustom { .. }
        );
        if needs_directory && !crate::util::path_exists(&path) {
            self.notify(
                format!("Directory no longer exists: {path}"),
                Severity::Error,
            );
            return;
        }

        match action {
            Action::Sync => self.sync(path),
            Action::AddWorktree => self.action_add_worktree(),
            Action::Editor => self.open_in_editor(&path),
            Action::Terminal => self.open_in_terminal(&path),
            Action::Files => self.open_in_file_manager(&path),
            Action::ClaudeNew => self.start_claude(&path, None, false, None),
            Action::ClaudeYolo => self.start_claude(&path, None, true, None),
            Action::ClaudeCustom(index) => {
                if let Some(button) = self.custom_button(index) {
                    self.start_claude(&path, None, false, Some(button));
                }
            }
            Action::ResumeSession(index) => {
                if let Some(id) = self.session_id(index) {
                    self.start_claude(&path, Some(&id), false, None);
                }
            }
            Action::ResumeYolo(index) => {
                if let Some(id) = self.session_id(index) {
                    self.start_claude(&path, Some(&id), true, None);
                }
            }
            Action::ResumeCustom { button, session } => {
                if let (Some(id), Some(button)) =
                    (self.session_id(session), self.custom_button(button))
                {
                    self.start_claude(&path, Some(&id), false, Some(button));
                }
            }
            Action::RefreshIssues => {
                github::invalidate_cache(None);
                self.issues = None;
                self.fetch_issues();
            }
            Action::CreateFromIssue(index) => {
                if let Some(repo_id) = self.state.selection.repository_id {
                    self.pending_issue = Some(index);
                    self.load_branches_then(repo_id, BranchTarget::CreateFromIssue);
                }
            }
            Action::RemoveRepository | Action::Delete => self.action_delete(),
            Action::Archive | Action::Unarchive => self.action_toggle_archive(),
        }
    }

    fn sync(&mut self, path: String) {
        self.notify("Syncing...", Severity::Information);
        let tx = self.tx.clone();
        tokio::spawn(async move {
            match git::pull(&path).await {
                Ok(()) => {
                    tx.info("Sync complete");
                    tx.send(AppEvent::ReloadDetail);
                }
                Err(error) => tx.error(format!("Sync failed: {error}")),
            }
        });
    }

    fn open_in_editor(&mut self, path: &str) {
        let editor = self.settings.default_editor.clone();

        if tmux::is_inside_tmux() && tmux::is_tui_editor(&editor) {
            let name = self.state.tmux_window_name(path);
            if tmux::create_editor_window(&name, path, &editor) {
                self.notify(
                    format!("Opened {editor} in edit:{name}"),
                    Severity::Information,
                );
                return;
            }
        }

        // GUI editor, or not inside tmux: spawn it detached.
        let mut parts = editor.split_whitespace();
        let Some(program) = parts.next() else {
            return;
        };
        let args: Vec<&str> = parts.collect();
        match std::process::Command::new(program)
            .args(&args)
            .arg(path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(_) => self.notify(format!("Opened in {program}"), Severity::Information),
            Err(_) => self.notify(format!("Editor '{editor}' not found"), Severity::Error),
        }
    }

    fn open_in_terminal(&mut self, path: &str) {
        let name = self.state.tmux_window_name(path);
        if tmux::create_shell_window(&name, path) {
            self.notify(
                format!("Opened terminal in term:{name}"),
                Severity::Information,
            );
        } else {
            self.notify("Failed to create terminal window", Severity::Error);
        }
    }

    fn open_in_file_manager(&mut self, path: &str) {
        let name = self.state.tmux_window_name(path);
        if tmux::create_mc_window(&name, path) {
            self.notify(format!("Opened mc in files:{name}"), Severity::Information);
        } else {
            self.notify("Failed to create mc window", Severity::Error);
        }
    }

    fn start_claude(
        &mut self,
        path: &str,
        resume_session_id: Option<&str>,
        yolo: bool,
        custom: Option<CustomClaudeButton>,
    ) {
        let name = self.state.tmux_window_name(path);
        let window = tmux::create_claude_window(
            &name,
            path,
            resume_session_id,
            yolo,
            custom.as_ref().map(|b| b.command.as_str()),
            custom.as_ref().map(|b| b.prefix.as_str()),
        );

        match window {
            Some(window_name) => {
                let verb = if resume_session_id.is_some() {
                    "Resuming"
                } else {
                    "Started"
                };
                let label = match (&custom, yolo) {
                    (Some(button), _) => format!(" ({})", button.label),
                    (None, true) => " (YOLO)".to_string(),
                    (None, false) => String::new(),
                };
                self.notify(
                    format!("{verb} Claude{label} in {window_name}"),
                    Severity::Information,
                );
            }
            None => self.notify("Failed to create Claude window", Severity::Error),
        }
    }

    fn submit_rename(&mut self, field: Field) {
        let Some((repo, worktree)) = self.state.selected_worktree() else {
            return;
        };
        let repo_path = repo.source_path.clone();
        let worktree_id = worktree.id;
        let old_name = worktree.name.clone();
        let old_branch = worktree.branch.clone();
        let old_path = PathBuf::from(&worktree.path);

        match field {
            Field::WorktreeName => {
                let new_name = self.name_input.value().to_string();
                if new_name.is_empty() || new_name == old_name {
                    return;
                }
                let new_path = old_path
                    .parent()
                    .map(|p| p.join(&new_name))
                    .unwrap_or_else(|| PathBuf::from(&new_name));
                if new_path.exists() {
                    self.notify("Path already exists", Severity::Error);
                    return;
                }
                if let Err(error) = std::fs::rename(&old_path, &new_path) {
                    self.notify(format!("Rename failed: {error}"), Severity::Error);
                    return;
                }
                let new_path_string = new_path.to_string_lossy().to_string();
                self.state.update_worktree(worktree_id, |w| {
                    w.name = new_name.clone();
                    w.path = new_path_string.clone();
                });
                claude_session::migrate_sessions(&old_path, &new_path);

                let tx = self.tx.clone();
                let target = new_path.clone();
                tokio::spawn(async move {
                    if let Err(error) = git::repair_worktree(&repo_path, &target).await {
                        tx.error(format!("Rename failed: {error}"));
                    }
                    tx.send(AppEvent::StateChanged { select: None });
                });
                self.rebuild_rows();
                self.sync_sidebar_index();
            }
            Field::BranchName => {
                let new_branch = self.branch_input.value().to_string();
                if new_branch.is_empty() || new_branch == old_branch {
                    return;
                }
                let worktree_path = worktree.path.clone();
                let tx = self.tx.clone();
                let requested = new_branch.clone();
                tokio::spawn(async move {
                    match git::rename_branch(&worktree_path, &old_branch, &requested).await {
                        Ok(()) => tx.send(AppEvent::StateChanged { select: None }),
                        Err(error) => tx.error(format!("Branch rename failed: {error}")),
                    }
                });
                self.state
                    .update_worktree(worktree_id, |w| w.branch = new_branch.clone());
                self.rebuild_rows();
            }
        }
    }

    fn add_repository(&mut self, path: String, import_worktrees: bool) {
        let name = Path::new(&path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.clone());
        let repo = Repository::new(name, path.clone());
        let repo_id = repo.id;
        self.state.add_repository(repo);
        self.state.select_repository(repo_id);
        self.rebuild_rows();
        self.sync_sidebar_index();
        self.reload_detail();

        if import_worktrees {
            self.import_existing_worktrees(repo_id, path);
        }
    }

    fn import_existing_worktrees(&mut self, repo_id: Uuid, source_path: String) {
        let forest_dir = settings_service::get_forest_path();
        let tx = self.tx.clone();
        let config_path = forest_dir.join(".forestui-config.json");

        tokio::spawn(async move {
            let listed = match git::list_worktrees(&source_path).await {
                Ok(listed) => listed,
                Err(error) => {
                    tx.error(format!("Failed to import worktrees: {error}"));
                    return;
                }
            };

            // Re-load state inside the task so the write is against fresh data.
            let mut state = crate::state::AppState::load_from(config_path);
            let source_resolved = std::fs::canonicalize(&source_path).ok();
            let mut imported = 0usize;

            for info in &listed {
                let candidate = std::fs::canonicalize(&info.path).ok();
                if candidate.is_some() && candidate == source_resolved {
                    continue;
                }
                if info
                    .path
                    .starts_with(&forest_dir.to_string_lossy().to_string())
                {
                    continue;
                }
                let name = Path::new(&info.path)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| info.path.clone());
                let branch = info.branch.clone().unwrap_or_else(|| "HEAD".to_string());
                state.add_worktree(repo_id, Worktree::new(name, branch, info.path.clone()));
                imported += 1;
            }

            tx.info(format!("Imported {imported} worktrees"));
            tx.send(AppEvent::StateChanged {
                select: Some((repo_id, None)),
            });
        });
    }

    fn create_worktree(
        &mut self,
        repo_id: Uuid,
        name: String,
        branch: String,
        new_branch: bool,
        base_branch: Option<String>,
        pull_first: bool,
    ) {
        let Some(repo) = self.state.find_repository(repo_id) else {
            return;
        };
        let source_path = repo.source_path.clone();
        let worktree_path = settings_service::get_forest_path()
            .join(&repo.name)
            .join(&name);
        let config_path = settings_service::get_forest_path().join(".forestui-config.json");
        let tx = self.tx.clone();

        tokio::spawn(async move {
            if pull_first {
                tx.info("Pulling repo...");
                let _ = git::pull(&source_path).await;
            }

            // Record where the worktree stemmed from, for the detail header.
            let (base, base_ref) = if let Some(base) = base_branch.clone() {
                let reference = git::get_ref(&source_path, &base).await;
                (Some(base), reference)
            } else if new_branch {
                let current = git::get_current_branch(&source_path).await.ok();
                (current, git::get_ref(&source_path, "HEAD").await)
            } else {
                (
                    Some(branch.clone()),
                    git::get_ref(&source_path, &branch).await,
                )
            };

            if let Err(error) = git::create_worktree(
                &source_path,
                &worktree_path,
                &branch,
                new_branch,
                base_branch.as_deref(),
            )
            .await
            {
                tx.error(format!("Failed to create worktree: {error}"));
                return;
            }

            let mut state = crate::state::AppState::load_from(config_path);
            let mut worktree = Worktree::new(
                name.clone(),
                branch,
                worktree_path.to_string_lossy().to_string(),
            );
            worktree.base_branch = base;
            worktree.created_from_ref = base_ref;
            let worktree_id = worktree.id;
            state.add_worktree(repo_id, worktree);

            tx.info(format!("Created worktree '{name}'"));
            tx.send(AppEvent::StateChanged {
                select: Some((repo_id, Some(worktree_id))),
            });
        });
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use crate::models::Selection;

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
    use ratatui::crossterm::event::KeyCode;

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

    #[tokio::test]
    async fn state_changed_keeps_the_current_selection() {
        let dir = tempfile::tempdir().expect("tempdir");
        crate::services::settings::set_forest_path(Some(&dir.path().to_string_lossy()));

        let mut state = AppState::load_from(dir.path().join(".forestui-config.json"));
        let repo = Repository::new("demo".into(), "/tmp/demo".into());
        let repo_id = repo.id;
        state.add_repository(repo);
        let worktree = Worktree::new("wt".into(), "feat/x".into(), "/tmp/wt".into());
        let worktree_id = worktree.id;
        state.add_worktree(repo_id, worktree);
        state.select_worktree(repo_id, worktree_id);

        let (tx, _rx) = crate::event::start();
        let mut app = App::with_state(tx, state, Settings::default());
        app.state.show_archived = true;

        app.handle_event(AppEvent::StateChanged { select: None });
        assert_eq!(app.state.selection.worktree_id, Some(worktree_id));
        assert!(
            app.state.show_archived,
            "the archived toggle survives a reload"
        );

        app.handle_event(AppEvent::StateChanged {
            select: Some((repo_id, None)),
        });
        assert_eq!(app.state.selection.worktree_id, None);
        assert_eq!(app.state.selection.repository_id, Some(repo_id));
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
    async fn scroll_wheel_moves_the_focused_pane() {
        use ratatui::crossterm::event::{KeyModifiers, MouseEvent, MouseEventKind};
        let (_dir, mut app) = app_with_fixture();
        let wheel = |kind| MouseEvent {
            kind,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        };

        app.handle_mouse(wheel(MouseEventKind::ScrollDown));
        assert_eq!(app.sidebar_index, 1);
        app.handle_mouse(wheel(MouseEventKind::ScrollUp));
        assert_eq!(app.sidebar_index, 0);
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
