//! Installing the Claude Code plugin that names a session after its tmux window.
//!
//! The plugin is a directory forestui owns end to end: a manifest, a hooks
//! declaration and one shell script. Claude Code discovers plugin directories
//! under `<config>/skills/` on its own, so installing writes nothing into the
//! user's `settings.json` and cannot disturb hooks they configured themselves —
//! plugin hooks and settings hooks both run.
//!
//! Enabling and disabling is Claude's own business (`claude plugin
//! enable|disable`), which is why nothing here edits `enabledPlugins`.

use crate::util;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub const PLUGIN_NAME: &str = "forestui-tmux-title";

/// The files that make up the plugin, relative to its directory. Shipped inside
/// the binary so an install is never a download and never a partial checkout.
const ASSETS: &[(&str, &str)] = &[
    (
        ".claude-plugin/plugin.json",
        include_str!("../../assets/claude-plugin/plugin.json"),
    ),
    (
        "hooks/hooks.json",
        include_str!("../../assets/claude-plugin/hooks.json"),
    ),
    (
        "hooks/tmux-title.sh",
        include_str!("../../assets/claude-plugin/tmux-title.sh"),
    ),
];

/// The only file that has to be executable — Claude runs it as a command.
const EXECUTABLE: &str = "hooks/tmux-title.sh";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    NotInstalled,
    /// Every shipped file is present and byte-identical to this build's copy.
    Installed,
    /// Installed, but these files differ from what this build ships — either a
    /// hand edit or an older forestui. Never overwritten without being asked.
    Drifted(Vec<String>),
}

impl Status {
    pub fn label(&self) -> String {
        match self {
            Status::NotInstalled => "Not installed".to_string(),
            Status::Installed => "Installed".to_string(),
            Status::Drifted(files) => format!("Installed, {} file(s) modified", files.len()),
        }
    }
}

/// Where Claude Code keeps its configuration. `CLAUDE_CONFIG_DIR` wins so that
/// installing lands wherever Claude will actually read from.
pub fn claude_config_dir() -> PathBuf {
    match std::env::var("CLAUDE_CONFIG_DIR") {
        Ok(dir) if !dir.trim().is_empty() => util::expanduser(dir.trim()),
        _ => util::home_dir().join(".claude"),
    }
}

pub fn plugin_dir() -> PathBuf {
    claude_config_dir().join("skills").join(PLUGIN_NAME)
}

pub fn status() -> Status {
    status_in(&plugin_dir())
}

/// Directory-scoped form of [`status`], for testing.
pub fn status_in(dir: &Path) -> Status {
    if !dir.join(".claude-plugin/plugin.json").exists() {
        return Status::NotInstalled;
    }
    let drifted: Vec<String> = ASSETS
        .iter()
        .filter(|(rel, shipped)| {
            std::fs::read_to_string(dir.join(rel))
                .map(|found| digest(&found) != digest(shipped))
                .unwrap_or(true)
        })
        .map(|(rel, _)| (*rel).to_string())
        .collect();

    if drifted.is_empty() {
        Status::Installed
    } else {
        Status::Drifted(drifted)
    }
}

/// The paths an install touches, for showing the user before it happens.
pub fn planned_paths() -> Vec<PathBuf> {
    let dir = plugin_dir();
    ASSETS.iter().map(|(rel, _)| dir.join(rel)).collect()
}

pub fn install(overwrite_drift: bool) -> Result<(), String> {
    install_in(&plugin_dir(), overwrite_drift)
}

/// Directory-scoped form of [`install`], for testing.
pub fn install_in(dir: &Path, overwrite_drift: bool) -> Result<(), String> {
    if let Status::Drifted(files) = status_in(dir)
        && !overwrite_drift
    {
        return Err(format!(
            "{} was modified after forestui installed it ({}). Re-install to overwrite those edits.",
            dir.display(),
            files.join(", ")
        ));
    }

    for (rel, contents) in ASSETS {
        let target = dir.join(rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
        }
        util::write_atomically(&target, contents)
            .map_err(|e| format!("could not write {}: {e}", target.display()))?;
    }
    make_executable(&dir.join(EXECUTABLE))?;
    Ok(())
}

