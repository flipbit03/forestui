//! Persisted application state: the repositories tracked in a forest.
//!
//! Stored as `.forestui-config.json` inside the forest directory itself, which
//! is what makes multiple independent forests possible.

use crate::models::{AppStateData, Repository, Selection, Worktree};
use crate::services::settings::get_forest_path;
use std::path::PathBuf;
use uuid::Uuid;

pub struct AppState {
    repositories: Vec<Repository>,
    pub selection: Selection,
    pub show_archived: bool,
    config_path: PathBuf,
}

impl AppState {
    /// Load state for the active forest, creating the forest directory if needed.
    pub fn load() -> Self {
        let forest_dir = get_forest_path();
        let _ = std::fs::create_dir_all(&forest_dir);
        Self::load_from(forest_dir.join(".forestui-config.json"))
    }

    /// Load state from an explicit config path. Corrupt files are ignored, the
    /// same as the Textual build, so a bad file never blocks startup.
    pub fn load_from(config_path: PathBuf) -> Self {
        let repositories = std::fs::read_to_string(&config_path)
            .ok()
            .and_then(|raw| serde_json::from_str::<AppStateData>(&raw).ok())
            .map(|data| data.repositories)
            .unwrap_or_default();

        Self {
            repositories,
            selection: Selection::default(),
            show_archived: false,
            config_path,
        }
    }

    fn save(&self) {
        if let Some(parent) = self.config_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let data = AppStateData {
            repositories: self.repositories.clone(),
        };
        if let Ok(json) = serde_json::to_string_pretty(&data) {
            let _ = std::fs::write(&self.config_path, json);
        }
    }

    pub fn repositories(&self) -> &[Repository] {
        &self.repositories
    }

    pub fn add_repository(&mut self, repository: Repository) {
        self.repositories.push(repository);
        self.save();
    }

    pub fn remove_repository(&mut self, repo_id: Uuid) {
        self.repositories.retain(|r| r.id != repo_id);
        if self.selection.repository_id == Some(repo_id) {
            self.selection = Selection::default();
        }
        self.save();
    }

    pub fn find_repository(&self, repo_id: Uuid) -> Option<&Repository> {
        self.repositories.iter().find(|r| r.id == repo_id)
    }

    /// Find a worktree and the repository that owns it.
    pub fn find_worktree(&self, worktree_id: Uuid) -> Option<(&Repository, &Worktree)> {
        self.repositories.iter().find_map(|repo| {
            repo.worktrees
                .iter()
                .find(|w| w.id == worktree_id)
                .map(|w| (repo, w))
        })
    }

    pub fn add_worktree(&mut self, repo_id: Uuid, worktree: Worktree) {
        if let Some(repo) = self.repositories.iter_mut().find(|r| r.id == repo_id) {
            repo.worktrees.push(worktree);
            self.save();
        }
    }

    /// Add a batch of worktrees with a single save, for the import flow.
    pub fn add_worktrees(&mut self, repo_id: Uuid, worktrees: Vec<Worktree>) {
        if let Some(repo) = self.repositories.iter_mut().find(|r| r.id == repo_id) {
            repo.worktrees.extend(worktrees);
            self.save();
        }
    }

    pub fn remove_worktree(&mut self, worktree_id: Uuid) {
        for repo in &mut self.repositories {
            repo.worktrees.retain(|w| w.id != worktree_id);
        }
        if self.selection.worktree_id == Some(worktree_id) {
            self.selection = Selection {
                repository_id: self.selection.repository_id,
                worktree_id: None,
            };
        }
        self.save();
    }

    /// Apply a mutation to a worktree and persist.
    pub fn update_worktree<F: FnOnce(&mut Worktree)>(&mut self, worktree_id: Uuid, edit: F) {
        for repo in &mut self.repositories {
            if let Some(worktree) = repo.worktrees.iter_mut().find(|w| w.id == worktree_id) {
                edit(worktree);
                self.save();
                return;
            }
        }
    }

    pub fn set_archived(&mut self, worktree_id: Uuid, archived: bool) {
        self.update_worktree(worktree_id, |w| w.is_archived = archived);
    }

    pub fn select_repository(&mut self, repo_id: Uuid) {
        self.selection = Selection {
            repository_id: Some(repo_id),
            worktree_id: None,
        };
    }

