//! Persisted application state: the repositories tracked in a forest.
//!
//! Stored as `.forestui-config.json` inside the forest directory itself, which
//! is what makes multiple independent forests possible.

use crate::models::{AppStateData, Repository, Selection, Worktree};
use crate::services::git::WorktreeInfo;
use crate::services::settings::get_forest_path;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// What a reconcile against `git worktree list` changed, for the toast.
/// Branch-only refreshes are counted so the caller knows a save happened,
/// but they are not worth announcing.
#[derive(Debug, Default)]
pub struct WorktreeReconcile {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    /// Worktrees whose recorded branch was corrected to what git reports.
    pub branch_updated: Vec<Uuid>,
}

impl WorktreeReconcile {
    fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.branch_updated.is_empty()
    }
}

/// The form git output and config entries can be compared in: `~` expanded,
/// symlinks resolved. Git records the fully resolved spelling, the config
/// records whatever the user typed; comparing raw strings would re-import
/// every tracked worktree under its other spelling.
///
/// When the path itself is gone — a deleted worktree is exactly what the
/// prune check feeds through here — resolving via the deepest existing
/// ancestor keeps the two spellings equal. Falling back to the unresolved
/// path instead made a symlinked forest compare a config entry against
/// git's resolved listing as *different* locations, and prune a row git
/// still tracked.
fn resolve(path: &str) -> PathBuf {
    let expanded = crate::util::expanduser(path);
    if let Ok(resolved) = std::fs::canonicalize(&expanded) {
        return resolved;
    }
    // Walk up to the deepest ancestor that still resolves and re-append the
    // missing components, so a deleted subtree keeps its resolved spelling.
    let mut missing = Vec::new();
    let mut current = expanded.as_path();
    while let (Some(parent), Some(name)) = (current.parent(), current.file_name()) {
        missing.push(name);
        if let Ok(resolved) = std::fs::canonicalize(parent) {
            return missing
                .iter()
                .rev()
                .fold(resolved, |path, part| path.join(part));
        }
        current = parent;
    }
    expanded
}

pub struct AppState {
    repositories: Vec<Repository>,
    /// Pinned Claude sessions per path, in pin order — see
    /// [`crate::models::AppStateData::pinned_sessions`].
    pinned_sessions: std::collections::HashMap<String, Vec<String>>,
    pub selection: Selection,
    pub show_archived: bool,
    config_path: PathBuf,
    /// Why the last [`AppState::save`] failed, if it did.
    ///
    /// A failed write used to be discarded outright, so the app went on to
    /// announce "Created worktree 'x'" for a worktree that never reached the
    /// config — the loudest possible lie, because a corrupt or missing file
    /// loads as *empty* state and the entry is simply gone on the next launch.
    /// Callers that announce success drain this first.
    save_error: Option<String>,
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
        let data = std::fs::read_to_string(&config_path)
            .ok()
            .and_then(|raw| serde_json::from_str::<AppStateData>(&raw).ok())
            .unwrap_or_default();

