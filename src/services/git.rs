//! Git operations, executed by shelling out to the `git` binary.
//!
//! Shelling out keeps the argv identical to the Textual build, so behaviour
//! (including how `git` resolves config, hooks and credentials) is unchanged.

use chrono::{DateTime, TimeZone, Utc};
use std::fmt;
use std::path::{Path, PathBuf};
use tokio::process::Command;

#[derive(Debug, Clone)]
pub struct GitError(pub String);

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for GitError {}

pub type GitResult<T> = Result<T, GitError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeInfo {
    pub path: String,
    pub head: String,
    pub branch: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CommitInfo {
    pub short_hash: String,
    pub timestamp: DateTime<Utc>,
}

/// Run a git command, returning `(exit_code, stdout, stderr)`.
///
/// Returns [`GitError`] when the process cannot be spawned at all — most
/// commonly a stale worktree whose directory has been deleted, which makes
/// `cwd` invalid.
async fn run_git(args: &[&str], cwd: Option<&Path>) -> GitResult<(i32, String, String)> {
    let mut cmd = Command::new("git");
    cmd.args(args);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let output = cmd.output().await.map_err(|e| {
        GitError(format!(
            "Failed to run git in {}: {e}",
            cwd.map(|p| p.display().to_string()).unwrap_or_default()
        ))
    })?;
    Ok((
        output.status.code().unwrap_or(0),
        String::from_utf8_lossy(&output.stdout).trim().to_string(),
        String::from_utf8_lossy(&output.stderr).trim().to_string(),
    ))
}

fn expand(path: &str) -> PathBuf {
    crate::util::expanduser(path)
}

pub async fn get_current_branch(path: &str) -> GitResult<String> {
    let dir = expand(path);
    let (code, stdout, stderr) = run_git(&["branch", "--show-current"], Some(&dir)).await?;
    if code != 0 {
        return Err(GitError(format!("Failed to get current branch: {stderr}")));
    }
    Ok(if stdout.is_empty() {
        "HEAD".to_string()
    } else {
        stdout
    })
}

pub async fn list_remotes(path: &str) -> GitResult<Vec<String>> {
    let dir = expand(path);
    let (code, stdout, stderr) = run_git(&["remote"], Some(&dir)).await?;
    if code != 0 {
        return Err(GitError(format!("Failed to list remotes: {stderr}")));
    }
    Ok(stdout
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect())
}

async fn safe_list_remotes(path: &str) -> Vec<String> {
    list_remotes(path).await.unwrap_or_default()
}

/// List branches. Remote branches keep their `<remote>/` prefix.
pub async fn list_branches(path: &str, include_remote: bool) -> GitResult<Vec<String>> {
    let dir = expand(path);
    let remotes = if include_remote {
        safe_list_remotes(path).await
    } else {
        Vec::new()
    };

    let (code, stdout, stderr) =
        run_git(&["branch", "-a", "--format=%(refname:short)"], Some(&dir)).await?;
    if code != 0 {
        return Err(GitError(format!("Failed to list branches: {stderr}")));
    }

    let mut branches = Vec::new();
    for raw in stdout.lines() {
        let line = raw.trim();
        if line.is_empty() || line.ends_with("/HEAD") {
            continue;
        }
        // Skip bare remote names (e.g. "origin" without a branch).
        if remotes.iter().any(|r| r == line) {
            continue;
        }
        let is_remote = remotes.iter().any(|r| line.starts_with(&format!("{r}/")));
        if is_remote {
            if include_remote {
                branches.push(line.to_string());
            }
        } else {
            branches.push(line.to_string());
        }
    }
    branches.sort();
    Ok(branches)
}

/// Create a new worktree.
///
/// * `new_branch` — create `branch` fresh, optionally stemming from `base_branch`.
/// * otherwise — check out an existing branch, creating a local tracking branch
///   when the given branch is a remote one, to avoid a detached HEAD.
pub async fn create_worktree(
    repo_path: &str,
    worktree_path: &Path,
    branch: &str,
    new_branch: bool,
    base_branch: Option<&str>,
) -> GitResult<()> {
    let repo = expand(repo_path);
    if let Some(parent) = worktree_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| GitError(format!("Failed to create worktree parent: {e}")))?;
    }
    let wt = worktree_path.to_string_lossy().to_string();
    let remotes = safe_list_remotes(repo_path).await;

    let (code, stderr) = if new_branch {
        let mut args = vec!["worktree", "add", "-b", branch, wt.as_str()];
        if let Some(base) = base_branch {
            args.push(base);
        }
        let (code, _out, stderr) = run_git(&args, Some(&repo)).await?;

        // A remote base can make git auto-set an upstream (branch.autoSetupMerge).
        // Unset it: new branches should be pushed with `git push -u origin <branch>`.
        if code == 0
            && let Some(base) = base_branch
            && remotes.iter().any(|r| base.starts_with(&format!("{r}/")))
        {
            let _ = run_git(&["branch", "--unset-upstream", branch], Some(worktree_path)).await;
        }
        (code, stderr)
    } else {
        let remote_prefix = remotes
            .iter()
            .find(|r| branch.starts_with(&format!("{r}/")))
            .map(|r| format!("{r}/"));

        if let Some(prefix) = remote_prefix {
            let local_branch = &branch[prefix.len()..];
            let (code, _out, stderr) = run_git(
                &[
                    "worktree",
                    "add",
                    "--track",
                    "-b",
                    local_branch,
                    wt.as_str(),
                    branch,
                ],
                Some(&repo),
            )
            .await?;
            (code, stderr)
        } else {
            let (code, _out, stderr) =
                run_git(&["worktree", "add", wt.as_str(), branch], Some(&repo)).await?;
            (code, stderr)
        }
    };

    if code != 0 {
        return Err(GitError(format!("Failed to create worktree: {stderr}")));
    }
    Ok(())
}