    pub fn select_worktree(&mut self, repo_id: Uuid, worktree_id: Uuid) {
        self.selection = Selection {
            repository_id: Some(repo_id),
            worktree_id: Some(worktree_id),
        };
    }

    pub fn selected_repository(&self) -> Option<&Repository> {
        self.selection
            .repository_id
            .and_then(|id| self.find_repository(id))
    }

    pub fn selected_worktree(&self) -> Option<(&Repository, &Worktree)> {
        self.selection
            .worktree_id
            .and_then(|id| self.find_worktree(id))
    }

    pub fn has_archived_worktrees(&self) -> bool {
        self.repositories
            .iter()
            .any(|r| r.worktrees.iter().any(|w| w.is_archived))
    }

    /// Path of the currently selected repository or worktree.
    pub fn selected_path(&self) -> Option<String> {
        if let Some((_repo, worktree)) = self.selected_worktree() {
            return Some(worktree.path.clone());
        }
        self.selected_repository().map(|r| r.source_path.clone())
    }

    /// tmux window name for a path: `repo:worktree`, or just `repo`.
    pub fn tmux_window_name(&self, path: &str) -> String {
        for repo in &self.repositories {
            for worktree in &repo.worktrees {
                if worktree.path == path {
                    return format!("{}:{}", repo.name, worktree.name);
                }
            }
            if repo.source_path == path {
                return repo.name.clone();
            }
        }
        "session".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_state() -> (tempfile::TempDir, AppState) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".forestui-config.json");
        (dir, AppState::load_from(path))
    }

    #[test]
    fn add_and_persist_roundtrip() {
        let (dir, mut state) = temp_state();
        let repo = Repository::new("demo".into(), "/tmp/demo".into());
        let repo_id = repo.id;
        state.add_repository(repo);
        state.add_worktree(
            repo_id,
            Worktree::new("wt".into(), "feat/x".into(), "/f/wt".into()),
        );

        let reloaded = AppState::load_from(dir.path().join(".forestui-config.json"));
        assert_eq!(reloaded.repositories().len(), 1);
        assert_eq!(reloaded.repositories()[0].worktrees.len(), 1);
        assert_eq!(reloaded.repositories()[0].worktrees[0].branch, "feat/x");
    }

    #[test]
    fn corrupt_config_loads_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".forestui-config.json");
        std::fs::write(&path, "{not json").unwrap();
        assert!(AppState::load_from(path).repositories().is_empty());
    }

    #[test]
    fn removing_selection_clears_it() {
        let (_dir, mut state) = temp_state();
        let repo = Repository::new("demo".into(), "/tmp/demo".into());
        let repo_id = repo.id;
        state.add_repository(repo);
        let worktree = Worktree::new("wt".into(), "b".into(), "/f/wt".into());
        let wt_id = worktree.id;
        state.add_worktree(repo_id, worktree);

        state.select_worktree(repo_id, wt_id);
        state.remove_worktree(wt_id);
        assert_eq!(state.selection.worktree_id, None);
        assert_eq!(state.selection.repository_id, Some(repo_id));

        state.remove_repository(repo_id);
        assert_eq!(state.selection, Selection::default());
    }

    #[test]
    fn tmux_window_names() {
        let (_dir, mut state) = temp_state();
        let repo = Repository::new("demo".into(), "/tmp/demo".into());
        let repo_id = repo.id;
        state.add_repository(repo);
        state.add_worktree(
            repo_id,
            Worktree::new("wt".into(), "b".into(), "/f/wt".into()),
        );

        assert_eq!(state.tmux_window_name("/f/wt"), "demo:wt");
        assert_eq!(state.tmux_window_name("/tmp/demo"), "demo");
        assert_eq!(state.tmux_window_name("/elsewhere"), "session");
    }

    #[test]
    fn archive_toggle_persists() {
        let (dir, mut state) = temp_state();
        let repo = Repository::new("demo".into(), "/tmp/demo".into());
        let repo_id = repo.id;
        state.add_repository(repo);
        let worktree = Worktree::new("wt".into(), "b".into(), "/f/wt".into());
        let wt_id = worktree.id;
        state.add_worktree(repo_id, worktree);

        state.set_archived(wt_id, true);
        assert!(state.has_archived_worktrees());
        let reloaded = AppState::load_from(dir.path().join(".forestui-config.json"));
        assert!(reloaded.repositories()[0].worktrees[0].is_archived);
    }
}