        Self {
            repositories: data.repositories,
            pinned_sessions: data.pinned_sessions,
            selection: Selection::default(),
            show_archived: false,
            config_path,
            save_error: None,
        }
    }

    fn save(&mut self) {
        if let Some(parent) = self.config_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let data = AppStateData {
            repositories: self.repositories.clone(),
            pinned_sessions: self.pinned_sessions.clone(),
        };
        self.save_error = match serde_json::to_string_pretty(&data) {
            Ok(json) => crate::util::write_atomically(&self.config_path, &json)
                .err()
                .map(|error| error.to_string()),
            Err(error) => Some(error.to_string()),
        };
    }

    /// Take the last save failure, if the most recent write did not land.
    ///
    /// Draining it means one failure is reported once, by whoever was about to
    /// claim the change succeeded.
    pub fn take_save_error(&mut self) -> Option<String> {
        self.save_error.take()
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
            // A reconcile scan can adopt this path in the window between git
            // creating the tree and the create task reporting back. The create
            // flow's entry wins — it carries the user's chosen name and base
            // branch — but the scan's must go, or the sidebar shows the same
            // worktree twice.
            let resolved = resolve(&worktree.path);
            repo.worktrees.retain(|w| resolve(&w.path) != resolved);
            repo.worktrees.push(worktree);
            self.save();
        }
    }

    /// Reconcile a repository's tracked worktrees against `git worktree list`.
    ///
    /// Git is the source of truth for which worktrees *exist*; the config only
    /// annotates them (name, archived flag, sort order). Listed paths the
    /// config has never seen are adopted, tracked paths git no longer lists
    /// are dropped, and a survivor's branch is corrected when it drifted —
    /// worktrees created, removed, or switched by anything else (an agent
    /// running `git worktree add`, a manual `git worktree remove`) all
    /// converge into the sidebar this way.
    ///
    /// Returns `None` — and does not save — when nothing changed, so the
    /// periodic scan is free while reality already matches.
    ///
    /// `protected` names worktrees with a removal or rename still in flight:
    /// the listing cannot be trusted about them (it may predate the mutation
    /// or observe it half-done), so they are neither pruned nor
    /// branch-corrected — their own fold will land the truth.
    pub fn reconcile_worktrees(
        &mut self,
        repo_id: Uuid,
        listed: &[WorktreeInfo],
        protected: &std::collections::HashSet<Uuid>,
    ) -> Option<WorktreeReconcile> {
        // Paths that belong to *other* tracked repositories (their checkouts
        // and their worktrees). `git worktree list` from a repository that is
        // itself a linked worktree prints its siblings too; adopting those
        // here would show one directory as two rows owned by two entries,
        // where deleting either destroys the other's tree.
        let foreign: std::collections::HashSet<PathBuf> = self
            .repositories
            .iter()
            .filter(|r| r.id != repo_id)
            .flat_map(|r| {
                std::iter::once(resolve(&r.source_path))
                    .chain(r.worktrees.iter().map(|w| resolve(&w.path)))
            })
            .collect();

        let repo = self.repositories.iter_mut().find(|r| r.id == repo_id)?;
        let source = resolve(&repo.source_path);

        // Drop the main checkout: it is the repository row, not a worktree.
        // Git prints it first — the parser keeps a bare repository's entry
        // for exactly this reason — and it only differs from `source` when
        // the "repository" the user added is itself a linked worktree, so
        // skipping by position covers that case too, instead of adopting the
        // real checkout as a deletable row.
        let listed: Vec<(PathBuf, &WorktreeInfo)> = listed
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != 0)
            .map(|(_, info)| (resolve(&info.path), info))
            .filter(|(resolved, _)| *resolved != source)
            .collect();
        let listed_by_path: std::collections::HashMap<&PathBuf, &WorktreeInfo> =
            listed.iter().map(|(path, info)| (path, *info)).collect();

        let mut outcome = WorktreeReconcile::default();

        // Each tracked path is canonicalized exactly once, keyed by id; the
        // loops below are lookups, not repeated realpath syscalls on the
        // event loop.
        let tracked: std::collections::HashMap<Uuid, PathBuf> = repo
            .worktrees
            .iter()
            .map(|w| (w.id, resolve(&w.path)))
            .collect();

        let mut removed_ids = Vec::new();
        for worktree in &repo.worktrees {
            let Some(resolved) = tracked.get(&worktree.id) else {
                continue;
            };
            // Unlisted alone is not enough to prune: the listing is a
            // snapshot, and a rename or create can fold in between git
            // running and this landing — the entry's directory existing is
            // the proof it is newer than the listing. A real removal
            // (`git worktree remove`) deletes the directory, so it still
            // prunes; a directory deleted by hand stays *listed* by git and
            // is not this case.
            if !protected.contains(&worktree.id)
                && !listed_by_path.contains_key(resolved)
                && !resolved.exists()
            {
                removed_ids.push(worktree.id);
                outcome.removed.push(worktree.name.clone());
            }
        }
        repo.worktrees.retain(|w| !removed_ids.contains(&w.id));

        for worktree in repo.worktrees.iter_mut() {
            if protected.contains(&worktree.id) {
                continue;
            }
            let Some(resolved) = tracked.get(&worktree.id) else {
                continue;
            };
            if let Some(info) = listed_by_path.get(resolved) {
                let branch = info.branch.clone().unwrap_or_else(|| "HEAD".to_string());
                if worktree.branch != branch {
                    worktree.branch = branch;
                    outcome.branch_updated.push(worktree.id);
                }
            }
        }

        let known: std::collections::HashSet<&PathBuf> = repo
            .worktrees
            .iter()
            .filter_map(|w| tracked.get(&w.id))
            .collect();
        for (resolved, info) in &listed {
            // A listed path whose directory is gone is a prunable stub —
            // adopting it would add a row whose every action fails.
            if known.contains(resolved) || foreign.contains(resolved) || !resolved.exists() {
                continue;
            }
            let base = Path::new(&info.path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| info.path.clone());
            // Names key tmux windows (`repo:name`), so two adoptions sharing
            // a basename would open and focus each other's editors; suffix
            // the later one instead.
            let mut name = base.clone();
            let mut suffix = 2;
            while repo.worktrees.iter().any(|w| w.name == name) {
                name = format!("{base}-{suffix}");
                suffix += 1;
            }
            let branch = info.branch.clone().unwrap_or_else(|| "HEAD".to_string());
            outcome.added.push(name.clone());
            repo.worktrees
                .push(Worktree::new(name, branch, info.path.clone()));
        }

        if outcome.is_empty() {
            return None;
        }
        // The pruned worktree can be the one on screen.
        if let Some(selected) = self.selection.worktree_id
            && removed_ids.contains(&selected)
        {
            self.selection.worktree_id = None;
        }
        self.save();
        Some(outcome)
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

    // -------------------------------------------------------- session pins

    /// Pinned session ids for a path, in pin order.
    pub fn pinned_for(&self, path: &str) -> Vec<String> {
        self.pinned_sessions.get(path).cloned().unwrap_or_default()
    }

    pub fn is_pinned(&self, path: &str, session_id: &str) -> bool {
        self.pinned_sessions
            .get(path)
            .is_some_and(|ids| ids.iter().any(|id| id == session_id))
    }

    /// Pin or unpin a session. Returns whether it is pinned afterwards.
    /// A new pin appends, so pin order is the order the user pinned in.
    pub fn toggle_pin(&mut self, path: &str, session_id: &str) -> bool {
        let ids = self.pinned_sessions.entry(path.to_string()).or_default();
        let pinned = if let Some(index) = ids.iter().position(|id| id == session_id) {
            ids.remove(index);
            false
        } else {
            ids.push(session_id.to_string());
            true
        };
        if self.pinned_sessions.get(path).is_some_and(Vec::is_empty) {
            // An empty list would sit in the config file forever.
            self.pinned_sessions.remove(path);
        }
        self.save();
        pinned
    }

    /// Drop a pin without toggling — for a session that was deleted.
    pub fn remove_pin(&mut self, path: &str, session_id: &str) {
        if let Some(ids) = self.pinned_sessions.get_mut(path) {
            let before = ids.len();
            ids.retain(|id| id != session_id);
            let now_empty = ids.is_empty();
            let changed = ids.len() != before;
            if now_empty {
                self.pinned_sessions.remove(path);
            }
            if changed {
                self.save();
            }
        }
    }

    /// Move a pinned session within the pinned order. Returns whether anything
    /// moved — unpinned sessions are ordered by recency and cannot be moved.
    pub fn move_pin(&mut self, path: &str, session_id: &str, delta: isize) -> bool {
        let Some(ids) = self.pinned_sessions.get_mut(path) else {
            return false;
        };
        let Some(index) = ids.iter().position(|id| id == session_id) else {
            return false;
        };
        let target = index as isize + delta;
        if target < 0 || target as usize >= ids.len() {
            return false;
        }
        ids.swap(index, target as usize);
        self.save();
        true
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

    fn info(path: &str, branch: Option<&str>) -> WorktreeInfo {
        WorktreeInfo {
            path: path.into(),
            head: "abc123".into(),
            branch: branch.map(str::to_string),
        }
    }

    fn unprotected() -> std::collections::HashSet<Uuid> {
        std::collections::HashSet::new()
    }

    /// The reconcile corrects a drifted branch, never adopts the main
    /// checkout as a worktree, and reports nothing when reality already
    /// matches.
    #[test]
    fn reconcile_updates_branches_and_skips_the_main_checkout() {
        let (dir, mut state) = temp_state();
        let source = dir.path().join("demo").to_string_lossy().to_string();
        let repo = Repository::new("demo".into(), source.clone());
        let repo_id = repo.id;
        state.add_repository(repo);
        let wt_path = dir.path().join("wt").to_string_lossy().to_string();
        state.add_worktree(
            repo_id,
            Worktree::new("wt".into(), "feat/old".into(), wt_path.clone()),
        );

        let listed = [
            info(&source, Some("main")),
            info(&wt_path, Some("feat/new")),
        ];
        let outcome = state
            .reconcile_worktrees(repo_id, &listed, &unprotected())
            .expect("a branch drift is a change");
        assert!(outcome.added.is_empty(), "adopted the main checkout");
        assert!(outcome.removed.is_empty());
        assert_eq!(outcome.branch_updated.len(), 1);
        assert_eq!(state.repositories()[0].worktrees[0].branch, "feat/new");

        assert!(
            state
                .reconcile_worktrees(repo_id, &listed, &unprotected())
                .is_none(),
            "an unchanged listing must be a silent no-op"
        );
    }

    /// An unlisted worktree whose directory still exists is kept: the listing
    /// is a snapshot, and a rename that folded while git ran must not have its
    /// result pruned by the stale scan.
    #[test]
    fn reconcile_keeps_unlisted_worktrees_whose_directory_exists() {
        let (dir, mut state) = temp_state();
        let source = dir.path().join("demo").to_string_lossy().to_string();
        let repo = Repository::new("demo".into(), source.clone());
        let repo_id = repo.id;
        state.add_repository(repo);
        let renamed = dir.path().join("just-renamed");
        std::fs::create_dir_all(&renamed).unwrap();
        state.add_worktree(
            repo_id,
            Worktree::new(
                "just-renamed".into(),
                "feat/r".into(),
                renamed.to_string_lossy().to_string(),
            ),
        );

        let listed = [info(&source, Some("main"))];
        assert!(
            state
                .reconcile_worktrees(repo_id, &listed, &unprotected())
                .is_none()
        );
        assert_eq!(state.repositories()[0].worktrees.len(), 1);
    }

    /// A listed path whose directory is gone is a prunable stub, not a row.
    #[test]
    fn reconcile_does_not_adopt_deleted_directories() {
        let (dir, mut state) = temp_state();
        let source = dir.path().join("demo").to_string_lossy().to_string();
        let repo = Repository::new("demo".into(), source.clone());
        let repo_id = repo.id;
        state.add_repository(repo);

        let ghost = dir.path().join("ghost").to_string_lossy().to_string();
        let listed = [info(&source, Some("main")), info(&ghost, Some("feat/g"))];
        assert!(
            state
                .reconcile_worktrees(repo_id, &listed, &unprotected())
                .is_none()
        );
        assert!(state.repositories()[0].worktrees.is_empty());
    }

    /// A protected worktree (removal or rename in flight) is neither pruned
    /// nor branch-corrected, whatever the listing claims.
    #[test]
    fn reconcile_leaves_protected_worktrees_alone() {
        let (dir, mut state) = temp_state();
        let source = dir.path().join("demo").to_string_lossy().to_string();
        let repo = Repository::new("demo".into(), source.clone());
        let repo_id = repo.id;
        state.add_repository(repo);
        let worktree = Worktree::new("wt".into(), "feat/current".into(), "/gone/wt".into());
        let worktree_id = worktree.id;
        state.add_worktree(repo_id, worktree);

        let protected = std::collections::HashSet::from([worktree_id]);
        // The listing neither contains the path nor could its directory
        // exist — exactly what a half-done removal looks like.
        let listed = [info(&source, Some("main"))];
        assert!(
            state
                .reconcile_worktrees(repo_id, &listed, &protected)
                .is_none()
        );
        assert!(state.find_worktree(worktree_id).is_some());

        // And a listed-but-drifted branch is not "corrected" either.
        let wt_dir = dir.path().join("wt");
        std::fs::create_dir_all(&wt_dir).unwrap();
        state.update_worktree(worktree_id, |w| {
            w.path = wt_dir.to_string_lossy().to_string()
        });
        let listed = [
            info(&source, Some("main")),
            info(&wt_dir.to_string_lossy(), Some("feat/mid-rename")),
        ];
        assert!(
            state
                .reconcile_worktrees(repo_id, &listed, &protected)
                .is_none()
        );
        assert_eq!(
            state
                .find_worktree(worktree_id)
                .map(|(_, w)| w.branch.as_str()),
            Some("feat/current")
        );
    }

    /// Listing entry zero is the main checkout even when it differs from the
    /// tracked source path — the "repository" the user added may itself be a
    /// linked worktree — and paths owned by other tracked repositories are
    /// never adopted as this one's worktrees.
    #[test]
    fn reconcile_does_not_adopt_the_main_checkout_or_other_repositories_trees() {
        let (dir, mut state) = temp_state();
        let real_main = dir.path().join("real-main");
        let linked = dir.path().join("linked-wt");
        let other_source = dir.path().join("other");
        let other_wt = dir.path().join("other-wt");
        for path in [&real_main, &linked, &other_source, &other_wt] {
            std::fs::create_dir_all(path).unwrap();
        }

        let other = Repository::new("other".into(), other_source.to_string_lossy().to_string());
        let other_id = other.id;
        state.add_repository(other);
        state.add_worktree(
            other_id,
            Worktree::new(
                "other-wt".into(),
                "feat/o".into(),
                other_wt.to_string_lossy().to_string(),
            ),
        );

        // The user added a linked worktree as a "repository": git lists the
        // real main checkout first, then the siblings.
        let pseudo = Repository::new("linked".into(), linked.to_string_lossy().to_string());
        let pseudo_id = pseudo.id;
        state.add_repository(pseudo);

        let listed = [
            info(&real_main.to_string_lossy(), Some("main")),
            info(&linked.to_string_lossy(), Some("feat/l")),
            info(&other_wt.to_string_lossy(), Some("feat/o")),
        ];
        assert!(
            state
                .reconcile_worktrees(pseudo_id, &listed, &unprotected())
                .is_none(),
            "adopted the main checkout or another repository's worktree"
        );
        assert!(state.repositories()[1].worktrees.is_empty());
    }

    /// The config may spell a path through a symlink while git records the
    /// resolved form. The two must compare equal even after the directory is
    /// deleted, or a by-hand `rm -rf` under a symlinked forest reads as
    /// unlisted-and-gone and prunes a row git still tracks.
    #[test]
    fn reconcile_matches_symlinked_spellings_of_a_deleted_worktree() {
        let (dir, mut state) = temp_state();
        let source = dir.path().join("demo").to_string_lossy().to_string();
        let real = dir.path().join("real-forest");
        std::fs::create_dir_all(&real).unwrap();
        let link = dir.path().join("linked-forest");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let repo = Repository::new("demo".into(), source.clone());
        let repo_id = repo.id;
        state.add_repository(repo);
        // Tracked through the symlink; the directory does not exist.
        state.add_worktree(
            repo_id,
            Worktree::new(
                "wt".into(),
                "feat/x".into(),
                link.join("wt").to_string_lossy().to_string(),
            ),
        );

        // Git lists the resolved spelling, still, as a prunable stub.
        let listed = [
            info(&source, Some("main")),
            info(&real.join("wt").to_string_lossy(), Some("feat/x")),
        ];
        assert!(
            state
                .reconcile_worktrees(repo_id, &listed, &unprotected())
                .is_none(),
            "the two spellings of one deleted worktree compared unequal"
        );
        assert_eq!(state.repositories()[0].worktrees.len(), 1);
    }

    /// Two adoptions sharing a directory basename get distinct names — the
    /// name keys the tmux windows, so a duplicate would focus the other
    /// worktree's editor.
    #[test]
    fn reconcile_uniquifies_colliding_adopted_names() {
        let (dir, mut state) = temp_state();
        let source = dir.path().join("demo").to_string_lossy().to_string();
        let repo = Repository::new("demo".into(), source.clone());
        let repo_id = repo.id;
        state.add_repository(repo);
        let a = dir.path().join("a").join("feature");
        let b = dir.path().join("b").join("feature");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();

        let listed = [
            info(&source, Some("main")),
            info(&a.to_string_lossy(), Some("feat/a")),
            info(&b.to_string_lossy(), Some("feat/b")),
        ];
        let outcome = state
            .reconcile_worktrees(repo_id, &listed, &unprotected())
            .expect("two adoptions");
        assert_eq!(outcome.added, vec!["feature", "feature-2"]);
    }

    /// The create flow's entry replaces a scan-adopted one for the same path
    /// rather than sitting next to it in the sidebar.
    #[test]
    fn add_worktree_replaces_a_scan_adopted_duplicate() {
        let (_dir, mut state) = temp_state();
        let repo = Repository::new("demo".into(), "/tmp/demo".into());
        let repo_id = repo.id;
        state.add_repository(repo);

        state.add_worktree(
            repo_id,
            Worktree::new("adopted".into(), "feat/x".into(), "/f/x".into()),
        );
        state.add_worktree(
            repo_id,
            Worktree::new("chosen-name".into(), "feat/x".into(), "/f/x".into()),
        );

        let worktrees = &state.repositories()[0].worktrees;
        assert_eq!(worktrees.len(), 1);
        assert_eq!(worktrees[0].name, "chosen-name");
    }

    /// Pins survive a relaunch and keep their order — that order *is* the
    /// feature: it is what the user arranged with K/J.
    #[test]
    fn session_pins_toggle_reorder_and_persist() {
        let (dir, mut state) = temp_state();
        let path = "/tmp/forest/demo/wt";

        assert!(state.toggle_pin(path, "a"));
        assert!(state.toggle_pin(path, "b"));
        assert!(state.toggle_pin(path, "c"));
        assert_eq!(state.pinned_for(path), vec!["a", "b", "c"]);
        assert!(state.is_pinned(path, "b"));

        // Move "c" up past "b"; the edges refuse silently.
        assert!(state.move_pin(path, "c", -1));
        assert_eq!(state.pinned_for(path), vec!["a", "c", "b"]);
        assert!(!state.move_pin(path, "a", -1));
        assert!(!state.move_pin(path, "b", 1));
        assert!(!state.move_pin(path, "unpinned", 1));

        // Unpinning removes; deleting a session drops its pin the same way.
        assert!(!state.toggle_pin(path, "c"));
        state.remove_pin(path, "b");
        assert_eq!(state.pinned_for(path), vec!["a"]);

        let reloaded = AppState::load_from(dir.path().join(".forestui-config.json"));
        assert_eq!(reloaded.pinned_for(path), vec!["a"]);
        assert!(reloaded.pinned_for("/other/path").is_empty());

        // Emptying a path's pins removes the key from the file entirely.
        let mut reloaded = reloaded;
        assert!(!reloaded.toggle_pin(path, "a"));
        let raw =
            std::fs::read_to_string(dir.path().join(".forestui-config.json")).unwrap_or_default();
        assert!(
            !raw.contains("/tmp/forest/demo/wt"),
            "an empty pin list lingers in the config: {raw}"
        );
    }

    /// The new field is additive: a config written before pins existed loads,
    /// and one written with pins still carries its repositories.
    #[test]
    fn configs_without_pins_load_cleanly() {
        let json = r#"{"repositories": []}"#;
        let data: crate::models::AppStateData = serde_json::from_str(json).unwrap();
        assert!(data.pinned_sessions.is_empty());
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