/// Remove a worktree, retrying with `--force` once if the plain remove fails.
pub async fn remove_worktree(repo_path: &str, worktree_path: &str) -> GitResult<()> {
    let repo = expand(repo_path);
    let wt = expand(worktree_path).to_string_lossy().to_string();

    let (code, _out, _err) = run_git(&["worktree", "remove", wt.as_str()], Some(&repo)).await?;
    if code == 0 {
        return Ok(());
    }
    let (code, _out, stderr) =
        run_git(&["worktree", "remove", "--force", wt.as_str()], Some(&repo)).await?;
    if code != 0 {
        return Err(GitError(format!("Failed to remove worktree: {stderr}")));
    }
    Ok(())
}

pub async fn rename_branch(path: &str, old_name: &str, new_name: &str) -> GitResult<()> {
    let dir = expand(path);
    let (code, _out, stderr) = run_git(&["branch", "-m", old_name, new_name], Some(&dir)).await?;
    if code != 0 {
        return Err(GitError(format!("Failed to rename branch: {stderr}")));
    }
    Ok(())
}

/// Repair worktree references after the directory was moved.
pub async fn repair_worktree(repo_path: &str, worktree_path: &Path) -> GitResult<()> {
    let repo = expand(repo_path);
    let wt = worktree_path.to_string_lossy().to_string();
    let (code, _out, stderr) = run_git(&["worktree", "repair", wt.as_str()], Some(&repo)).await?;
    if code != 0 {
        return Err(GitError(format!("Failed to repair worktree: {stderr}")));
    }
    Ok(())
}

pub async fn list_worktrees(repo_path: &str) -> GitResult<Vec<WorktreeInfo>> {
    let repo = expand(repo_path);
    let (code, stdout, stderr) = run_git(&["worktree", "list", "--porcelain"], Some(&repo)).await?;
    if code != 0 {
        return Err(GitError(format!("Failed to list worktrees: {stderr}")));
    }
    Ok(parse_worktree_porcelain(&stdout))
}

