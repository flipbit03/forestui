//! Actions and the flows behind them: running a control, folding modal
//! results into state, and the git/tmux work they spawn.
//!
//! Anything that may take time runs in a `tokio::spawn` with a cloned
//! `EventTx`; results come back as events, and only the main loop ever
//! writes the state file.

use super::{Action, App, Field};
use crate::event::Severity;
use crate::event::{AppEvent, BranchTarget, EventTx};
use crate::modal::{
    AddWorktreeModal, ConfirmAction, ConfirmModal, CreateFromIssueModal, Modal, ModalEffect,
    ModalResult,
};
use crate::modal::{IntegrationAction, ModalOutcome};
use crate::models::{CustomClaudeButton, Repository, Worktree};
use crate::services::{claude_session, git, github, settings as settings_service, tmux};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Gather the branch and remote lists and deliver them to `target`.
///
/// The one implementation behind both the initial modal open and the
/// refetch-after-fetch inside it — what a branch load returns must not depend
/// on which of the two paths asked for it.
async fn load_branches(tx: EventTx, path: String, repo_id: Uuid, target: BranchTarget) {
    let branches = git::list_branches(&path).await.unwrap_or_default();
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
}

impl App {
    // ------------------------------------------------------------------- modals

    pub(super) fn handle_modal_key(&mut self, key: ratatui::crossterm::event::KeyEvent) {
        let Some(modal) = self.modals.last_mut() else {
            return;
        };
        match modal.handle_key(key) {
            ModalOutcome::None => {}
            ModalOutcome::Close => {
                let closed = self.modals.pop();
                // The integration dialog installs and removes the plugin, so
                // the Settings section that opened it has to re-read the status
                // it snapshotted — including on Esc, which reports nothing.
                if matches!(closed, Some(Modal::ClaudeIntegration(_)))
                    && let Some(Modal::Settings(parent)) = self.modals.last_mut()
                {
                    parent.integration_status = crate::services::claude_plugin::status();
                }
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

    /// Install, upgrade, overwrite or remove the plugin, then leave the dialog
    /// showing what happened.
    ///
    /// Synchronous on purpose: this writes three small files inside the user's
    /// own config directory, and a dialog that reported "Installed" from a
    /// background task could just as easily report it before the write landed.
    fn run_claude_plugin_action(&mut self, action: IntegrationAction) {
        use crate::services::claude_plugin;

        let outcome = match action {
            IntegrationAction::Install | IntegrationAction::Upgrade => {
                claude_plugin::install(false)
                    .map(|()| "Sessions started from now on are named after their tab.".to_string())
            }
            // Reinstall writes the same bytes back; overwrite is the only path
            // that discards a hand edit, and it is only reachable from a dialog
            // whose button says so.
            IntegrationAction::Reinstall => {
                claude_plugin::install(false).map(|()| "Shipped files rewritten.".to_string())
            }
            IntegrationAction::Overwrite => {
                claude_plugin::install(true).map(|()| "Local edits replaced.".to_string())
            }
            IntegrationAction::Uninstall => claude_plugin::uninstall()
                .map(|()| "Removed. Sessions already running keep their names.".to_string()),
            IntegrationAction::Close => return,
        };

        let message = match outcome {
            Ok(done) => {
                self.notify(done.clone(), Severity::Information);
                done
            }
            Err(error) => {
                self.notify(error.clone(), Severity::Error);
                error
            }
        };
        if let Some(Modal::ClaudeIntegration(modal)) = self.modals.last_mut() {
            modal.refresh(message);
        }
    }

    fn run_modal_effect(&mut self, effect: ModalEffect) {
        match effect {
            ModalEffect::ClaudePlugin(action) => self.run_claude_plugin_action(action),
            ModalEffect::Fetch(repo_path) => {
                let repo_id = self.state.selection.repository_id.unwrap_or_default();
                let tx = self.tx.clone();
                tokio::spawn(async move {
                    if let Err(error) = git::fetch(&repo_path).await {
                        tx.send(AppEvent::FetchFailed(error.to_string()));
                        return;
                    }
                    load_branches(tx, repo_path, repo_id, BranchTarget::RefetchOpenModal).await;
                });
            }
        }
    }

    fn apply_modal_result(&mut self, result: ModalResult) {
        match result {
            ModalResult::RepositoryAdded { path } => self.add_repository(path),
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
                // On every path that reaches Save the picker has already made
                // active == theme_name (commit applies, Esc reverts), so this
                // is not correction but ownership: the fold applies the state
                // it was handed, rather than trusting a dialog to have left
                // the global in the right place.
                crate::theme::set_active(&self.settings.theme_name);
                if let Err(error) = settings_service::save_settings(&self.settings) {
                    self.notify(format!("Could not save settings: {error}"), Severity::Error);
                } else {
                    self.notify("Settings saved", Severity::Information);
                }
                self.detail_index = 0;
                // The sidebar derives each row's branch suffix from
                // `branch_prefix`, so a changed prefix has to rebuild the rows
                // or they keep showing the old elision until something else does.
                self.rebuild_rows();
            }
            // The picker already applied the theme live and `receive_child`
            // carried the slug into the Settings dialog; persisting waits for
            // Save.
            ModalResult::CustomButtonsSaved(_)
            | ModalResult::CustomButtonSaved(_)
            | ModalResult::ThemeChosen(_) => {}
            ModalResult::Confirmed(action) => self.apply_confirmed(action),
            ModalResult::SessionRenamed {
                path,
                session_id,
                name,
                live_window_id,
            } => self.apply_session_rename(path, session_id, name, live_window_id),
        }
    }

    /// Run a committed session rename, live or stopped.
    fn apply_session_rename(
        &mut self,
        path: String,
        session_id: String,
        name: String,
        live_window_id: Option<String>,
    ) {
        match live_window_id {
            // Live: rename the tmux window and let the sync plugin adopt the
            // name into the session on its next hook — the same path a manual
            // tab rename takes, and the only safe one while Claude holds the
            // transcript open.
            Some(window_id) => {
                let tx = self.tx.clone();
                let shown = name.clone();
                tokio::spawn(async move {
                    let renamed = tokio::task::spawn_blocking(move || {
                        let renamed = tmux::rename_window_by_id(&window_id, &name);
                        // The badge on screen names the window; refresh it in
                        // the same breath so it does not spend the next sweep
                        // interval telling the user the name they just changed.
                        (renamed, tmux::list_claude_windows())
                    })
                    .await
                    .ok();
                    match renamed {
                        Some((true, windows)) => {
                            tx.info(format!(
                                "Window renamed to '{shown}' — the session follows on its next prompt"
                            ));
                            tx.send(AppEvent::LiveSessions { windows });
                        }
                        _ => tx.error("Could not rename the window"),
                    }
                });
            }
            // Stopped: append the same record `/rename` writes, then re-read
            // the transcript so the card shows the new name in the same fold
            // that announces it.
            None => {
                let tx = self.tx.clone();
                tokio::spawn(async move {
                    let (read_path, read_id, rename_name) =
                        (path.clone(), session_id.clone(), name.clone());
                    let result = tokio::task::spawn_blocking(move || {
                        claude_session::rename_session(&read_path, &read_id, &rename_name)
                    })
                    .await
                    .unwrap_or_else(|_| Err("the rename task panicked".to_string()));
                    let session = if result.is_ok() {
                        let (p, id) = (path.clone(), session_id.clone());
                        tokio::task::spawn_blocking(move || claude_session::refresh_one(&p, &id))
                            .await
                            .ok()
                            .flatten()
                    } else {
                        None
                    };
                    tx.send(AppEvent::SessionRenamed {
                        path,
                        session_id,
                        session: Box::new(session),
                        result,
                    });
                });
            }
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
                self.spawn_worktree_removal(worktree_id, false);
            }
            ConfirmAction::ForceDeleteWorktree(worktree_id) => {
                self.spawn_worktree_removal(worktree_id, true);
            }
            ConfirmAction::DeleteClaudeSession {
                path,
                session_id,
                title,
            } => {
                let tx = self.tx.clone();
                let (task_path, task_id) = (path.clone(), session_id.clone());
                tokio::spawn(async move {
                    let result = tokio::task::spawn_blocking(move || {
                        // The guard that disabled Del read a map up to a sweep
                        // old; a window opened since would lose its transcript
                        // out from under a running Claude. Ask tmux again at
                        // the moment of truth.
                        if let Some(window) = tmux::list_claude_windows()
                            .into_iter()
                            .find(|window| window.session_id == task_id)
                        {
                            return Err(format!("it is open in window '{}'", window.window_name));
                        }
                        claude_session::delete_session(&task_path, &task_id)
                    })
                    .await
                    .unwrap_or_else(|_| Err("the delete task panicked".to_string()));
                    tx.send(AppEvent::SessionDeleted {
                        path,
                        session_id,
                        title,
                        result,
                    });
                });
            }
            ConfirmAction::SwitchToWindow {
                window_id,
                window_name,
            } => {
                if tmux::select_window(&window_id) {
                    self.notify(format!("Switched to {window_name}"), Severity::Information);
                } else {
                    // The window closed between the scan and the click; the
                    // next sweep drops its badge.
                    self.notify(format!("Window {window_name} is gone"), Severity::Warning);
                    self.scan_live_sessions();
                }
            }
        }
    }

    /// Run a worktree removal in the background. The entry leaves state only
    /// when git has actually removed the worktree — the fold decides, so a
    /// dirty refusal can stop with everything intact.
    fn spawn_worktree_removal(&mut self, worktree_id: Uuid, force: bool) {
        let Some((repo, worktree)) = self.state.find_worktree(worktree_id) else {
            return;
        };
        // A second confirm on the same worktree while git is still running
        // would race a duplicate removal and stack a second destructive modal.
        if !self.removals_in_flight.insert(worktree_id) {
            return;
        }
        let repo_path = repo.source_path.clone();
        let worktree_path = worktree.path.clone();
        let name = worktree.name.clone();
        // Immediate feedback: on a large tree the round trip takes seconds and
        // the frame would otherwise be identical to the one before confirming.
        self.notify(format!("Deleting '{name}'…"), Severity::Information);
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let outcome = if force {
                git::force_remove_worktree(&repo_path, &worktree_path)
                    .await
                    .map(|()| git::RemoveOutcome::Removed)
            } else {
                git::remove_worktree(&repo_path, &worktree_path).await
            }
            .map_err(|error| error.to_string());
            tx.send(AppEvent::WorktreeRemoveResult {
                worktree_id,
                outcome,
            });
        });
    }

    pub(super) fn on_branches(
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
                let Some(issue) = self.pending_issue.take() else {
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
        tokio::spawn(load_branches(tx, path, repo_id, target));
    }

    pub(super) fn action_add_worktree(&mut self) {
        match self.state.selection.repository_id {
            Some(repo_id) => self.load_branches_then(repo_id, BranchTarget::AddWorktree),
            None => self.notify("Select a repository first", Severity::Warning),
        }
    }

    pub(super) fn action_toggle_archive(&mut self) {
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

    pub(super) fn action_delete(&mut self) {
        if let Some((_repo, worktree)) = self.state.selected_worktree() {
            let name = worktree.name.clone();
            let id = worktree.id;
            if self.removals_in_flight.contains(&id) {
                return;
            }
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

    fn session_at(&self, index: usize) -> Option<&crate::models::ClaudeSession> {
        self.sessions.as_ref().and_then(|s| s.get(index))
    }

    /// The duplicate-open guard. A resume aimed at a session that already has
    /// a window would start a second Claude on the same transcript — two
    /// conversations diverging from one history, exactly the mess the
    /// `@claude_session_id` stamp exists to prevent. Offer the window instead.
    /// Returns true when the guard took the action over.
    fn guard_open_session(&mut self, index: usize) -> bool {
        let Some(session) = self.session_at(index) else {
            return false;
        };
        let Some(window) = self.live_sessions.get(&session.id) else {
            return false;
        };
        let (window_id, window_name) = (window.window_id.clone(), window.window_name.clone());
        let message = if window.running {
            format!("This session is running in window '{window_name}'.\nSwitch to it?")
        } else {
            format!(
                "Claude exited, but its window '{window_name}' is still open —\n\
                 the up arrow there restarts it. Switch to it?"
            )
        };
        self.modals.push(Modal::Confirm(ConfirmModal::new(
            "Session Already Open",
            message,
            ConfirmAction::SwitchToWindow {
                window_id,
                window_name,
            },
        )));
        true
    }

    /// The name the user gave this session, if any. Resuming a named session
    /// names its window after the session, so the tab says which conversation
    /// it holds rather than repeating the worktree already selected on screen.
    fn session_window_name(&self, index: usize) -> Option<String> {
        self.sessions
            .as_ref()
            .and_then(|s| s.get(index))
            .and_then(|s| s.custom_title.clone())
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
            Action::ClaudeNew => self.start_claude(&path, None, false, None, None),
            Action::ClaudeYolo => self.start_claude(&path, None, true, None, None),
            Action::ClaudeCustom(index) => {
                if let Some(button) = self.custom_button(index) {
                    self.start_claude(&path, None, false, Some(button), None);
                }
            }
            Action::ResumeSession(index) => {
                if !self.guard_open_session(index)
                    && let Some(id) = self.session_id(index)
                {
                    let seed = self.session_window_name(index);
                    self.start_claude(&path, Some(&id), false, None, seed);
                }
            }
            Action::ResumeYolo(index) => {
                if !self.guard_open_session(index)
                    && let Some(id) = self.session_id(index)
                {
                    let seed = self.session_window_name(index);
                    self.start_claude(&path, Some(&id), true, None, seed);
                }
            }
            Action::ResumeCustom { button, session } => {
                if !self.guard_open_session(session)
                    && let (Some(id), Some(button)) =
                        (self.session_id(session), self.custom_button(button))
                {
                    let seed = self.session_window_name(session);
                    self.start_claude(&path, Some(&id), false, Some(button), seed);
                }
            }
            Action::RenameSession(index) => self.action_rename_session(index, &path),
            Action::TogglePinSession(index) => self.action_toggle_pin(index, &path),
            Action::DeleteSession(index) => self.action_delete_session(index, &path),
            Action::RefreshIssues => {
                // The cache is keyed by the *repository* path — `path` here is
                // the selected path, which for a worktree row would be the
                // worktree's directory and would invalidate nothing.
                if let Some(repo) = self.state.selected_repository() {
                    let repo_path = repo.source_path.clone();
                    github::invalidate_cache(&repo_path);
                    // The spinner is deliberate: the user asked for a refresh
                    // and gets feedback.
                    self.issues = None;
                    self.fetch_issues();
                }
            }
            Action::CreateFromIssue(index) => {
                // The issue is captured here rather than looked up again when
                // the branches land: `issues` is replaced wholesale by every
                // refresh and by selecting another repository, so an index held
                // across the branch load could resolve to a different issue —
                // or to another repository's issue entirely, which then built a
                // worktree in the wrong repository.
                if let Some(repo_id) = self.state.selection.repository_id
                    && let Some(issue) = self.issues.as_ref().and_then(|i| i.get(index)).cloned()
                {
                    self.pending_issue = Some(issue);
                    self.load_branches_then(repo_id, BranchTarget::CreateFromIssue);
                }
            }
            Action::RemoveRepository | Action::Delete => self.action_delete(),
            Action::Archive | Action::Unarchive => self.action_toggle_archive(),
        }
    }

    fn action_rename_session(&mut self, index: usize, path: &str) {
        let Some(session) = self.session_at(index) else {
            return;
        };
        let id = session.id.clone();
        // The dialog opens on the name the card shows, cursor at the end —
        // the way tmux's own rename prompt continues the existing name. For a
        // live session the window's current name is the truest form (the two
        // are one string by design); a stopped one falls back through the
        // chosen title to whatever the card is displaying, so the field is
        // never empty on a session that visibly has a name.
        let live = self.live_sessions.get(&id);
        let current = live
            .map(|window| window.window_name.clone())
            .or_else(|| session.custom_title.clone())
            .unwrap_or_else(|| session.title.clone());
        let live_window = live.map(|window| (window.window_id.clone(), window.window_name.clone()));
        self.modals
            .push(Modal::RenameSession(crate::modal::RenameSessionModal::new(
                path.to_string(),
                id,
                current,
                live_window,
            )));
    }

    fn action_toggle_pin(&mut self, index: usize, path: &str) {
        let Some(id) = self.session_id(index) else {
            return;
        };
        self.state.toggle_pin(path, &id);
        self.sort_visible_sessions();
        self.report_save_error();
    }

    fn action_delete_session(&mut self, index: usize, path: &str) {
        let Some(session) = self.session_at(index) else {
            return;
        };
        // The control is drawn disabled for a live session, but the drawn
        // snapshot can be a sweep older than the map — refuse here too.
        if let Some(window) = self.live_sessions.get(&session.id) {
            self.notify(
                format!(
                    "Session is open in window '{}' — close it first",
                    window.window_name
                ),
                Severity::Warning,
            );
            return;
        }
        let title = crate::util::truncate(&session.title, 40);
        let count = session.message_count;
        let msgs = if count == 1 { "message" } else { "messages" };
        self.modals.push(Modal::Confirm(ConfirmModal::new(
            "Delete Session",
            format!("Permanently delete '{title}' ({count} {msgs})?\nThere is no undo."),
            ConfirmAction::DeleteClaudeSession {
                path: path.to_string(),
                session_id: session.id.clone(),
                title: session.title.clone(),
            },
        )));
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
        seed_name: Option<String>,
    ) {
        let opening_name = tmux::opening_window_name(
            seed_name.as_deref(),
            &self.state.tmux_window_name(path),
            yolo,
            custom.as_ref().map(|b| b.prefix.as_str()),
        );
        let window = tmux::create_claude_window(
            &opening_name,
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

    pub(super) fn submit_rename(&mut self, field: Field) {
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

                // Everything that touches the filesystem — the existence
                // probe, the directory rename, the session-history migration,
                // the git repair — runs off the loop; a slow disk must not
                // freeze the UI mid-keystroke. The result comes back as an
                // event and only the main loop touches state.
                //
                // While it runs, a worktree scan must not act on what a
                // listing says about this entry: after `git worktree repair`
                // the listing shows the new path while the config still holds
                // the old one, which reads as "removed" to the reconcile.
                // Every exit below releases the protection by event.
                self.renames_in_flight.insert(worktree_id);
                let tx = self.tx.clone();
                tokio::spawn(async move {
                    if tokio::fs::try_exists(&new_path).await.unwrap_or(false) {
                        tx.error("Path already exists");
                        tx.send(AppEvent::WorktreeRenameAborted { worktree_id });
                        return;
                    }
                    if let Err(error) = tokio::fs::rename(&old_path, &new_path).await {
                        tx.error(format!("Rename failed: {error}"));
                        tx.send(AppEvent::WorktreeRenameAborted { worktree_id });
                        return;
                    }
                    let (migrate_old, migrate_new) = (old_path.clone(), new_path.clone());
                    let _ = tokio::task::spawn_blocking(move || {
                        claude_session::migrate_sessions(&migrate_old, &migrate_new);
                    })
                    .await;
                    // The directory has moved either way; a failed repair is
                    // reported but must not strand state on the old path.
                    if let Err(error) = git::repair_worktree(&repo_path, &new_path).await {
                        tx.error(format!("Rename failed: {error}"));
                    }
                    tx.send(AppEvent::WorktreeRenamed {
                        worktree_id,
                        name: new_name,
                        path: new_path.to_string_lossy().to_string(),
                    });
                });
            }
            Field::BranchName => {
                let new_branch = self.branch_input.value().to_string();
                if new_branch.is_empty() || new_branch == old_branch {
                    return;
                }
                let worktree_path = worktree.path.clone();
                // Reconcile-protected like the directory rename: a listing
                // captured mid-rename must not "correct" the branch either
                // direction while git is still deciding.
                self.renames_in_flight.insert(worktree_id);
                let tx = self.tx.clone();
                let requested = new_branch.clone();
                tokio::spawn(async move {
                    match git::rename_branch(&worktree_path, &old_branch, &requested).await {
                        // Folded on success only. Writing the name up front left
                        // the config — and the sidebar — permanently showing a
                        // branch git had refused to create.
                        Ok(()) => tx.send(AppEvent::WorktreeBranchRenamed {
                            worktree_id,
                            branch: requested,
                        }),
                        Err(error) => {
                            tx.error(format!("Branch rename failed: {error}"));
                            tx.send(AppEvent::WorktreeRenameAborted { worktree_id });
                        }
                    }
                });
            }
        }
    }

    fn add_repository(&mut self, path: String) {
        let name = Path::new(&path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.clone());
        let repo = Repository::new(name, path);
        let repo_id = repo.id;
        self.state.add_repository(repo);
        self.state.select_repository(repo_id);
        self.rebuild_rows();
        self.sync_sidebar_index();
        self.reload_detail();
        // Existing worktrees appear via the same reconcile scan that keeps
        // them fresh afterwards — there is no one-shot import to opt into.
        self.scan_worktrees(repo_id);
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

            let mut worktree = Worktree::new(
                name.clone(),
                branch,
                worktree_path.to_string_lossy().to_string(),
            );
            worktree.base_branch = base;
            worktree.created_from_ref = base_ref;

            tx.send(AppEvent::WorktreeAdded {
                repo_id,
                worktree: Box::new(worktree),
            });
        });
    }
}
