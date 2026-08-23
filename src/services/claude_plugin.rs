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
    (
        "hooks/heartbeat.sh",
        include_str!("../../assets/claude-plugin/heartbeat.sh"),
    ),
];

/// The files that have to be executable — Claude runs them as commands.
const EXECUTABLES: &[&str] = &["hooks/tmux-title.sh", "hooks/heartbeat.sh"];

/// Bumped whenever a shipped file changes, so an install by an older forestui
/// is recognised as old rather than reported as the user having edited it.
const SHIPPED_VERSION: &str = "4";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    NotInstalled,
    /// Every shipped file is present and byte-identical to this build's copy.
    Installed,
    /// An older forestui installed this. Upgrading is not overwriting the
    /// user's work, so it needs no confirmation — which is the whole reason
    /// this is a separate state from drift.
    Outdated {
        installed: String,
    },
    /// Installed at this build's version, but these files differ from what it
    /// ships. That is a hand edit, and it is never overwritten unasked.
    Drifted(Vec<String>),
}

impl Status {
    pub fn label(&self) -> String {
        match self {
            Status::NotInstalled => "Not installed".to_string(),
            Status::Installed => "Installed".to_string(),
            Status::Outdated { installed } => {
                format!("Installed at version {installed}, this build ships {SHIPPED_VERSION}")
            }
            Status::Drifted(files) => format!("Installed, {} file(s) modified", files.len()),
        }
    }
}