/// Parse `git worktree list --porcelain` output.
pub fn parse_worktree_porcelain(stdout: &str) -> Vec<WorktreeInfo> {
    let mut worktrees = Vec::new();
    let mut current_path: Option<String> = None;
    let mut current_head: Option<String> = None;
    let mut current_branch: Option<String> = None;

    let flush = |worktrees: &mut Vec<WorktreeInfo>,
                 path: &mut Option<String>,
                 head: &mut Option<String>,
                 branch: &mut Option<String>| {
        if let (Some(p), Some(h)) = (path.clone(), head.clone()) {
            worktrees.push(WorktreeInfo {
                path: p,
                head: h,
                branch: branch.clone(),
            });
        }
    };

    for raw in stdout.split('\n') {
        let line = raw.trim();
        if let Some(rest) = line.strip_prefix("worktree ") {
            flush(
                &mut worktrees,
                &mut current_path,
                &mut current_head,
                &mut current_branch,
            );
            current_path = Some(rest.to_string());
            current_head = None;
            current_branch = None;
        } else if let Some(rest) = line.strip_prefix("HEAD ") {
            current_head = Some(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("branch ") {
            current_branch = Some(rest.replace("refs/heads/", ""));
        } else if line.is_empty() && current_path.is_some() && current_head.is_some() {
            flush(
                &mut worktrees,
                &mut current_path,
                &mut current_head,
                &mut current_branch,
            );
            current_path = None;
            current_head = None;
            current_branch = None;
        }
    }
    flush(
        &mut worktrees,
        &mut current_path,
        &mut current_head,
        &mut current_branch,
    );
    worktrees
}

/// Short commit hash for a ref, or `None` when the ref cannot be resolved.
pub async fn get_ref(path: &str, reference: &str) -> Option<String> {
    let dir = expand(path);
    let (code, stdout, _err) = run_git(&["rev-parse", "--short", reference], Some(&dir))
        .await
        .ok()?;
    if code != 0 || stdout.trim().is_empty() {
        return None;
    }
    Some(stdout.trim().to_string())
}

pub async fn get_latest_commit(path: &str) -> GitResult<CommitInfo> {
    let dir = expand(path);
    let (code, stdout, stderr) = run_git(&["log", "-1", "--format=%H|%h|%ct"], Some(&dir)).await?;
    if code != 0 {
        return Err(GitError(format!("Failed to get latest commit: {stderr}")));
    }
    let parts: Vec<&str> = stdout.split('|').collect();
    if parts.len() != 3 {
        return Err(GitError("Unexpected git log output format".into()));
    }
    let secs: i64 = parts[2]
        .parse()
        .map_err(|_| GitError("Unexpected git log output format".into()))?;
    let timestamp = Utc
        .timestamp_opt(secs, 0)
        .single()
        .ok_or_else(|| GitError("Unexpected git log timestamp".into()))?;
    // parts[0] is the full hash; the UI only ever shows the short one.
    Ok(CommitInfo {
        short_hash: parts[1].to_string(),
        timestamp,
    })
}

pub async fn fetch(path: &str) -> GitResult<()> {
    let dir = expand(path);
    let (code, _out, stderr) = run_git(&["fetch"], Some(&dir)).await?;
    if code != 0 {
        return Err(GitError(format!("Failed to fetch: {stderr}")));
    }
    Ok(())
}

pub async fn pull(path: &str) -> GitResult<()> {
    let dir = expand(path);
    let (code, _out, stderr) = run_git(&["pull"], Some(&dir)).await?;
    if code != 0 {
        return Err(GitError(format!("Failed to pull: {stderr}")));
    }
    Ok(())
}

/// Whether the current branch has a remote tracking branch.
pub async fn has_remote_tracking(path: &str) -> GitResult<bool> {
    let dir = expand(path);
    let (code, stdout, _err) = run_git(
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
        Some(&dir),
    )
    .await?;
    Ok(code == 0 && !stdout.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stale worktree (deleted directory) must surface as GitError, not a panic.
    #[tokio::test]
    async fn missing_cwd_raises_git_error() {
        assert!(
            get_latest_commit("/nonexistent/stale/worktree")
                .await
                .is_err()
        );
        assert!(
            has_remote_tracking("/nonexistent/stale/worktree")
                .await
                .is_err()
        );
        assert!(
            get_current_branch("/nonexistent/stale/worktree")
                .await
                .is_err()
        );
    }

    #[test]
    fn porcelain_parsing() {
        let out = "worktree /repo\nHEAD abc123\nbranch refs/heads/main\n\
                   \nworktree /repo/wt\nHEAD def456\nbranch refs/heads/feat/x\n\
                   \nworktree /repo/detached\nHEAD 000111\ndetached\n";
        let parsed = parse_worktree_porcelain(out);
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[0].path, "/repo");
        assert_eq!(parsed[0].branch.as_deref(), Some("main"));
        assert_eq!(parsed[1].branch.as_deref(), Some("feat/x"));
        assert_eq!(parsed[2].branch, None);
        assert_eq!(parsed[2].head, "000111");
    }

    #[tokio::test]
    async fn real_repo_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let repo_str = repo.to_string_lossy().to_string();

        for args in [
            vec!["init", "-b", "main"],
            vec!["config", "user.email", "t@example.com"],
            vec!["config", "user.name", "t"],
            vec!["commit", "--allow-empty", "-m", "init"],
        ] {
            let out = std::process::Command::new("git")
                .args(&args)
                .current_dir(&repo)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?} failed");
        }

        assert!(list_worktrees(&repo_str).await.is_ok());
        assert_eq!(get_current_branch(&repo_str).await.unwrap(), "main");
        assert!(
            list_branches(&repo_str, true)
                .await
                .unwrap()
                .contains(&"main".to_string())
        );
        assert!(get_latest_commit(&repo_str).await.is_ok());
        assert!(!has_remote_tracking(&repo_str).await.unwrap());
        assert!(list_remotes(&repo_str).await.unwrap().is_empty());

        let wt = dir.path().join("forest").join("wt1");
        create_worktree(&repo_str, &wt, "feat/x", true, None)
            .await
            .unwrap();
        assert!(wt.exists());
        assert!(
            list_branches(&repo_str, true)
                .await
                .unwrap()
                .contains(&"feat/x".to_string())
        );

        let listed = list_worktrees(&repo_str).await.unwrap();
        assert_eq!(listed.len(), 2);

        remove_worktree(&repo_str, &wt.to_string_lossy())
            .await
            .unwrap();
        assert!(!wt.exists());
    }
}
