//! Actions and the flows behind them: running a control, folding modal
//! results into state, and the git/tmux work they spawn.
//!
//! Anything that may take time runs in a `tokio::spawn` with a cloned
//! `EventTx`; results come back as events, and only the main loop ever
//! writes the state file.

use super::{Action, App, Field};
use crate::event::Severity;
use crate::event::{AppEvent, BranchTarget};
use crate::modal::ModalOutcome;
use crate::modal::{
    AddWorktreeModal, ConfirmAction, ConfirmModal, CreateFromIssueModal, Modal, ModalEffect,
    ModalResult,
};
use crate::models::{CustomClaudeButton, Repository, Worktree};
use crate::services::{claude_session, git, github, settings as settings_service, tmux};
use std::path::{Path, PathBuf};
use uuid::Uuid;

impl App {
    // ------------------------------------------------------------------- modals

    pub(super) fn handle_modal_key(&mut self, key: ratatui::crossterm::event::KeyEvent) {
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
                    // State was already updated and saved before the spawn; the
                    // pane just needs to re-read sessions and meta for the new path.
                    tx.send(AppEvent::ReloadDetail);
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
                        Ok(()) => tx.send(AppEvent::ReloadDetail),
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

        tokio::spawn(async move {
            let listed = match git::list_worktrees(&source_path).await {
                Ok(listed) => listed,
                Err(error) => {
                    tx.error(format!("Failed to import worktrees: {error}"));
                    return;
                }
            };

            let source_resolved = std::fs::canonicalize(&source_path).ok();
            let mut worktrees = Vec::new();

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
                worktrees.push(Worktree::new(name, branch, info.path.clone()));
            }

            // The main loop folds these into state, saves, and announces the
            // result; writing the config file from here would race a save the
            // user's next action makes, and announcing from here would toast a
            // success even when the fold drops a late result.
            tx.send(AppEvent::WorktreesImported { repo_id, worktrees });
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