/// The startup nag for an install an older forestui wrote, and only that.
///
/// `NotInstalled` is a choice the user never made and `Drifted` is their own
/// edit — neither is forestui's to warn about. Outdated is different: the
/// user opted in, and a newer build ships hooks their install lacks (the
/// liveness heartbeat arrived in version 4), so silence would look like the
/// feature not working.
pub fn upgrade_notice(status: &Status) -> Option<String> {
    match status {
        Status::Outdated { installed } => Some(format!(
            "Claude integration is outdated (v{installed}, this build ships v{SHIPPED_VERSION}) — \
             upgrade it in Settings > Claude Code Integration"
        )),
        _ => None,
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
    if installed_version(dir).as_deref() != Some(SHIPPED_VERSION) {
        return Status::Outdated {
            installed: installed_version(dir).unwrap_or_else(|| "unknown".to_string()),
        };
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

fn installed_version(dir: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(dir.join(".claude-plugin/plugin.json")).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&raw).ok()?;
    Some(parsed.get("version")?.as_str()?.to_string())
}

/// The paths an install touches, for showing the user before it happens.
pub fn planned_paths() -> Vec<PathBuf> {
    let dir = plugin_dir();
    ASSETS.iter().map(|(rel, _)| dir.join(rel)).collect()
}

/// The shipped files, relative to the plugin directory. Shown under the
/// directory rather than as absolute paths: three absolute paths that differ
/// only in their last component are three identical lines once a dialog has
/// truncated them.
pub fn asset_names() -> Vec<&'static str> {
    ASSETS.iter().map(|(rel, _)| *rel).collect()
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
    for rel in EXECUTABLES {
        make_executable(&dir.join(rel))?;
    }
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
    // Parsed, not substring-matched: this guards a recursive delete, and
    // "forestui-tmux-title-fork" contains our name without being ours.
    let named_ours = serde_json::from_str::<serde_json::Value>(&manifest)
        .ok()
        .and_then(|v| v.get("name")?.as_str().map(str::to_string))
        .is_some_and(|name| name == PLUGIN_NAME);
    if !named_ours {
        return Err(format!(
            "{} holds a different plugin; refusing to remove it",
            dir.display()
        ));
    }
    std::fs::remove_dir_all(dir).map_err(|e| format!("could not remove {}: {e}", dir.display()))
}

/// The hook reads the session's own name with `jq`. Without it the sync does
/// nothing at all rather than half of it, so this is worth saying out loud
/// wherever the plugin is installed.
pub fn jq_available() -> bool {
    std::process::Command::new("jq")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
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

    /// An install by an older forestui is not the user's work. Reporting it as
    /// drift would make every upgrade look like something to be careful about.
    #[test]
    fn an_older_install_is_outdated_not_drifted() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(PLUGIN_NAME);
        install_in(&dir, false).unwrap();

        let manifest = dir.join(".claude-plugin/plugin.json");
        let old = std::fs::read_to_string(&manifest)
            .unwrap()
            .replace(SHIPPED_VERSION, "1");
        std::fs::write(&manifest, old).unwrap();

        match status_in(&dir) {
            Status::Outdated { installed } => assert_eq!(installed, "1"),
            other => panic!("expected Outdated, got {other:?}"),
        }
        // Upgrading is not clobbering, so it needs no overwrite flag.
        install_in(&dir, false).expect("an upgrade installs without confirmation");
        assert_eq!(status_in(&dir), Status::Installed);
    }

    /// A hand-edited hook is the user's work. Reporting it and stopping is the
    /// whole point of hashing the files.
    #[test]
    fn a_hand_edited_file_is_drift_and_is_not_clobbered() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(PLUGIN_NAME);
        install_in(&dir, false).unwrap();

        let script = dir.join(EXECUTABLES[0]);
        std::fs::write(&script, "#!/bin/sh\n# my own version\n").unwrap();

        match status_in(&dir) {
            Status::Drifted(files) => assert_eq!(files, vec![EXECUTABLES[0].to_string()]),
            other => panic!("expected drift, got {other:?}"),
        }

        let refused = install_in(&dir, false).expect_err("install must refuse to clobber");
        assert!(refused.contains(EXECUTABLES[0]));
        assert!(
            std::fs::read_to_string(&script)
                .unwrap()
                .contains("my own version")
        );

        install_in(&dir, true).expect("an explicit overwrite is allowed");
        assert_eq!(status_in(&dir), Status::Installed);
    }

    /// The startup nag fires for an outdated install and nothing else:
    /// not-installed was never chosen, drift is the user's own edit, and a
    /// current install has nothing to say.
    #[test]
    fn only_an_outdated_install_earns_the_startup_notice() {
        let outdated = Status::Outdated {
            installed: "3".into(),
        };
        let notice = upgrade_notice(&outdated).expect("outdated must notify");
        assert!(notice.contains("v3"), "{notice}");
        assert!(notice.contains(SHIPPED_VERSION), "{notice}");
        assert!(
            notice.contains("Settings > Claude Code Integration"),
            "{notice}"
        );

        assert_eq!(upgrade_notice(&Status::NotInstalled), None);
        assert_eq!(upgrade_notice(&Status::Installed), None);
        assert_eq!(
            upgrade_notice(&Status::Drifted(vec!["hooks/tmux-title.sh".into()])),
            None
        );
    }

    /// And the real status of a freshly written old-version install produces
    /// that notice end to end.
    #[test]
    fn an_old_install_reads_as_outdated_and_notifies() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(PLUGIN_NAME);
        install_in(&dir, false).unwrap();
        std::fs::write(
            dir.join(".claude-plugin/plugin.json"),
            format!("{{\"name\":\"{PLUGIN_NAME}\",\"version\":\"3\"}}"),
        )
        .unwrap();

        let status = status_in(&dir);
        assert!(matches!(status, Status::Outdated { .. }), "{status:?}");
        assert!(upgrade_notice(&status).is_some());
    }

    /// A recursive delete guarded by a substring match would take
    /// "forestui-tmux-title-fork" with it.
    #[test]
    fn uninstall_refuses_a_plugin_whose_name_merely_contains_ours() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("forestui-tmux-title-fork");
        std::fs::create_dir_all(dir.join(".claude-plugin")).unwrap();
        std::fs::write(
            dir.join(".claude-plugin/plugin.json"),
            "{\"name\":\"forestui-tmux-title-fork\"}",
        )
        .unwrap();

        uninstall_in(&dir).expect_err("must refuse a different plugin");
        assert!(dir.exists());
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

        for rel in EXECUTABLES {
            let mode = std::fs::metadata(dir.join(rel))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111, "{rel}: owner/group/other execute");
        }
    }
}