pub fn uninstall() -> Result<(), String> {
    uninstall_in(&plugin_dir())
}

/// Directory-scoped form of [`uninstall`], for testing.
///
/// Refuses to remove a directory that does not look like this plugin, so a
/// mis-set `CLAUDE_CONFIG_DIR` cannot turn an uninstall into a recursive
/// delete of something else.
pub fn uninstall_in(dir: &Path) -> Result<(), String> {
    if !dir.exists() {
        return Ok(());
    }
    let manifest = std::fs::read_to_string(dir.join(".claude-plugin/plugin.json"))
        .map_err(|_| format!("{} is not a forestui plugin directory", dir.display()))?;
    if !manifest.contains(PLUGIN_NAME) {
        return Err(format!(
            "{} holds a different plugin; refusing to remove it",
            dir.display()
        ));
    }
    std::fs::remove_dir_all(dir).map_err(|e| format!("could not remove {}: {e}", dir.display()))
}

fn digest(contents: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(contents.as_bytes());
    hasher.finalize().into()
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)
        .map_err(|e| format!("could not stat {}: {e}", path.display()))?
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms)
        .map_err(|e| format!("could not make {} executable: {e}", path.display()))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_then_status_then_uninstall() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("skills").join(PLUGIN_NAME);

        assert_eq!(status_in(&dir), Status::NotInstalled);
        install_in(&dir, false).expect("a clean install");
        assert_eq!(status_in(&dir), Status::Installed);

        // Installing twice is not an error and not a change.
        install_in(&dir, false).expect("re-install over an identical copy");
        assert_eq!(status_in(&dir), Status::Installed);

        uninstall_in(&dir).expect("uninstall");
        assert!(!dir.exists());
        // Uninstalling what is not there is a no-op, not a failure.
        uninstall_in(&dir).expect("second uninstall");
    }

    /// A hand-edited hook is the user's work. Reporting it and stopping is the
    /// whole point of hashing the files.
    #[test]
    fn a_hand_edited_file_is_drift_and_is_not_clobbered() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(PLUGIN_NAME);
        install_in(&dir, false).unwrap();

        let script = dir.join(EXECUTABLE);
        std::fs::write(&script, "#!/bin/sh\n# my own version\n").unwrap();

        match status_in(&dir) {
            Status::Drifted(files) => assert_eq!(files, vec![EXECUTABLE.to_string()]),
            other => panic!("expected drift, got {other:?}"),
        }

        let refused = install_in(&dir, false).expect_err("install must refuse to clobber");
        assert!(refused.contains(EXECUTABLE));
        assert!(
            std::fs::read_to_string(&script)
                .unwrap()
                .contains("my own version")
        );

        install_in(&dir, true).expect("an explicit overwrite is allowed");
        assert_eq!(status_in(&dir), Status::Installed);
    }

    #[test]
    fn uninstall_refuses_a_directory_it_does_not_own() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("someone-elses-plugin");
        std::fs::create_dir_all(dir.join(".claude-plugin")).unwrap();
        std::fs::write(
            dir.join(".claude-plugin/plugin.json"),
            "{\"name\":\"something-else\"}",
        )
        .unwrap();

        uninstall_in(&dir).expect_err("must refuse");
        assert!(dir.exists());
    }

    /// The hook is run as a command by Claude; a non-executable file is a
    /// silent no-op that would look exactly like the feature not working.
    #[cfg(unix)]
    #[test]
    fn the_hook_script_is_installed_executable() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(PLUGIN_NAME);
        install_in(&dir, false).unwrap();

        let mode = std::fs::metadata(dir.join(EXECUTABLE))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0o111, "owner/group/other execute bits");
    }
}
