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
pub use keys::BINDINGS;
pub use mouse::{Direction, Hit, HitTarget, ModalClick, ScrollbarGeom};

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
/// How often every repository is re-checked against `git worktree list`, so
/// worktrees created or removed outside forestui (an agent's `git worktree
/// add`, a manual remove) converge into the sidebar without being asked for.
pub const WORKTREE_SCAN_INTERVAL: Duration = Duration::from_secs(30);
/// How often the session list is re-scanned while forestui is just sitting
/// there. Frequent because a conversation in another window moves constantly,
/// and affordable because an unchanged transcript is never re-parsed. The
/// per-card refresh does *not* run on this timer — see `App::on_tick`.
pub const SESSION_REFRESH_INTERVAL: Duration = Duration::from_secs(10);
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

/// The branch to show beside a worktree name, or `None` when it only repeats it.
///
/// `w` creates `<branch_prefix><name>`, which is the overwhelmingly common case,
/// so showing it back costs a third of the sidebar and tells the user nothing. A
/// branch that genuinely differs — an existing branch checked out under another
/// name — is worth the space.
fn informative_branch(branch: &str, name: &str, prefix: &str) -> Option<String> {
    if branch == name || branch == format!("{prefix}{name}") {
        return None;
    }
    Some(branch.to_string())
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
        /// The branch, when it says something the name does not. A worktree
        /// created here is `<branch_prefix><name>`, so printing that beside the
        /// name is pure repetition — and in a 35-column sidebar the repetition
        /// is what gets truncated, leaving every row ending in `[feat/doc-impro`.
        branch: Option<String>,
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
    /// Paths with an issue fetch / session scan already running. Selection
    /// changes arrive far faster than the subprocess chains behind these
    /// panels; without the guard, bouncing across the sidebar stacks
    /// concurrent duplicate fetches for identical results.
    issues_in_flight: std::collections::HashSet<String>,
    sessions_in_flight: std::collections::HashSet<String>,
    /// Repositories with a `git worktree list` scan already running — the same
    /// duplicate-work guard as above, keyed by repository.
    worktree_scans_in_flight: std::collections::HashSet<Uuid>,
    /// When the last periodic worktree sweep started.
    last_worktree_scan: Instant,
    /// When the last periodic session refresh started.
    last_session_refresh: Instant,
    /// Session ids being re-read right now. Each one draws a spinner on its own
    /// card, so a slow transcript is visibly working rather than silently old.
    pub sessions_refreshing: std::collections::HashSet<String>,
    /// Which tmux window currently holds each Claude session, from the last
    /// live scan. The `@claude_session_id` stamp makes this a lookup: a card
    /// whose id is here is open somewhere, and `running` says whether Claude
    /// is actually the pane's foreground process.
    pub live_sessions: std::collections::HashMap<String, tmux::ClaudeWindow>,
    /// The last few pane lines of each *running* session, for the card peek.
    pub session_peeks: std::collections::HashMap<String, Vec<String>>,
    /// One live scan at a time — it shells out to tmux once per sweep.
    live_scan_in_flight: bool,
    /// Per-repository worktree mutation counter; see
    /// [`AppEvent::WorktreesScanned`]. Missing means 0 — repositories only
    /// enter the map once something about their worktrees changes.
    worktree_epochs: std::collections::HashMap<Uuid, u64>,
    /// Worktrees with a directory or branch rename running. Like
    /// [`App::removals_in_flight`], the reconcile must not act on what a
    /// listing says about these — it may describe the half-done rename.
    renames_in_flight: std::collections::HashSet<Uuid>,
    /// Worktrees with a removal running in the background. Guards duplicate
    /// removals, disables the Delete control, and holds quit until the results
    /// are folded (an orphaned git child would remove the tree on disk after
    /// the config was last saved).
    pub removals_in_flight: std::collections::HashSet<Uuid>,
    /// Quit was requested while removals were in flight; honoured when the
    /// last one folds. A second request quits immediately.
    pub quit_pending: bool,

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
    /// Whether to check for a newer release at startup; `--no-self-update`
    /// clears it.
    pub self_update: bool,
    /// Which optional terminal input modes this run asked for; `--no-hover`
    /// and `--no-focus-events` narrow it.
    pub input_modes: crate::terminal::InputModes,
    /// Whether *this* run turned tmux's server-wide `focus-events` on, and so
    /// has to turn it off again on the way out.
    focus_events_owned: bool,

    /// The detail items as the last frame drew them, each with whether it was
    /// enabled. Activation — a click or Enter — resolves against this snapshot
    /// rather than re-deriving the list, because a background event drained in
    /// the same batch (issues finishing, sessions landing) can grow or shrink
    /// the live list before the click is handled, silently remapping the index
    /// the user aimed at onto a different control. The snapshot is what was on
    /// screen, which is what the user acted on.
    pub drawn_items: Vec<(DetailItem, bool)>,

    pub name_input: TextInput,
    pub branch_input: TextInput,
    /// Worktree the rename inputs belong to, so a selection change resets them.
    pub(crate) rename_target: Option<Uuid>,
    /// Issue whose modal is waiting on a branch list to finish loading.
    /// The issue a Create-WT click captured, held across the branch load.
    pub(crate) pending_issue: Option<crate::models::GitHubIssue>,
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
        // The renderers read the active theme from the global; activate the
        // saved one before the first frame so launch never flashes the default.
        // `theme_name` carries the chosen slug — `theme` is the legacy
        // System/Dark/Light field preserved for the Python build, and reading
        // it here silently reset every launch to the default palette.
        crate::theme::set_active(&settings.theme_name);
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
            issues_in_flight: std::collections::HashSet::new(),
            sessions_in_flight: std::collections::HashSet::new(),
            worktree_scans_in_flight: std::collections::HashSet::new(),
            last_worktree_scan: Instant::now(),
            last_session_refresh: Instant::now(),
            sessions_refreshing: std::collections::HashSet::new(),
            live_sessions: std::collections::HashMap::new(),
            session_peeks: std::collections::HashMap::new(),
            live_scan_in_flight: false,
            worktree_epochs: std::collections::HashMap::new(),
            renames_in_flight: std::collections::HashSet::new(),
            removals_in_flight: std::collections::HashSet::new(),
            quit_pending: false,
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
            self_update: true,
            input_modes: crate::terminal::InputModes::default(),
            focus_events_owned: false,
            drawn_items: Vec::new(),
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

    /// Quit, unless a removal is still in flight — quitting then would orphan
    /// the git child: it finishes removing the tree on disk, but the result is
    /// never folded, so the saved config keeps a phantom entry. A second
    /// request forces the quit anyway.
    pub(super) fn request_quit(&mut self) {
        if self.removals_in_flight.is_empty() || self.quit_pending {
            self.should_quit = true;
        } else {
            self.quit_pending = true;
            self.notify(
                "Waiting for a worktree deletion to finish — press again to force quit",
                Severity::Information,
            );
        }
    }

    /// Undo the global tmux state this run turned on.
    ///
    /// Called from the main loop's exit; a `SIGTERM` still skips it, which is
    /// the same gap every tmux command has and not one a signal handler can
    /// close (spawning a process from one is not async-signal-safe).
    pub fn release_tmux_options(&mut self) {
        if std::mem::take(&mut self.focus_events_owned) {
            tmux::disable_focus_events();
        }
    }

    // ------------------------------------------------------------------ startup

    /// Work kicked off once the terminal is up.
    pub fn on_start(&mut self) {
        if self.input_modes.focus {
            match tmux::ensure_focus_events() {
                tmux::FocusEvents::Ours => self.focus_events_owned = true,
                tmux::FocusEvents::AlreadyOn => {}
                tmux::FocusEvents::Unavailable => {
                    self.notify("Could not enable focus events", Severity::Warning);
                }
            }
        } else {
            // Declining the mode also means clearing one a killed run left on:
            // the flag exists for people whose prefix key it is eating.
            tmux::release_stranded_focus_events();
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
        self.scan_all_worktrees();
        self.load_gh_status();
        if self.self_update {
            self.check_for_update();
        }
    }

    /// Kick a directory walk for one path's sessions. This is what finds
    /// sessions that appeared or disappeared; [`Self::refresh_visible_sessions`]
    /// is what keeps the ones already on screen current.
    fn scan_sessions(&mut self, path: &str) {
        // One scan per path at a time: bouncing across the sidebar must not
        // stack duplicate directory walks for content the cache already shows.
        if !self.sessions_in_flight.insert(path.to_string()) {
            return;
        }
        let tx = self.tx.clone();
        let session_path = path.to_string();
        let event_path = path.to_string();
        // Cloned here so the task needs nothing from state: pins are the app's
        // to own, the scan only honours them.
        let pinned = self.state.pinned_for(path);
        tokio::spawn(async move {
            let sessions = tokio::task::spawn_blocking(move || {
                claude_session::get_sessions_with_pins(&session_path, SESSION_LIMIT, &pinned)
            })
            .await
            .ok();
            tx.send(AppEvent::Sessions {
                path: event_path,
                sessions,
            });
        });
    }

    /// Ask tmux which windows hold which Claude sessions, and capture a tail
    /// of every running pane for the peek. One tmux listing plus one capture
    /// per running session, all off the loop.
    pub(super) fn scan_live_sessions(&mut self) {
        if self.live_scan_in_flight {
            return;
        }
        self.live_scan_in_flight = true;
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = tokio::task::spawn_blocking(|| {
                let windows = tmux::list_claude_windows();
                let peeks: std::collections::HashMap<String, Vec<String>> = windows
                    .iter()
                    .filter(|window| window.running)
                    .map(|window| {
                        (
                            window.session_id.clone(),
                            tmux::capture_pane_tail(&window.window_id, detail::PEEK_LINES),
                        )
                    })
                    .collect();
                (windows, peeks)
            })
            .await
            .unwrap_or_default();
            tx.send(AppEvent::LiveSessions {
                windows: result.0,
                peeks: result.1,
            });
        });
    }

    /// Which session card the focused detail control belongs to, if any.
    /// Read from the drawn snapshot, like every other act on the focus.
    fn focused_session_index(&self) -> Option<usize> {
        match self.drawn_items.get(self.detail_index)? {
            (
                DetailItem::Action(
                    Action::ResumeSession(index)
                    | Action::ResumeYolo(index)
                    | Action::RenameSession(index)
                    | Action::TogglePinSession(index)
                    | Action::DeleteSession(index)
                    | Action::ResumeCustom { session: index, .. },
                ),
                _,
            ) => Some(*index),
            _ => None,
        }
    }

    /// Move the pinned session under the cursor one slot up or down within
    /// the pinned block, keeping the cursor on the card it moved with.
    /// Returns false when the cursor is not on a pinned card, so the caller
    /// can let the key fall through.
    pub(super) fn move_focused_pin(&mut self, delta: isize) -> bool {
        let Some(path) = self.state.selected_path() else {
            return false;
        };
        let Some(index) = self.focused_session_index() else {
            return false;
        };
        let Some(id) = self
            .sessions
            .as_ref()
            .and_then(|list| list.get(index))
            .map(|s| s.id.clone())
        else {
            return false;
        };
        if !self.state.is_pinned(&path, &id) {
            return false;
        }
        if !self.state.move_pin(&path, &id, delta) {
            // Pinned but already at the end of the pinned block: the key was
            // still for us, it just has nowhere to go.
            return true;
        }
        self.sort_visible_sessions();
        // Every card carries the same controls, so the moved card's slots sit
        // exactly one card-width away and the cursor can ride along.
        let per_card = 5 + self.settings.custom_buttons.len();
        let target = self.detail_index as isize + delta * per_card as isize;
        if target >= 0 {
            self.detail_index = target as usize;
            self.detail_follow_focus = true;
        }
        self.report_save_error();
        self.redraw = true;
        true
    }

    /// Re-sort the visible session list around the current pin order.
    pub(super) fn sort_visible_sessions(&mut self) {
        let Some(path) = self.state.selected_path() else {
            return;
        };
        let pinned = self.state.pinned_for(&path);
        if let Some(list) = self.sessions.as_mut() {
            claude_session::sort_sessions(list, &pinned);
        }
    }

    /// Re-read every session card on screen, each in its own task.
    ///
    /// Coming back to forestui is exactly when the conversations it lists have
    /// moved on: turns were taken in other windows while it sat in the
    /// background. One task per card means each answers as soon as its own
    /// transcript is read, and the card carries a spinner until it does — so
    /// the pane fills in rather than sitting still and then blinking.
    pub fn refresh_visible_sessions(&mut self) {
        let Some(path) = self.state.selected_path() else {
            return;
        };
        // A directory walk alongside, for sessions that were started or removed
        // while we were away; the per-card reads cannot see those.
        self.scan_sessions(&path);

        let Some(sessions) = self.sessions.as_ref() else {
            return;
        };
        for id in sessions.iter().map(|s| s.id.clone()).collect::<Vec<_>>() {
            if !self.sessions_refreshing.insert(id.clone()) {
                continue;
            }
            let tx = self.tx.clone();
            let read_path = path.clone();
            let read_id = id.clone();
            let event_path = path.clone();
            tokio::spawn(async move {
                let session = tokio::task::spawn_blocking(move || {
                    claude_session::refresh_one(&read_path, &read_id)
                })
                .await
                .ok()
                .flatten();
                tx.send(AppEvent::SessionRefreshed {
                    path: event_path,
                    id,
                    session: Box::new(session),
                });
            });
        }
    }

    /// Reconcile every repository against `git worktree list`. One cheap git
    /// subprocess per repository, so this runs on startup, on tmux focus
    /// return, and on the periodic sweep.
    fn scan_all_worktrees(&mut self) {
        let ids: Vec<Uuid> = self.state.repositories().iter().map(|r| r.id).collect();
        for id in ids {
            self.scan_worktrees(id);
        }
    }

    /// Kick a background `git worktree list` for one repository. The listing
    /// comes back as [`AppEvent::WorktreesScanned`] and is reconciled against
    /// the config on the main loop — never here, where state may move while
    /// git runs.
    pub(super) fn scan_worktrees(&mut self, repo_id: Uuid) {
        let Some(repo) = self.state.find_repository(repo_id) else {
            return;
        };
        if !self.worktree_scans_in_flight.insert(repo_id) {
            return;
        }
        let source = repo.source_path.clone();
        let epoch = self.worktree_epoch(repo_id);
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let listed = git::list_worktrees(&source).await.ok();
            tx.send(AppEvent::WorktreesScanned {
                repo_id,
                epoch,
                listed,
            });
        });
    }

    fn worktree_epoch(&self, repo_id: Uuid) -> u64 {
        self.worktree_epochs.get(&repo_id).copied().unwrap_or(0)
    }

    /// Record that a repository's worktrees changed, so any scan spawned
    /// before this moment is folded as stale rather than reconciled.
    pub(super) fn bump_worktree_epoch(&mut self, repo_id: Uuid) {
        let counter = self.worktree_epochs.entry(repo_id).or_insert(0);
        *counter = counter.wrapping_add(1);
    }

    /// Bump the epoch of whichever repository owns a worktree, for the folds
    /// that arrive keyed by worktree alone.
    fn bump_worktree_epoch_for(&mut self, worktree_id: Uuid) {
        if let Some((repo, _)) = self.state.find_worktree(worktree_id) {
            let repo_id = repo.id;
            self.bump_worktree_epoch(repo_id);
        }
    }

    /// Keep the binary current, the way the Python build did on every launch.
    ///
    /// Deliberately fire-and-forget on a background task: the UI is already up
    /// by the time this runs, so a slow or unreachable GitHub costs nothing but
    /// a notification that never arrives. Being offline stays silent — it is
    /// the common case and not the user's problem to solve mid-session — but a
    /// *persistent* install failure (an unwritable install dir) is theirs to
    /// fix and surfaces once per launch.
    fn check_for_update(&self) {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let status = crate::version_check::update_if_stale().await;
            if let Some((message, severity)) = status.notification() {
                tx.notify(message, severity);
            }
        });
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
                    branch: informative_branch(
                        &worktree.branch,
                        &worktree.name,
                        &self.settings.branch_prefix,
                    ),
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
        // Landing on a repository (or one of its worktrees) is the moment its
        // tree should be fresh — an agent may have added one since last look.
        if let Some(repo_id) = self.state.selection.repository_id {
            self.scan_worktrees(repo_id);
        }
    }

    // ------------------------------------------------------------------- detail

    /// The list of focusable items the detail pane would render right now.
    ///
    /// Derived from the same [`detail::content`] walk the renderer draws, so
    /// item N here is control N on screen by construction. The running app
    /// acts on the [`App::drawn_items`] snapshot instead; this live view
    /// exists for the tests that assert on pane structure.
    #[cfg(test)]
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
        // Nothing of the new selection is on screen yet, and activating a
        // control from the previous pane against the new selection would act
        // on the wrong thing entirely.
        self.drawn_items.clear();
        // A create-from-issue flow belongs to the selection that started it.
        self.pending_issue = None;

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
            // Three independent subprocesses; run them concurrently so the
            // header appears after one git round-trip rather than three. The
            // branch answer goes unused for a worktree, which costs one spawn
            // and saves a second await chain.
            let (branch, commit, has_remote) = tokio::join!(
                git::get_current_branch(&meta_path),
                git::get_latest_commit(&meta_path),
                git::has_remote_tracking(&meta_path),
            );
            if is_repository {
                meta.branch = branch.ok();
            }
            if let Ok(commit) = commit {
                meta.commit_hash = Some(commit.short_hash);
                meta.commit_time = Some(commit.timestamp);
                meta.has_remote = has_remote.unwrap_or(false);
            }
            tx.send(AppEvent::Meta(Box::new(meta)));
        });

        // Cache-first: what was shown for this selection before renders again
        // synchronously, and the fresh scan lands behind it. The alternative —
        // dropping to "Loading…" on every switch with a warm cache in hand —
        // is the flash issue #29 is about.
        self.sessions = claude_session::peek_sessions(&path, SESSION_LIMIT);
        // The cache holds plain recency order; the pane shows pins first.
        self.sort_visible_sessions();
        // A new selection has nothing of the old one's refreshes to wait for.
        self.sessions_refreshing.clear();
        self.scan_sessions(&path);
        self.scan_live_sessions();

        if is_repository {
            match github::peek_issues(&path) {
                // Fresh enough to stand on its own; the 300s tick refreshes.
                Some((issues, true)) => self.issues = Some(issues),
                // Stale content beats a spinner: render it and refresh behind.
                Some((issues, false)) => {
                    self.issues = Some(issues);
                    self.fetch_issues();
                }
                None => self.fetch_issues(),
            }
        }
    }

    fn fetch_issues(&mut self) {
        let Some(repo) = self.state.selected_repository() else {
            return;
        };
        let path = repo.source_path.clone();
        // One fetch per path at a time — each one is a chain of gh
        // subprocesses and possibly a network call for an identical result.
        if !self.issues_in_flight.insert(path.clone()) {
            return;
        }
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

    /// Announce a state change that has just been persisted — unless it wasn't.
    ///
    /// The success toast is only true if the config write landed. A dropped
    /// write is silent otherwise, and the config deliberately loads as empty
    /// state, so the user would be told a worktree exists that vanishes on the
    /// next launch.
    pub fn notify_saved(&mut self, text: impl Into<String>) {
        match self.state.take_save_error() {
            Some(error) => self.notify(
                format!("Could not save the config: {error}"),
                Severity::Error,
            ),
            None => self.notify(text, Severity::Information),
        }
    }

    /// Report a failed write for a change that announces nothing when it works.
    fn report_save_error(&mut self) {
        if let Some(error) = self.state.take_save_error() {
            self.notify(
                format!("Could not save the config: {error}"),
                Severity::Error,
            );
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
        if self.issue_spinner_visible() || !self.sessions_refreshing.is_empty() {
            self.redraw = true;
        }

        // The relative times on screen ("11 seconds ago") have second
        // granularity, so once a second is exactly as often as they can
        // change. Repainting every tick — ten times a second — kept the
        // terminal busy for content that mostly cannot differ; repainting
        // never leaves them frozen at whatever the last keypress showed.
        if self.spinner_index.is_multiple_of(10) {
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

        if self.last_worktree_scan.elapsed() >= WORKTREE_SCAN_INTERVAL {
            self.last_worktree_scan = Instant::now();
            // No repaint here: a sweep that changes nothing stays invisible,
            // and one that does pays for its own when the fold lands.
            self.scan_all_worktrees();
        }

        if self.last_session_refresh.elapsed() >= SESSION_REFRESH_INTERVAL {
            self.last_session_refresh = Instant::now();
            // The directory walk only — not the per-card refresh. A cached scan
            // costs a fraction of a millisecond and repaints nothing when it
            // finds nothing, whereas the per-card pass raises a spinner on
            // every card, and one that clears before the next frame is a
            // flicker every ten seconds with nothing to wait for.
            if let Some(path) = self.state.selected_path() {
                self.scan_sessions(&path);
            }
            // The live map ages the same way: a window closed in another tab
            // should drop its badge without being asked. Its fold repaints
            // only on change, so the common no-change sweep stays free.
            self.scan_live_sessions();
        }

        if self.last_issue_refresh.elapsed() >= ISSUE_REFRESH_INTERVAL {
            self.last_issue_refresh = Instant::now();
            // Only while a repository pane is actually showing its issues —
            // refreshing under a worktree pane would churn subprocesses (and
            // destroy a warm cache) for content that is not on screen.
            if self.state.selection.is_repository()
                && let Some(repo) = self.state.selected_repository()
            {
                // Invalidate only the repository being refreshed. Nuking the
                // whole cache here made the next selection change on every
                // other repository a guaranteed cold miss — spinner included.
                let path = repo.source_path.clone();
                github::invalidate_cache(&path);
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
        // Scan results opt out for the same reason as the tick — the periodic
        // sweep almost always finds nothing, and its fold claims a repaint
        // itself when it changes something.
        if !mouse::is_pointer_motion(&event)
            && !matches!(
                event,
                AppEvent::Tick
                    | AppEvent::WorktreesScanned { .. }
                    | AppEvent::Sessions { .. }
                    | AppEvent::LiveSessions { .. }
            )
        {
            self.redraw = true;
        }
        match event {
            AppEvent::Term(term_event) => self.handle_term_event(term_event),
            AppEvent::Tick => self.on_tick(),
            AppEvent::GhStatus(status, username) => {
                self.gh_status = status.display(username.as_deref());
            }
            AppEvent::SessionRefreshed { path, id, session } => {
                self.sessions_refreshing.remove(&id);
                // A result for a selection the user has already left is not
                // wrong, just irrelevant — and folding it would put another
                // path's conversation into this pane. Keyed on the pane's own
                // path, as the directory scan is: two notions of "current"
                // eventually disagree.
                if path != self.meta.path {
                    return;
                }
                if let Some(list) = self.sessions.as_mut() {
                    match *session {
                        // Gone from disk: drop the card rather than leave a
                        // Resume button pointing at nothing.
                        None => list.retain(|s| s.id != id),
                        Some(fresh) => {
                            if let Some(slot) = list.iter_mut().find(|s| s.id == id) {
                                *slot = fresh;
                            }
                        }
                    }
                    // A turn taken while we were away makes a session the most
                    // recent one — within the recency half of the list, which
                    // pins always precede.
                    self.sort_visible_sessions();
                }
            }
            AppEvent::Sessions { path, sessions } => {
                // Release the guard even for a result the checks below drop.
                self.sessions_in_flight.remove(&path);
                // A failed scan (None) keeps whatever the cache painted; an
                // empty *successful* scan is real content and lands normally.
                if let Some(mut sessions) = sessions
                    && path == self.meta.path
                {
                    // The scan already sorted around the pins it was handed,
                    // but a pin toggled while it ran wins here.
                    claude_session::sort_sessions(&mut sessions, &self.state.pinned_for(&path));
                    if self.sessions.as_deref() != Some(sessions.as_slice()) {
                        // Claims its own repaint, like the worktree sweep: this
                        // runs on a timer and almost always finds exactly what
                        // is already on screen.
                        self.sessions = Some(sessions);
                        self.redraw = true;
                    }
                }
            }
            AppEvent::LiveSessions { windows, peeks } => {
                self.live_scan_in_flight = false;
                let fresh: std::collections::HashMap<String, tmux::ClaudeWindow> = windows
                    .into_iter()
                    .map(|window| (window.session_id.clone(), window))
                    .collect();
                // Claims its own repaint: this runs on the ten-second sweep
                // and usually finds exactly the map it replaced.
                if fresh != self.live_sessions || peeks != self.session_peeks {
                    self.live_sessions = fresh;
                    self.session_peeks = peeks;
                    self.redraw = true;
                }
            }
            AppEvent::SessionDeleted {
                path,
                session_id,
                title,
                result,
            } => match result {
                Ok(()) => {
                    // The pin dies with the transcript, whichever path it was
                    // pinned under — this is the one the card was deleted from.
                    self.state.remove_pin(&path, &session_id);
                    if path == self.meta.path
                        && let Some(list) = self.sessions.as_mut()
                    {
                        list.retain(|s| s.id != session_id);
                    }
                    self.notify(
                        format!("Deleted session '{}'", crate::util::truncate(&title, 40)),
                        Severity::Information,
                    );
                }
                Err(error) => self.notify(
                    format!("Could not delete the session: {error}"),
                    Severity::Error,
                ),
            },
            AppEvent::SessionRenamed {
                path,
                session_id,
                session,
                result,
            } => match result {
                Ok(()) => {
                    if path == self.meta.path
                        && let Some(list) = self.sessions.as_mut()
                        && let Some(fresh) = *session
                        && let Some(slot) = list.iter_mut().find(|s| s.id == session_id)
                    {
                        *slot = fresh;
                    }
                    self.notify("Session renamed", Severity::Information);
                }
                Err(error) => self.notify(
                    format!("Could not rename the session: {error}"),
                    Severity::Error,
                ),
            },
            AppEvent::Issues { path, issues } => {
                self.issues_in_flight.remove(&path);
                if self.state.selection.is_repository()
                    && Some(path.as_str())
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
                let name = worktree.name.clone();
                self.bump_worktree_epoch(repo_id);
                self.state.add_worktree(repo_id, *worktree);
                // Selecting a row a collapsed repository hides would leave the
                // cursor pointing at nothing, so the fold opens to reveal what
                // the user just created.
                self.collapsed.remove(&repo_id);
                self.state.select_worktree(repo_id, worktree_id);
                self.rebuild_rows();
                self.sync_sidebar_index();
                self.reload_detail();
                // Announced here rather than from the task, so a result that
                // was dropped above never produces a success toast for a
                // worktree that is not in the config — and `notify_saved`
                // extends that to a write that was attempted and failed.
                self.notify_saved(format!("Created worktree '{name}'"));
            }
            AppEvent::WorktreesScanned {
                repo_id,
                epoch,
                listed,
            } => {
                // Release the guard even for a result the checks below drop.
                self.worktree_scans_in_flight.remove(&repo_id);
                // A failed listing keeps state as-is: "could not list" must
                // never read as "there are none" and prune the whole tree.
                let Some(listed) = listed else { return };
                // The repository can be removed while git runs.
                if self.state.find_repository(repo_id).is_none() {
                    return;
                }
                // A create, remove, or rename folded while git ran: the
                // listing describes a state the config has moved past, and
                // reconciling against it would revert the newer change (a
                // renamed branch back to its old name, most visibly).
                if epoch != self.worktree_epoch(repo_id) {
                    return;
                }
                // Mutations still in flight are not the listing's to judge
                // either way; their own fold lands the truth.
                let protected: std::collections::HashSet<Uuid> = self
                    .removals_in_flight
                    .union(&self.renames_in_flight)
                    .copied()
                    .collect();
                let selected = self.state.selection;
                let Some(outcome) = self.state.reconcile_worktrees(repo_id, &listed, &protected)
                else {
                    return;
                };
                self.redraw = true;
                // Changed worktrees are *sidebar rows*; no detail pane shows
                // them, so the pane is left alone — reloading it would reset
                // the scroll of whatever the user is reading, twice a minute.
                self.rebuild_rows();
                self.sync_sidebar_index();
                // …unless the reconcile disturbed the selection itself. A
                // pruned selection reloads the pane outright. A branch that
                // drifted on the selected worktree reseeds only the rename
                // inputs — Enter would otherwise rename the branch straight
                // back to the stale value they still show — but must not
                // reload the pane: the path is unchanged, so its sessions and
                // scroll position are not stale, and a full reload on every
                // external `git checkout` would yank them out from under the
                // reader twice a minute.
                if self.state.selection != selected {
                    self.rename_target = None;
                    self.reload_detail();
                } else if selected
                    .worktree_id
                    .is_some_and(|id| outcome.branch_updated.contains(&id))
                    && let Some((_, worktree)) = self.state.selected_worktree()
                {
                    self.rename_target = Some(worktree.id);
                    self.name_input = TextInput::new(worktree.name.clone());
                    self.branch_input = TextInput::new(worktree.branch.clone());
                }
                let mut parts: Vec<String> = Vec::new();
                match outcome.added.as_slice() {
                    [] => {}
                    [name] => parts.push(format!("Detected worktree '{name}'")),
                    added => parts.push(format!("Detected {} worktrees", added.len())),
                }
                match outcome.removed.as_slice() {
                    [] => {}
                    [name] => parts.push(format!("Worktree '{name}' was removed outside forestui")),
                    removed => parts.push(format!(
                        "{} worktrees were removed outside forestui",
                        removed.len()
                    )),
                }
                if parts.is_empty() {
                    // A branch-only correction saved, but is not worth a toast.
                    self.report_save_error();
                } else {
                    self.notify_saved(parts.join("; "));
                }
            }
            AppEvent::WorktreeRenamed {
                worktree_id,
                name,
                path,
            } => {
                self.renames_in_flight.remove(&worktree_id);
                self.bump_worktree_epoch_for(worktree_id);
                self.state.update_worktree(worktree_id, |worktree| {
                    worktree.name = name;
                    worktree.path = path;
                });
                // The rename fields are only re-seeded when the *target*
                // changes, so without this they keep whatever the pane last
                // showed — the pre-rename name, if the user navigated away and
                // back while the task ran. Pressing Enter would then rename the
                // worktree straight back.
                self.rename_target = None;
                self.rebuild_rows();
                self.sync_sidebar_index();
                self.reload_detail();
                self.report_save_error();
            }
            AppEvent::WorktreeBranchRenamed {
                worktree_id,
                branch,
            } => {
                self.renames_in_flight.remove(&worktree_id);
                self.bump_worktree_epoch_for(worktree_id);
                self.state
                    .update_worktree(worktree_id, |worktree| worktree.branch = branch);
                self.rename_target = None;
                self.rebuild_rows();
                self.sync_sidebar_index();
                self.reload_detail();
                self.report_save_error();
            }
            AppEvent::WorktreeRemoveResult {
                worktree_id,
                outcome,
            } => self.on_worktree_remove_result(worktree_id, outcome),
            AppEvent::WorktreeRenameAborted { worktree_id } => {
                self.renames_in_flight.remove(&worktree_id);
            }
            AppEvent::ReloadDetail => self.reload_detail(),
        }
    }

    /// Fold a finished removal attempt. State changes only here — the confirm
    /// handler just spawns git, so a dirty refusal arrives with the entry (and
    /// the user's uncommitted work) fully intact.
    fn on_worktree_remove_result(
        &mut self,
        worktree_id: Uuid,
        outcome: Result<git::RemoveOutcome, String>,
    ) {
        self.removals_in_flight.remove(&worktree_id);
        // A quit that waited on this removal can go through now.
        if self.quit_pending && self.removals_in_flight.is_empty() {
            self.should_quit = true;
        }
        // The entry may have been removed by other means while git ran.
        let Some((_, worktree)) = self.state.find_worktree(worktree_id) else {
            return;
        };
        let name = worktree.name.clone();
        match outcome {
            Ok(git::RemoveOutcome::Removed) => {
                self.bump_worktree_epoch_for(worktree_id);
                self.state.remove_worktree(worktree_id);
                self.rebuild_rows();
                self.sync_sidebar_index();
                self.reload_detail();
                self.report_save_error();
            }
            Ok(git::RemoveOutcome::Dirty(summary)) => {
                // The name is capped so the summary — the one thing the user
                // needs before confirming — never clips off the dialog edge.
                let display = crate::util::truncate(&name, 40);
                self.modals.push(Modal::Confirm(
                    crate::modal::ConfirmModal::new(
                        "Uncommitted Changes",
                        format!(
                            "'{display}' has uncommitted changes:\n{summary}\nDeleting will discard them permanently."
                        ),
                        crate::modal::ConfirmAction::ForceDeleteWorktree(worktree_id),
                    )
                    // Pushed by a background event, not a user action — a
                    // keystroke already in the queue must not confirm it.
                    .with_arm_delay(),
                ));
            }
            Err(error) => {
                self.notify(
                    format!("Could not delete '{name}': {error}"),
                    Severity::Error,
                );
            }
        }
    }

    fn handle_term_event(&mut self, event: ratatui::crossterm::event::Event) {
        use ratatui::crossterm::event::{Event, KeyEventKind};
        match event {
            Event::Key(key) if key.kind == KeyEventKind::Press => self.handle_key(key),
            Event::Mouse(mouse) => self.handle_mouse(mouse),
            // tmux focus-events let the app refresh when the user comes back —
            // which is exactly when an agent in another window may have
            // created or removed a worktree.
            Event::FocusGained => {
                // Not `reload_detail`: that is for a *new* selection and resets
                // the focus index and scroll, which would yank the pane out
                // from under someone who just switched back to the window.
                self.last_session_refresh = Instant::now();
                self.refresh_visible_sessions();
                self.scan_live_sessions();
                self.scan_all_worktrees();
            }
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
    fn fixture(settings: Settings) -> (tempfile::TempDir, App) {
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

        // A bare channel: `event::start` would add a detached stdin-polling
        // thread per test. Dropping the receiver is fine — sends are
        // best-effort and no test reads events back.
        let (tx, _rx) = crate::event::test_channel();
        let app = App::with_state(tx, state, settings);
        (dir, app)
    }

    /// [`app_with_fixture`] with explicit settings, for tests that need a
    /// non-default configuration (a saved theme, a branch prefix).
    pub fn app_with_settings(settings: Settings) -> (tempfile::TempDir, App) {
        fixture(settings)
    }

    pub fn app_with_fixture() -> (tempfile::TempDir, App) {
        fixture(Settings::default())
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

    /// The same key with `Ctrl` held — what a pane is handed when tmux sends
    /// the prefix through, and what the app must not act on.
    pub fn ctrl(code: ratatui::crossterm::event::KeyCode) -> ratatui::crossterm::event::KeyEvent {
        use ratatui::crossterm::event::KeyModifiers;
        ratatui::crossterm::event::KeyEvent {
            modifiers: KeyModifiers::CONTROL,
            ..key(code)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{app_with_fixture, ctrl, key};
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
            custom_title: None,
            id: "s1".into(),
            title: "t".into(),
            recent_turns: Vec::new(),
            last_timestamp: chrono::Utc::now(),
            message_count: 1,
            git_branch: None,
            tokens: Default::default(),
            model: None,
        }]);
        // Resume + YOLO + one per custom button + Rename + Pin + Del.
        assert_eq!(app.detail_items().len(), baseline + 1 + 6);
    }

    #[tokio::test]
    async fn typing_in_a_rename_field_does_not_trigger_hotkeys() {
        let (_dir, mut app) = app_with_fixture();
        app.handle_key(key(KeyCode::Down));
        app.sessions = Some(Vec::new());
        snapshot_drawn(&mut app);

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
    async fn a_completed_rename_resyncs_the_field_it_renamed() {
        let (_dir, mut app) = app_with_fixture();
        // Select the first worktree, which seeds the rename fields from it.
        app.handle_key(key(KeyCode::Down));
        let worktree_id = app.state.selection.worktree_id.expect("a worktree");
        let old_name = app.name_input.value().to_string();

        // The rename runs off the event loop now, and the user is free to move
        // around while it does. Coming back re-seeds the field from the state
        // as it still is — the old name.
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Up));
        assert_eq!(app.name_input.value(), old_name);

        // Now the task lands.
        app.handle_event(AppEvent::WorktreeRenamed {
            worktree_id,
            name: "renamed".into(),
            path: "/tmp/renamed".into(),
        });

        assert_eq!(
            app.name_input.value(),
            "renamed",
            "the field still holds the pre-rename name; pressing Enter would rename it back"
        );
    }

    #[tokio::test]
    async fn quit_hotkey_quits_and_unknown_keys_do_nothing() {
        let (_dir, mut app) = app_with_fixture();
        // `?` is deliberately unbound: the footer is the complete key surface.
        app.handle_key(key(KeyCode::Char('?')));
        assert!(app.notifications.is_empty());
        assert!(app.modals.is_empty());

        app.handle_key(key(KeyCode::Char('q')));
        assert!(app.should_quit);
    }

    /// Defect B of issue #51: every binding fired on `Ctrl`+letter, so a
    /// `Ctrl+A` that reached the pane — which is exactly what tmux's own
    /// `bind C-a send-prefix` does — opened Add Repository, and `Ctrl+D`
    /// started deleting a worktree. `Ctrl+C` stays quit.
    #[tokio::test]
    async fn control_modified_keys_never_fire_footer_bindings() {
        let (_dir, mut app) = app_with_fixture();
        let archived_before = app.state.show_archived;

        for binding in ['a', 's', 'w', 'A', 'q'] {
            app.handle_key(ctrl(KeyCode::Char(binding)));
        }

        assert!(app.modals.is_empty(), "a Ctrl combination opened a modal");
        assert!(!app.should_quit, "Ctrl+Q quit the app");
        assert_eq!(
            app.state.show_archived, archived_before,
            "Ctrl+A toggled the archived section"
        );

        app.handle_key(ctrl(KeyCode::Char('c')));
        assert!(app.should_quit, "Ctrl+C no longer quits");
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
        // Still there: the entry leaves state only when git reports success.
        assert!(app.state.find_worktree(worktree_id).is_some());

        app.handle_event(AppEvent::WorktreeRemoveResult {
            worktree_id,
            outcome: Ok(git::RemoveOutcome::Removed),
        });
        assert!(app.state.find_worktree(worktree_id).is_none());
    }

    #[tokio::test]
    async fn a_dirty_worktree_gets_a_second_confirmation() {
        let (_dir, mut app) = app_with_fixture();
        app.handle_key(key(KeyCode::Down));
        let worktree_id = app.state.selection.worktree_id.expect("worktree selected");

        app.handle_event(AppEvent::WorktreeRemoveResult {
            worktree_id,
            outcome: Ok(git::RemoveOutcome::Dirty("2 modified, 1 untracked".into())),
        });

        let Some(Modal::Confirm(confirm)) = app.modals.last() else {
            panic!("expected the second confirmation");
        };
        assert!(confirm.message.contains("2 modified, 1 untracked"));
        assert_eq!(confirm.action.confirm_label(), "Delete anyway");
        assert!(matches!(
            confirm.action,
            crate::modal::ConfirmAction::ForceDeleteWorktree(id) if id == worktree_id
        ));
        // Nothing destroyed, nothing dropped from state.
        assert!(app.state.find_worktree(worktree_id).is_some());

        // Pushed by a background event: a keystroke already in flight when the
        // modal appeared must not confirm it.
        app.handle_key(key(KeyCode::Char('y')));
        assert!(
            matches!(app.modals.last(), Some(Modal::Confirm(_))),
            "an unarmed confirm must swallow the stale 'y'"
        );

        if let Some(Modal::Confirm(confirm)) = app.modals.last_mut() {
            confirm.disarm();
        }
        app.handle_key(key(KeyCode::Char('y')));
        assert!(app.modals.is_empty());
        // The forced removal is now in flight; the entry waits for the fold.
        assert!(app.removals_in_flight.contains(&worktree_id));
        assert!(app.state.find_worktree(worktree_id).is_some());
    }

    #[tokio::test]
    async fn a_failed_removal_keeps_the_entry_and_reports() {
        let (_dir, mut app) = app_with_fixture();
        app.handle_key(key(KeyCode::Down));
        let worktree_id = app.state.selection.worktree_id.expect("worktree selected");

        app.handle_event(AppEvent::WorktreeRemoveResult {
            worktree_id,
            outcome: Err("worktree is locked".into()),
        });

        assert!(app.state.find_worktree(worktree_id).is_some());
        assert_eq!(app.notifications.len(), 1);
        assert!(app.notifications[0].text.contains("worktree is locked"));
    }

    /// The regression that made themes non-sticky: startup activated the
    /// legacy `theme` field (always "system") instead of `theme_name`, so a
    /// saved theme applied in-session and silently reset on every launch.
    #[tokio::test]
    async fn startup_activates_the_saved_theme() {
        let _guard = crate::theme::test_lock();
        let (_dir, _app) = test_support::app_with_settings(Settings {
            theme_name: "nord".into(),
            ..Settings::default()
        });
        assert_eq!(
            crate::theme::active().slug,
            "nord",
            "the saved theme must be active before the first frame"
        );
        // The guard restores the pre-test theme on drop.
    }

    #[tokio::test]
    async fn a_warm_cache_renders_issues_without_a_loading_flash() {
        let (_dir, mut app) = app_with_fixture();
        let path = app
            .state
            .selected_repository()
            .expect("repo selected")
            .source_path
            .clone();
        let issue: crate::models::GitHubIssue = serde_json::from_str(
            r#"{"number":9,"title":"Cached","state":"OPEN","url":"https://x/9",
                "createdAt":"2026-08-01T10:00:00Z","updatedAt":"2026-08-01T10:00:00Z",
                "author":{"login":"me"},"assignees":[],"labels":[]}"#,
        )
        .expect("issue json");
        crate::services::github::seed_cache_for_test(&path, vec![issue], chrono::Utc::now());

        // Leave the repository and come back: the second visit must render
        // from cache synchronously, not drop to the Loading state (#29).
        app.handle_key(key(KeyCode::Down));
        app.handle_key(key(KeyCode::Up));
        let issues = app
            .issues
            .as_ref()
            .expect("no Loading flash on a warm cache");
        assert_eq!(issues[0].number, 9);
    }

    #[tokio::test]
    async fn delete_is_ignored_while_a_removal_is_in_flight() {
        let (_dir, mut app) = app_with_fixture();
        app.handle_key(key(KeyCode::Down));
        let worktree_id = app.state.selection.worktree_id.expect("worktree selected");

        app.removals_in_flight.insert(worktree_id);
        app.handle_key(key(KeyCode::Char('d')));
        assert!(
            app.modals.is_empty(),
            "no duplicate confirm while the removal runs"
        );
    }

    #[tokio::test]
    async fn quit_waits_for_an_in_flight_removal() {
        let (_dir, mut app) = app_with_fixture();
        app.handle_key(key(KeyCode::Down));
        let worktree_id = app.state.selection.worktree_id.expect("worktree selected");
        app.removals_in_flight.insert(worktree_id);

        app.handle_key(key(KeyCode::Char('q')));
        assert!(!app.should_quit, "quit must wait for the fold");
        assert!(app.quit_pending);

        app.handle_event(AppEvent::WorktreeRemoveResult {
            worktree_id,
            outcome: Ok(git::RemoveOutcome::Removed),
        });
        assert!(app.should_quit, "the fold releases the pending quit");
    }

    #[tokio::test]
    async fn a_second_quit_forces_out_past_an_in_flight_removal() {
        let (_dir, mut app) = app_with_fixture();
        app.handle_key(key(KeyCode::Down));
        let worktree_id = app.state.selection.worktree_id.expect("worktree selected");
        app.removals_in_flight.insert(worktree_id);

        app.handle_key(key(KeyCode::Char('q')));
        assert!(!app.should_quit);
        app.handle_key(key(KeyCode::Char('q')));
        assert!(app.should_quit);
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
        // Collapsed on purpose: selecting a hidden row would leave the cursor
        // pointing at nothing, so the fold has to open to reveal the result.
        app.toggle_collapsed(0);
        assert!(app.collapsed.contains(&repo_id));
        let worktree = Worktree::new("fresh".into(), "feat/fresh".into(), "/tmp/fresh".into());
        let worktree_id = worktree.id;

        app.handle_event(AppEvent::WorktreeAdded {
            repo_id,
            worktree: Box::new(worktree),
        });

        assert!(app.state.find_worktree(worktree_id).is_some());
        assert_eq!(app.state.selection.worktree_id, Some(worktree_id));
        assert!(!app.collapsed.contains(&repo_id), "the fold stayed shut");
        assert!(
            app.rows
                .iter()
                .any(|row| matches!(row, SidebarRow::Worktree { id, .. } if *id == worktree_id)),
            "the sidebar was not rebuilt"
        );
        assert_eq!(
            app.notifications.last().map(|n| n.text.as_str()),
            Some("Created worktree 'fresh'"),
            "the toast is announced by the fold, not the task"
        );
    }

    #[tokio::test]
    async fn a_worktree_that_could_not_be_saved_is_not_announced_as_created() {
        let (dir, mut app) = app_with_fixture();
        let repo_id = app.state.repositories()[0].id;

        // Make the config unwritable: the directory holding it denies writes, so
        // the atomic write cannot even stage its temp file.
        let mut perms = std::fs::metadata(dir.path()).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(dir.path(), perms).unwrap();

        let worktree = Worktree::new("doomed".into(), "feat/doomed".into(), "/tmp/doomed".into());
        app.handle_event(AppEvent::WorktreeAdded {
            repo_id,
            worktree: Box::new(worktree),
        });

        let last = app.notifications.last().expect("a toast");
        assert_eq!(
            last.severity,
            Severity::Error,
            "a failed write was announced as a success: {}",
            last.text
        );
        assert!(
            last.text.starts_with("Could not save the config"),
            "unexpected toast: {}",
            last.text
        );

        // Let the tempdir clean itself up.
        let mut perms = std::fs::metadata(dir.path()).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        std::fs::set_permissions(dir.path(), perms).unwrap();
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

    /// A scan result stamped with the repository's *current* epoch, the way a
    /// listing that raced nothing would arrive.
    fn scanned(app: &App, repo_id: Uuid, listed: Vec<git::WorktreeInfo>) -> AppEvent {
        AppEvent::WorktreesScanned {
            repo_id,
            epoch: app.worktree_epoch(repo_id),
            listed: Some(listed),
        }
    }

    /// What `git worktree list` would print for the fixture repository: the
    /// main checkout first, then every tracked worktree, plus `extra` paths.
    fn scan_listing(app: &App, extra: &[(&std::path::Path, &str)]) -> Vec<git::WorktreeInfo> {
        let repo = &app.state.repositories()[0];
        let mut listed = vec![git::WorktreeInfo {
            path: repo.source_path.clone(),
            head: "abc123".into(),
            branch: Some("main".into()),
        }];
        listed.extend(repo.worktrees.iter().map(|w| git::WorktreeInfo {
            path: w.path.clone(),
            head: "abc123".into(),
            branch: Some(w.branch.clone()),
        }));
        listed.extend(extra.iter().map(|(path, branch)| git::WorktreeInfo {
            path: path.to_string_lossy().to_string(),
            head: "abc123".into(),
            branch: Some((*branch).to_string()),
        }));
        listed
    }

    /// A scan delivers git's listing as one event; the fold adopts the
    /// worktrees the config has never seen, keeps every tracked one
    /// (archived included), and persists the result once.
    #[tokio::test]
    async fn scanned_worktrees_reconcile_into_state() {
        let (dir, mut app) = app_with_fixture();
        let repo_id = app.state.repositories()[0].id;
        let before = app.state.repositories()[0].worktrees.len();

        // A worktree created outside forestui — the directory must exist, or
        // the reconcile treats it as a prunable stub.
        let adhoc = dir.path().join("adhoc");
        std::fs::create_dir_all(&adhoc).unwrap();
        let listed = scan_listing(&app, &[(&adhoc, "feat/adhoc")]);

        let event = scanned(&app, repo_id, listed.clone());
        app.handle_event(event);

        let worktrees = &app.state.repositories()[0].worktrees;
        assert_eq!(worktrees.len(), before + 1);
        let adopted = worktrees.last().expect("the adopted worktree");
        assert_eq!(adopted.name, "adhoc");
        assert_eq!(adopted.branch, "feat/adhoc");
        assert!(
            worktrees.iter().any(|w| w.is_archived),
            "reconciling dropped the archived flag"
        );
        assert_eq!(app.state.selection.repository_id, Some(repo_id));
        assert_eq!(app.state.selection.worktree_id, None);

        // In memory is not enough: an adoption that never reaches the config
        // is gone on the next launch, and the fold is the only writer.
        let reloaded = AppState::load_from(dir.path().join(".forestui-config.json"));
        assert_eq!(
            reloaded.repositories()[0].worktrees.len(),
            before + 1,
            "the adoption was never persisted"
        );

        // The same listing again is a no-op, not a duplicate row.
        let event = scanned(&app, repo_id, listed.clone());
        app.handle_event(event);
        assert_eq!(app.state.repositories()[0].worktrees.len(), before + 1);

        // A result for a repository that has since been removed is dropped,
        // and a failed listing must not prune anything.
        let event = scanned(&app, Uuid::new_v4(), listed);
        app.handle_event(event);
        let epoch = app.worktree_epoch(repo_id);
        app.handle_event(AppEvent::WorktreesScanned {
            repo_id,
            epoch,
            listed: None,
        });
        assert_eq!(app.state.repositories()[0].worktrees.len(), before + 1);
    }

    /// A scan that no longer lists a tracked worktree prunes it — someone ran
    /// `git worktree remove` outside forestui — and when it was the worktree
    /// on screen, the selection falls back to its repository.
    #[tokio::test]
    async fn a_scan_prunes_worktrees_git_no_longer_lists() {
        let (dir, mut app) = app_with_fixture();
        let repo_id = app.state.repositories()[0].id;

        app.handle_key(key(KeyCode::Down));
        let selected = app.state.selection.worktree_id.expect("a worktree");

        let mut listed = scan_listing(&app, &[]);
        let removed_path = app
            .state
            .find_worktree(selected)
            .map(|(_, w)| w.path.clone())
            .expect("the selected worktree");
        listed.retain(|info| info.path != removed_path);

        let event = scanned(&app, repo_id, listed);
        app.handle_event(event);

        assert!(app.state.find_worktree(selected).is_none());
        assert_eq!(app.state.selection.repository_id, Some(repo_id));
        assert_eq!(app.state.selection.worktree_id, None);
        let reloaded = AppState::load_from(dir.path().join(".forestui-config.json"));
        assert!(
            reloaded
                .repositories()
                .iter()
                .flat_map(|r| &r.worktrees)
                .all(|w| w.path != removed_path),
            "the prune was never persisted"
        );
    }

    /// A listing spawned before a mutation folded describes a state the
    /// config has moved past; reconciling against it would revert the newer
    /// change, so it must fold as stale.
    #[tokio::test]
    async fn a_scan_older_than_a_mutation_is_discarded() {
        let (dir, mut app) = app_with_fixture();
        let repo_id = app.state.repositories()[0].id;

        // Captured now, delivered later: the listing predates the rename.
        let stale = scanned(&app, repo_id, scan_listing(&app, &[]));

        app.handle_key(key(KeyCode::Down));
        let worktree_id = app.state.selection.worktree_id.expect("a worktree");
        app.handle_event(AppEvent::WorktreeBranchRenamed {
            worktree_id,
            branch: "feat/renamed".into(),
        });

        app.handle_event(stale);
        assert_eq!(
            app.state
                .find_worktree(worktree_id)
                .map(|(_, w)| w.branch.as_str()),
            Some("feat/renamed"),
            "the stale listing reverted the rename"
        );
        let reloaded = AppState::load_from(dir.path().join(".forestui-config.json"));
        assert!(
            reloaded
                .repositories()
                .iter()
                .flat_map(|r| &r.worktrees)
                .any(|w| w.branch == "feat/renamed"),
            "the reverted branch was persisted"
        );
    }

    /// A worktree whose removal (or rename) is still in flight is not the
    /// listing's to judge: git may have already deleted the directory, but
    /// the removal's own fold owns both the state change and the toast.
    #[tokio::test]
    async fn a_scan_does_not_prune_a_worktree_with_a_removal_in_flight() {
        let (_dir, mut app) = app_with_fixture();
        let repo_id = app.state.repositories()[0].id;
        let worktree_id = app.state.repositories()[0].worktrees[0].id;
        app.removals_in_flight.insert(worktree_id);

        let mut listed = scan_listing(&app, &[]);
        let removed_path = app.state.repositories()[0].worktrees[0].path.clone();
        listed.retain(|info| info.path != removed_path);

        let event = scanned(&app, repo_id, listed);
        app.handle_event(event);
        assert!(
            app.state.find_worktree(worktree_id).is_some(),
            "the scan pruned an entry whose removal fold still owns it"
        );
    }

    /// Focus return and the periodic tick are the standing triggers — the
    /// promise that an agent's worktree appears without being asked for.
    /// A minimal session card for the refresh tests.
    fn a_session(id: &str) -> ClaudeSession {
        ClaudeSession {
            id: id.to_string(),
            title: format!("session {id}"),
            custom_title: None,
            recent_turns: Vec::new(),
            last_timestamp: chrono::Utc::now(),
            message_count: 1,
            git_branch: None,
            tokens: Default::default(),
            model: None,
        }
    }

    /// Coming back to the window is when the conversations on screen have
    /// moved on, so each card on screen is re-read and says so while it is.
    #[tokio::test]
    async fn focus_return_refreshes_every_session_on_screen() {
        let (_dir, mut app) = app_with_fixture();
        app.sessions = Some(vec![a_session("one"), a_session("two")]);

        app.handle_event(AppEvent::Term(
            ratatui::crossterm::event::Event::FocusGained,
        ));

        assert_eq!(
            app.sessions_refreshing.len(),
            2,
            "focus return did not re-read the cards on screen"
        );

        // The tick keeps the list current too, but with the directory walk
        // alone: a per-card pass raises a spinner on every card, and one that
        // clears before the next frame is a flicker every ten seconds.
        app.sessions_refreshing.clear();
        app.sessions_in_flight.clear();
        app.last_session_refresh = Instant::now() - SESSION_REFRESH_INTERVAL;
        app.handle_event(AppEvent::Tick);
        assert!(
            app.sessions_refreshing.is_empty(),
            "the tick must not raise per-card spinners"
        );
        assert!(
            !app.sessions_in_flight.is_empty(),
            "the tick did not scan for new or removed sessions"
        );
    }

    /// The scan runs on a timer and almost always finds what is already on
    /// screen. Repainting for that is what the tick's whole design is against.
    #[tokio::test]
    async fn a_scan_that_changes_nothing_does_not_repaint() {
        let (_dir, mut app) = app_with_fixture();
        // The fold keys on the pane's own path, which is what a late result for
        // a selection the user has left is checked against.
        let path = app.state.selected_path().expect("a selection");
        app.meta.path = path.clone();
        let sessions = vec![a_session("one")];
        app.sessions = Some(sessions.clone());

        app.redraw = false;
        app.handle_event(AppEvent::Sessions {
            path: path.clone(),
            sessions: Some(sessions),
        });
        assert!(!app.redraw, "an unchanged scan repainted");

        app.handle_event(AppEvent::Sessions {
            path,
            sessions: Some(vec![a_session("one"), a_session("two")]),
        });
        assert!(
            app.redraw,
            "a scan that found a new session did not repaint"
        );
    }

    /// Each card lands on its own, so the fold replaces one entry and leaves
    /// the rest alone — and a session that took a turn while we were away
    /// becomes the newest, which is where the list puts it.
    #[tokio::test]
    async fn a_refreshed_session_replaces_its_own_card_and_reorders() {
        let (_dir, mut app) = app_with_fixture();
        let path = app.state.selected_path().expect("a selection");
        app.meta.path = path.clone();
        let mut older = a_session("one");
        older.last_timestamp = chrono::Utc::now() - chrono::Duration::hours(2);
        app.sessions = Some(vec![a_session("two"), older]);
        app.sessions_refreshing.insert("one".to_string());

        let mut moved_on = a_session("one");
        moved_on.title = "renamed while we were away".to_string();
        moved_on.message_count = 99;
        app.handle_event(AppEvent::SessionRefreshed {
            path: path.clone(),
            id: "one".to_string(),
            session: Box::new(Some(moved_on)),
        });

        assert!(app.sessions_refreshing.is_empty(), "spinner never cleared");
        let list = app.sessions.as_ref().expect("sessions");
        assert_eq!(list.len(), 2, "a refresh must not add or drop cards");
        assert_eq!(list[0].id, "one", "the session that just moved is newest");
        assert_eq!(list[0].title, "renamed while we were away");
        assert_eq!(list[1].id, "two", "the other card is untouched");
    }

    /// A transcript deleted while forestui was in the background leaves a card
    /// whose Resume button points at nothing.
    #[tokio::test]
    async fn a_session_that_vanished_loses_its_card() {
        let (_dir, mut app) = app_with_fixture();
        let path = app.state.selected_path().expect("a selection");
        app.meta.path = path.clone();
        app.sessions = Some(vec![a_session("one"), a_session("two")]);
        app.sessions_refreshing.insert("one".to_string());

        app.handle_event(AppEvent::SessionRefreshed {
            path,
            id: "one".to_string(),
            session: Box::new(None),
        });

        let list = app.sessions.as_ref().expect("sessions");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "two");
    }

    fn a_live_window(session_id: &str, running: bool) -> crate::services::tmux::ClaudeWindow {
        crate::services::tmux::ClaudeWindow {
            window_id: "@7".into(),
            window_name: "claude:demo:wt".into(),
            running,
            session_id: session_id.into(),
        }
    }

    /// Resuming a session that already has a window must offer that window,
    /// not fork the conversation — running or suspended alike, because either
    /// way a second `claude -r` diverges from the same transcript.
    #[tokio::test]
    async fn resuming_an_open_session_offers_its_window_instead() {
        let (_dir, mut app) = app_with_fixture();
        let path = app.state.selected_path().expect("a selection");
        std::fs::create_dir_all(&path).expect("the selected path exists");
        app.sessions = Some(vec![a_session("one")]);
        app.live_sessions
            .insert("one".into(), a_live_window("one", true));

        app.run_action(Action::ResumeSession(0));

        let Some(crate::modal::Modal::Confirm(confirm)) = app.modals.last() else {
            panic!("the guard did not open a dialog: {:?}", app.modals.last());
        };
        assert!(matches!(
            confirm.action,
            crate::modal::ConfirmAction::SwitchToWindow { .. }
        ));
        assert!(confirm.message.contains("claude:demo:wt"));

        // The suspended form guards too, with its own wording.
        app.modals.clear();
        app.live_sessions
            .insert("one".into(), a_live_window("one", false));
        app.run_action(Action::ResumeYolo(0));
        let Some(crate::modal::Modal::Confirm(confirm)) = app.modals.last() else {
            panic!("the suspended guard did not open a dialog");
        };
        assert!(confirm.message.contains("up arrow"), "{}", confirm.message);
    }

    /// Deleting a session asks first and really deletes after — and refuses
    /// outright while a window holds the transcript open.
    #[tokio::test]
    async fn deleting_a_session_confirms_then_folds_the_result() {
        let (_dir, mut app) = app_with_fixture();
        let path = app.state.selected_path().expect("a selection");
        app.meta.path = path.clone();
        app.sessions = Some(vec![a_session("one")]);
        app.state.toggle_pin(&path, "one");

        // Open in a window: refused with a warning, no dialog.
        app.live_sessions
            .insert("one".into(), a_live_window("one", true));
        app.run_action(Action::DeleteSession(0));
        assert!(app.modals.is_empty(), "a live session must not confirm");
        assert!(
            app.notifications
                .iter()
                .any(|n| n.text.contains("close it first"))
        );

        // Not open: the confirm carries the session, and the fold removes the
        // card, drops the pin and announces.
        app.live_sessions.clear();
        app.run_action(Action::DeleteSession(0));
        let Some(crate::modal::Modal::Confirm(confirm)) = app.modals.last() else {
            panic!("no confirmation dialog");
        };
        assert!(matches!(
            confirm.action,
            crate::modal::ConfirmAction::DeleteClaudeSession { .. }
        ));

        app.handle_event(AppEvent::SessionDeleted {
            path: path.clone(),
            session_id: "one".into(),
            title: "session one".into(),
            result: Ok(()),
        });
        assert!(app.sessions.as_ref().is_some_and(Vec::is_empty));
        assert!(!app.state.is_pinned(&path, "one"), "the pin outlived it");
        assert!(
            app.notifications
                .iter()
                .any(|n| n.text.contains("Deleted session"))
        );

        // A failed delete keeps the card and says why.
        app.sessions = Some(vec![a_session("two")]);
        app.handle_event(AppEvent::SessionDeleted {
            path,
            session_id: "two".into(),
            title: "session two".into(),
            result: Err("permission denied".into()),
        });
        assert_eq!(app.sessions.as_ref().map(Vec::len), Some(1));
        assert!(
            app.notifications
                .iter()
                .any(|n| n.text.contains("permission denied"))
        );
    }

    /// Pinning floats the card to the top; K/J rearrange within the pinned
    /// block, from whichever of the card's controls holds the cursor.
    #[tokio::test]
    async fn pins_reorder_the_list_and_follow_the_keys() {
        let (_dir, mut app) = app_with_fixture();
        let path = app.state.selected_path().expect("a selection");
        app.meta.path = path.clone();
        let mut old = a_session("old");
        old.last_timestamp = chrono::Utc::now() - chrono::Duration::hours(10);
        app.sessions = Some(vec![a_session("new"), old]);

        // Pin the older session: it leads the list.
        app.run_action(Action::TogglePinSession(1));
        assert!(app.state.is_pinned(&path, "old"));
        assert_eq!(app.sessions.as_ref().unwrap()[0].id, "old");

        // Pin the other one too (now at index 1) and move it up with K.
        app.run_action(Action::TogglePinSession(1));
        assert_eq!(app.state.pinned_for(&path), vec!["old", "new"]);

        // The cursor sits on a control of the second pinned card.
        app.drawn_items = detail::drawn(&detail::content(&app));
        let second_card_control = app
            .drawn_items
            .iter()
            .position(|(item, _)| *item == DetailItem::Action(Action::ResumeSession(1)))
            .expect("the second card renders");
        app.focus = Focus::Detail;
        app.detail_index = second_card_control;

        app.handle_key(key(KeyCode::Char('K')));
        assert_eq!(app.state.pinned_for(&path), vec!["new", "old"]);
        assert_eq!(app.sessions.as_ref().unwrap()[0].id, "new");

        // The cursor rode along: it now points at the same control on the
        // card's new position, one card-width up.
        let per_card = 5 + app.settings.custom_buttons.len();
        assert_eq!(app.detail_index, second_card_control - per_card);

        // On an unpinned card the key falls through to the (absent) binding.
        app.sessions.as_mut().unwrap().push(a_session("loose"));
        app.drawn_items = detail::drawn(&detail::content(&app));
        let loose_control = app
            .drawn_items
            .iter()
            .position(|(item, _)| *item == DetailItem::Action(Action::ResumeSession(2)))
            .expect("the loose card renders");
        app.detail_index = loose_control;
        app.handle_key(key(KeyCode::Char('J')));
        assert_eq!(app.state.pinned_for(&path), vec!["new", "old"]);
    }

    /// The live scan's fold replaces the maps and repaints only on change.
    #[tokio::test]
    async fn the_live_scan_folds_quietly_when_nothing_changed() {
        let (_dir, mut app) = app_with_fixture();
        let windows = vec![a_live_window("one", true)];
        let peeks: std::collections::HashMap<String, Vec<String>> =
            [("one".to_string(), vec!["$ cargo test".to_string()])]
                .into_iter()
                .collect();

        app.redraw = false;
        app.handle_event(AppEvent::LiveSessions {
            windows: windows.clone(),
            peeks: peeks.clone(),
        });
        assert!(app.redraw, "the first result changes the frame");
        assert_eq!(app.live_sessions.len(), 1);

        app.redraw = false;
        app.handle_event(AppEvent::LiveSessions {
            windows,
            peeks: peeks.clone(),
        });
        assert!(!app.redraw, "an identical sweep must not repaint");

        // A closed window disappears from the map wholesale.
        app.handle_event(AppEvent::LiveSessions {
            windows: Vec::new(),
            peeks: std::collections::HashMap::new(),
        });
        assert!(app.live_sessions.is_empty());
        assert!(app.session_peeks.is_empty());
    }

    /// A late answer for a selection the user has already left must not put
    /// another path's conversation into this pane.
    #[tokio::test]
    async fn a_refresh_for_another_path_is_dropped() {
        let (_dir, mut app) = app_with_fixture();
        app.sessions = Some(vec![a_session("one")]);
        app.sessions_refreshing.insert("one".to_string());

        app.handle_event(AppEvent::SessionRefreshed {
            path: "/somewhere/else".to_string(),
            id: "one".to_string(),
            session: Box::new(Some(a_session("intruder"))),
        });

        let list = app.sessions.as_ref().expect("sessions");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "one", "another path's result was folded in");
        assert!(
            app.sessions_refreshing.is_empty(),
            "the spinner must clear even for a result that is dropped"
        );
    }

    #[tokio::test]
    async fn focus_return_and_the_tick_kick_a_sweep() {
        let (_dir, mut app) = app_with_fixture();

        app.handle_event(AppEvent::Term(
            ratatui::crossterm::event::Event::FocusGained,
        ));
        assert!(
            !app.worktree_scans_in_flight.is_empty(),
            "focus return did not start a scan"
        );

        app.worktree_scans_in_flight.clear();
        app.last_worktree_scan = Instant::now() - WORKTREE_SCAN_INTERVAL;
        app.handle_event(AppEvent::Tick);
        assert!(
            !app.worktree_scans_in_flight.is_empty(),
            "the periodic sweep did not start a scan"
        );
    }

    /// When a scan corrects the branch of the worktree on screen, the rename
    /// input must be reseeded — otherwise Enter renames the branch straight
    /// back to the stale value it still shows.
    #[tokio::test]
    async fn branch_drift_on_the_selected_worktree_reseeds_the_rename_input() {
        let (_dir, mut app) = app_with_fixture();
        let repo_id = app.state.repositories()[0].id;
        app.handle_key(key(KeyCode::Down));
        let worktree_id = app.state.selection.worktree_id.expect("a worktree");

        let mut listed = scan_listing(&app, &[]);
        let path = app
            .state
            .find_worktree(worktree_id)
            .map(|(_, w)| w.path.clone())
            .expect("the selected worktree");
        for info in &mut listed {
            if info.path == path {
                info.branch = Some("feat/drifted".into());
            }
        }

        // The pane's async content must survive: the path did not change, so
        // a full reload here would only reset the reader's scroll and drop
        // the sessions list back to Loading on every external checkout.
        app.sessions = Some(Vec::new());

        let event = scanned(&app, repo_id, listed);
        app.handle_event(event);
        assert_eq!(
            app.branch_input.value(),
            "feat/drifted",
            "the rename input still shows the branch git no longer has"
        );
        assert!(
            app.sessions.is_some(),
            "a branch drift reloaded the whole pane"
        );
    }

    /// A branch rename that git refuses must leave no trace. The name used to
    /// be written and persisted before `git branch -m` ran, so a collision left
    /// the config and the sidebar naming a branch that does not exist.
    #[tokio::test]
    async fn a_branch_rename_lands_only_when_git_agrees() {
        let (dir, mut app) = app_with_fixture();
        app.handle_key(key(KeyCode::Down));
        let worktree_id = app.state.selection.worktree_id.expect("a worktree");
        let original = app
            .state
            .find_worktree(worktree_id)
            .map(|(_, w)| w.branch.clone())
            .expect("a branch");

        // What the failing path does: report, and fold nothing.
        app.handle_event(AppEvent::Notify(
            "Branch rename failed: already exists".into(),
            Severity::Error,
        ));
        assert_eq!(
            app.state.find_worktree(worktree_id).map(|(_, w)| &w.branch),
            Some(&original),
            "the branch was renamed in state before git ran"
        );

        // And the success path folds through the event.
        app.handle_event(AppEvent::WorktreeBranchRenamed {
            worktree_id,
            branch: "feat/accepted".into(),
        });
        assert_eq!(
            app.state
                .find_worktree(worktree_id)
                .map(|(_, w)| w.branch.as_str()),
            Some("feat/accepted")
        );
        let reloaded = AppState::load_from(dir.path().join(".forestui-config.json"));
        assert!(
            reloaded
                .repositories()
                .iter()
                .flat_map(|r| &r.worktrees)
                .any(|w| w.branch == "feat/accepted"),
            "the accepted rename was never persisted"
        );
    }

    #[tokio::test]
    async fn a_scan_that_lands_late_does_not_steal_the_selection() {
        let (dir, mut app) = app_with_fixture();
        let repo_id = app.state.repositories()[0].id;

        // The user moved on to a worktree while the scan was still running.
        app.handle_key(key(KeyCode::Down));
        let selected = app.state.selection.worktree_id.expect("a worktree");
        app.sessions = Some(Vec::new());

        let adhoc = dir.path().join("late-adhoc");
        std::fs::create_dir_all(&adhoc).unwrap();
        let listed = scan_listing(&app, &[(&adhoc, "feat/late")]);
        let event = scanned(&app, repo_id, listed);
        app.handle_event(event);

        assert_eq!(
            app.state.selection.worktree_id,
            Some(selected),
            "the scan yanked the cursor back to the repository"
        );
        assert!(
            app.sessions.is_some(),
            "the pane the user was reading was reloaded out from under them"
        );
    }

    #[tokio::test]
    async fn stale_results_for_a_previous_selection_are_ignored() {
        let (_dir, mut app) = app_with_fixture();
        app.meta.path = "/current".into();

        app.handle_event(AppEvent::Sessions {
            path: "/stale".into(),
            sessions: Some(vec![]),
        });
        assert!(app.sessions.is_none(), "a late result must not land");

        app.handle_event(AppEvent::Sessions {
            path: "/current".into(),
            sessions: Some(vec![]),
        });
        assert!(app.sessions.is_some());
    }

    #[tokio::test]
    async fn a_failed_session_scan_keeps_what_the_cache_painted() {
        let (_dir, mut app) = app_with_fixture();
        app.meta.path = "/current".into();
        app.sessions = Some(vec![]);

        app.handle_event(AppEvent::Sessions {
            path: "/current".into(),
            sessions: None,
        });
        assert!(
            app.sessions.is_some(),
            "a panicked scan must not overwrite painted content"
        );
    }

    /// What the renderer does each frame: snapshot the drawn items that
    /// activation resolves against.
    fn snapshot_drawn(app: &mut App) {
        app.drawn_items = detail::drawn(&detail::content(app));
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

    fn wheel(column: u16, row: u16) -> ratatui::crossterm::event::MouseEvent {
        use ratatui::crossterm::event::{KeyModifiers, MouseEvent, MouseEventKind};
        MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    /// The footer is a row of buttons, and a click on one says which binding
    /// was meant. Replaying it as a keystroke let a focused rename field eat it
    /// as text — every footer button was dead while renaming.
    #[tokio::test]
    async fn a_footer_click_works_while_a_rename_field_has_focus() {
        let (_dir, mut app) = app_with_fixture();
        app.handle_key(key(KeyCode::Down));
        app.sessions = Some(Vec::new());
        snapshot_drawn(&mut app);

        app.focus = Focus::Detail;
        app.detail_index = app
            .detail_items()
            .iter()
            .position(|item| *item == DetailItem::Field(Field::WorktreeName))
            .expect("rename field present");
        let name = app.name_input.value().to_string();

        app.push_hit(rect(0, 40, 10, 1), HitTarget::FooterKey('s'));
        app.handle_mouse(click(2, 40));

        assert_eq!(app.modals.len(), 1, "the settings dialog never opened");
        assert_eq!(
            app.name_input.value(),
            name,
            "the click was typed into the rename field"
        );
    }

    #[tokio::test]
    async fn a_toast_swallows_the_clicks_it_covers() {
        let (_dir, mut app) = app_with_fixture();
        // A control is recorded first, then a toast is drawn over the same cell.
        app.push_hit(rect(10, 5, 20, 3), HitTarget::DetailItem(0));
        app.push_hit(rect(10, 5, 20, 3), HitTarget::Notification);

        assert_eq!(
            app.hit_at(12, 6),
            Some(HitTarget::Notification),
            "the toast must win the cells it covers, or the control under it runs"
        );
    }

    #[tokio::test]
    async fn an_open_modal_owns_the_wheel_as_well_as_the_click() {
        let (_dir, mut app) = app_with_fixture();
        app.sidebar_rect = rect(0, 0, 30, 20);
        let before = app.state.selection;

        // The panes are still drawn behind a modal, so they are still
        // hit-testable; only the guard stops the wheel reaching them.
        app.handle_key(key(KeyCode::Char('s')));
        assert_eq!(app.modals.len(), 1);
        app.handle_mouse(wheel(4, 5));

        assert_eq!(
            app.state.selection, before,
            "the wheel reselected the sidebar behind an open modal"
        );
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
        snapshot_drawn(&mut app);

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

    /// A control the frame drew as disabled must not fire — from the mouse or
    /// from Enter. The greyed-out `↓ Git Pull (No remote)` used to run `git
    /// pull` anyway and answer with an error toast.
    #[tokio::test]
    async fn a_disabled_control_does_not_activate() {
        let (_dir, mut app) = app_with_fixture();
        app.sessions = Some(Vec::new());
        app.issues = Some(Vec::new());
        // The fixture has no remote, so the sync control renders disabled.
        assert!(!app.meta.has_remote);
        snapshot_drawn(&mut app);

        let index = app
            .detail_items()
            .iter()
            .position(|item| *item == DetailItem::Action(Action::Sync))
            .expect("sync control present");
        assert!(!app.drawn_items[index].1, "sync should be drawn disabled");

        app.push_hit(rect(40, 5, 12, 1), HitTarget::DetailItem(index));
        app.handle_mouse(click(45, 5));
        assert!(app.notifications.is_empty(), "the click ran the action");

        app.focus = Focus::Detail;
        app.detail_index = index;
        app.handle_key(key(KeyCode::Enter));
        assert!(app.notifications.is_empty(), "Enter ran the action");
    }

    /// A background event drained in the same batch as a click must not remap
    /// the clicked index onto a different control: activation resolves against
    /// the snapshot of the frame the user saw.
    #[tokio::test]
    async fn a_click_resolves_against_the_drawn_frame_not_the_live_list() {
        let (_dir, mut app) = app_with_fixture();
        app.sessions = Some(Vec::new());
        app.issues = Some(Vec::new());
        snapshot_drawn(&mut app);

        // On the drawn frame (zero issues), this index is Remove Repository.
        let index = app
            .detail_items()
            .iter()
            .position(|item| *item == DetailItem::Action(Action::RemoveRepository))
            .expect("remove control present");
        app.push_hit(rect(40, 5, 20, 1), HitTarget::DetailItem(index));

        // Issues finish loading before the click is handled; the live list
        // grows and `index` would now point at CreateFromIssue(0).
        let path = app
            .state
            .selected_repository()
            .expect("repository selected")
            .source_path
            .clone();
        app.handle_event(AppEvent::Issues {
            path,
            issues: vec![
                serde_json::from_str(
                    r#"{"number":1,"title":"t","createdAt":"2026-01-01T00:00:00Z",
                        "updatedAt":"2026-01-01T00:00:00Z","labels":[]}"#,
                )
                .expect("issue fixture"),
            ],
        });
        assert_eq!(
            app.detail_items().get(index),
            Some(&DetailItem::Action(Action::CreateFromIssue(0))),
            "precondition: the live list did remap the index"
        );

        app.handle_mouse(click(45, 5));
        // Remove Repository opens the confirm modal; CreateFromIssue would
        // instead have started a branch load with no modal.
        assert!(
            matches!(app.modals.last(), Some(Modal::Confirm(_))),
            "the click fired the remapped control instead of the drawn one"
        );
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
        // Removal is asynchronous now; the click only dispatches it. The entry
        // goes when the git result is folded.
        assert!(app.state.find_worktree(worktree_id).is_some());
        app.handle_event(AppEvent::WorktreeRemoveResult {
            worktree_id,
            outcome: Ok(git::RemoveOutcome::Removed),
        });
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
                click: ModalClick::Cycle(Direction::Next),
            },
        );
        app.handle_mouse(click(3, 5));
        assert_eq!(app.modals.len(), 1, "clicking a cycle closed the dialog");
        let advanced = match app.modals.last() {
            Some(Modal::Settings(m)) => {
                assert_ne!(m.editor_index, before, "cycle did not advance");
                m.editor_index
            }
            _ => panic!("settings modal expected"),
        };

        // `Previous` steps back to exactly where it started — not merely
        // "somewhere else", which a second forward step would also satisfy.
        app.push_hit(
            rect(0, 5, 20, 1),
            HitTarget::ModalControl {
                index: 0,
                click: ModalClick::Cycle(Direction::Previous),
            },
        );
        app.handle_mouse(click(3, 5));
        match app.modals.last() {
            Some(Modal::Settings(m)) => assert_eq!(
                m.editor_index, before,
                "stepping back from {advanced} landed on {} instead of {before}",
                m.editor_index
            ),
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
        app.spinner_index = 0;

        // Nine idle ticks paint nothing; the tenth refreshes the on-screen
        // relative times, which have second granularity.
        for tick in 1..=10 {
            app.redraw = false;
            app.handle_event(AppEvent::Tick);
            if tick < 10 {
                assert!(!app.redraw, "idle tick {tick} repainted the app");
            } else {
                assert!(app.redraw, "the once-a-second refresh never fired");
            }
        }
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

        // Once the issues have landed there is nothing animating. Away from
        // the whole-second boundary, a loaded pane paints nothing.
        app.issues = Some(Vec::new());
        app.sessions = Some(Vec::new());
        app.notifications.clear();
        app.spinner_index = 1;
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

    /// The sidebar's branch column earns its space or does not appear.
    #[test]
    fn only_a_branch_that_differs_from_the_name_is_shown() {
        // What `w` creates: prefix + name, and so pure repetition.
        assert_eq!(informative_branch("feat/wt-a", "wt-a", "feat/"), None);
        // No prefix configured.
        assert_eq!(informative_branch("wt-a", "wt-a", ""), None);
        // An existing branch checked out under a different worktree name.
        assert_eq!(
            informative_branch("release-2", "hotfix", "feat/"),
            Some("release-2".to_string())
        );
        // Same name, different prefix — still worth showing.
        assert_eq!(
            informative_branch("bugfix/wt-a", "wt-a", "feat/"),
            Some("bugfix/wt-a".to_string())
        );
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
