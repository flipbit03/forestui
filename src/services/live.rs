//! Where each Claude session is running right now.
//!
//! Two sources, merged into one answer:
//!
//! - **Window stamps** — the `@claude_session_id` option on windows forestui
//!   opened (and on any window the plugin's heartbeat hook healed). Cheap,
//!   and it names a window forestui can jump to.
//! - **Heartbeats** — files the plugin writes from inside every session on
//!   the machine, wherever it was launched: a hand-made tmux window, a
//!   terminal with no tmux at all. They carry the claude process id, which is
//!   what separates a live session from a file a crash left behind.
//!
//! A session found by stamp wins (it is switchable); a heartbeat fills in the
//! rest, resolving its recorded pane back to a window when that pane lives on
//! forestui's own tmux session, and falling back to "elsewhere" otherwise.

use crate::services::tmux;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Where a live session can be reached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LivePlace {
    /// A window on forestui's own tmux session — the guard can offer to
    /// switch there.
    Window {
        window_id: String,
        window_name: String,
    },
    /// Running somewhere forestui cannot jump to: another tmux session, or no
    /// tmux at all. The pid is shown so the user can find it.
    Elsewhere { pid: u32 },
}

/// One live Claude session and where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveSession {
    pub session_id: String,
    pub place: LivePlace,
}

/// A session's heartbeat, as `heartbeat.sh` writes it from inside the session.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct Heartbeat {
    pub session_id: String,
    pub pid: u32,
    #[serde(default)]
    pub tmux_pane: String,
    #[serde(default)]
    pub cwd: String,
}

/// Where the plugin writes heartbeats. Must match `heartbeat.sh` verbatim.
pub fn live_dir() -> PathBuf {
    crate::util::home_dir()
        .join(".config")
        .join("forestui")
        .join("live")
}

/// The full liveness picture, both sources merged. One `list-windows`, one
/// directory read, one `ps` — run off the UI loop like every other scan.
pub fn snapshot() -> Vec<LiveSession> {
    merge(
        tmux::list_claude_windows(),
        read_heartbeats(),
        tmux::current_session(),
        tmux::pane_window,
    )
}

/// Pure form of [`snapshot`], for testing: `resolve` stands in for asking
/// tmux which window a heartbeat's pane belongs to.
pub fn merge(
    windows: Vec<tmux::ClaudeWindow>,
    beats: Vec<Heartbeat>,
    ours: Option<String>,
    resolve: impl Fn(&str) -> Option<tmux::PaneWindow>,
) -> Vec<LiveSession> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<LiveSession> = Vec::new();

    // Stamped windows where Claude is the pane's foreground process. A window
    // whose Claude exited deliberately stays out: that session is free again.
    for window in windows {
        if window.running && seen.insert(window.session_id.clone()) {
            out.push(LiveSession {
                session_id: window.session_id,
                place: LivePlace::Window {
                    window_id: window.window_id,
                    window_name: window.window_name,
                },
            });
        }
    }

    // Heartbeats cover everything else. A recorded pane that resolves onto
    // our own tmux session is still switchable; anything else is elsewhere.
    // (A suspended Claude — Ctrl-Z — is alive by pid and so counts as live
    // here, which is right: resuming it somewhere else would fork the
    // conversation a `fg` can still continue.)
    for beat in read_heartbeats_merge(beats, &mut seen) {
        let place = match (&ours, beat.tmux_pane.is_empty()) {
            (Some(ours), false) => match resolve(&beat.tmux_pane) {
                Some(pane) if &pane.tmux_session == ours => LivePlace::Window {
                    window_id: pane.window_id,
                    window_name: pane.window_name,
                },
                _ => LivePlace::Elsewhere { pid: beat.pid },
            },
            _ => LivePlace::Elsewhere { pid: beat.pid },
        };
        out.push(LiveSession {
            session_id: beat.session_id,
            place,
        });
    }
    out
}

/// The heartbeats not already accounted for by a stamped window.
fn read_heartbeats_merge(beats: Vec<Heartbeat>, seen: &mut HashSet<String>) -> Vec<Heartbeat> {
    beats
        .into_iter()
        .filter(|beat| seen.insert(beat.session_id.clone()))
        .collect()
}

/// Whether this session id has a heartbeat naming a live claude — the
/// delete task's moment-of-truth recheck, alongside the tmux one.
pub fn heartbeat_alive(session_id: &str) -> bool {
    read_heartbeats()
        .iter()
        .any(|beat| beat.session_id == session_id)
}

/// Every heartbeat whose process is still a running claude. Stale files —
/// a crash skipped `SessionEnd`, or the pid was reused by something else —
/// are swept here, so the directory never accumulates lies.
pub fn read_heartbeats() -> Vec<Heartbeat> {
    read_heartbeats_in(&live_dir(), alive_claude_pids)
}

/// Directory-scoped form of [`read_heartbeats`], for testing. `alive` answers
/// which of the recorded pids currently run claude.
pub fn read_heartbeats_in(dir: &Path, alive: impl Fn(&[u32]) -> HashSet<u32>) -> Vec<Heartbeat> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut beats: Vec<(PathBuf, Heartbeat)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(beat) = std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<Heartbeat>(&raw).ok())
        else {
            // Unparseable is stale by definition; the plugin writes atomically,
            // so this is a leftover of something else, not a half-write.
            let _ = std::fs::remove_file(&path);
            continue;
        };
        // The filename is the session id; a mismatch is a copied or tampered
        // file and is not trusted either way.
        if path.file_stem().and_then(|s| s.to_str()) != Some(beat.session_id.as_str()) {
            let _ = std::fs::remove_file(&path);
            continue;
        }
        beats.push((path, beat));
    }

    let pids: Vec<u32> = beats.iter().map(|(_, b)| b.pid).collect();
    let alive = alive(&pids);
    beats
        .into_iter()
        .filter_map(|(path, beat)| {
            if alive.contains(&beat.pid) {
                Some(beat)
            } else {
                let _ = std::fs::remove_file(&path);
                None
            }
        })
        .collect()
}

/// Which of these pids are running claude right now. One `ps` for the lot;
/// a reused pid whose command is something else does not count — a heartbeat
/// must never outlive the session it describes.
fn alive_claude_pids(pids: &[u32]) -> HashSet<u32> {
    if pids.is_empty() {
        return HashSet::new();
    }
    let list = pids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let Ok(output) = std::process::Command::new("ps")
        .args(["-o", "pid=,comm=", "-p", &list])
        .output()
    else {
        return HashSet::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let pid: u32 = parts.next()?.parse().ok()?;
            let comm = parts.next()?;
            (comm.starts_with("claude") || comm.starts_with("node")).then_some(pid)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_beat(dir: &Path, sid: &str, pid: u32, pane: &str) {
        std::fs::write(
            dir.join(format!("{sid}.json")),
            format!(
                "{{\"session_id\":\"{sid}\",\"pid\":{pid},\"tmux_pane\":\"{pane}\",\"cwd\":\"/x\"}}"
            ),
        )
        .unwrap();
    }

    /// A heartbeat is only as good as its process: dead or reused pids are
    /// swept off disk, not just skipped, so a crash cannot leave a session
    /// reading as live forever.
    #[test]
    fn stale_heartbeats_are_swept_and_live_ones_kept() {
        let dir = tempfile::tempdir().unwrap();
        write_beat(dir.path(), "alive-1", 100, "%5");
        write_beat(dir.path(), "dead-1", 200, "");
        std::fs::write(dir.path().join("garbage.json"), "not json").unwrap();
        // A file whose name disagrees with its content is not trusted.
        std::fs::write(
            dir.path().join("mismatch.json"),
            "{\"session_id\":\"other\",\"pid\":100}",
        )
        .unwrap();

        let beats = read_heartbeats_in(dir.path(), |_| HashSet::from([100u32]));
        assert_eq!(beats.len(), 1);
        assert_eq!(beats[0].session_id, "alive-1");
        assert_eq!(beats[0].pid, 100);
        assert_eq!(beats[0].tmux_pane, "%5");

        assert!(!dir.path().join("dead-1.json").exists(), "dead pid swept");
        assert!(!dir.path().join("garbage.json").exists(), "garbage swept");
        assert!(
            !dir.path().join("mismatch.json").exists(),
            "mismatched file swept"
        );
        assert!(dir.path().join("alive-1.json").exists(), "live one kept");
    }

    /// A missing directory is the common case (plugin not installed, nothing
    /// ever ran) and must be silence, not an error.
    #[test]
    fn a_missing_live_dir_reads_as_no_heartbeats() {
        assert!(
            read_heartbeats_in(Path::new("/nowhere/forestui-live"), |_| HashSet::new()).is_empty()
        );
    }

    /// Run the shipped `heartbeat.sh` for real, with `ps` and `tmux` faked on
    /// PATH: the pid walk needs an ancestor that claims to be claude, and a
    /// test must never speak to a real tmux server. This pins the script's
    /// whole observable contract — the file it writes, the sweep on
    /// SessionEnd, the id sanitisation, and the window-stamp healing call.
    #[cfg(unix)]
    #[test]
    fn the_heartbeat_script_writes_sweeps_and_heals() {
        use std::os::unix::fs::PermissionsExt;
        if std::process::Command::new("jq")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("jq not installed; skipping the script test");
            return;
        }

        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let script = bin.join("heartbeat.sh");
        std::fs::write(
            &script,
            include_str!("../../assets/claude-plugin/heartbeat.sh"),
        )
        .unwrap();
        // A fake ps: every ancestry query answers pid 4242, whose command is
        // claude — so the walk terminates on the first hop.
        std::fs::write(
            bin.join("ps"),
            "#!/bin/sh\ncase \"$*\" in *ppid*) echo 4242;; *comm*) echo claude;; esac\n",
        )
        .unwrap();
        // A fake tmux that records its argv instead of reaching any server.
        std::fs::write(
            bin.join("tmux"),
            format!(
                "#!/bin/sh\necho \"$@\" >> {}\n",
                home.path().join("tmux.log").display()
            ),
        )
        .unwrap();
        for file in [&script, &bin.join("ps"), &bin.join("tmux")] {
            let mut perms = std::fs::metadata(file).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(file, perms).unwrap();
        }

        let run = |event: &str, input: &str, pane: Option<&str>| {
            let mut cmd = std::process::Command::new(&script);
            cmd.arg(event)
                .env("HOME", home.path())
                .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::null());
            match pane {
                Some(pane) => cmd.env("TMUX_PANE", pane),
                None => cmd.env_remove("TMUX_PANE"),
            };
            let mut child = cmd.spawn().expect("the script runs");
            use std::io::Write;
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.as_bytes())
                .unwrap();
            assert!(child.wait().unwrap().success());
        };

        let live = home.path().join(".config/forestui/live");

        // SessionStart writes the heartbeat, with the walked-to claude pid.
        run(
            "SessionStart",
            r#"{"session_id":"beat-1","cwd":"/w/x"}"#,
            None,
        );
        let written = std::fs::read_to_string(live.join("beat-1.json")).expect("a heartbeat");
        let parsed: Heartbeat = serde_json::from_str(&written).expect("valid JSON");
        assert_eq!(parsed.session_id, "beat-1");
        assert_eq!(parsed.pid, 4242);
        assert_eq!(parsed.cwd, "/w/x");
        assert_eq!(parsed.tmux_pane, "");

        // Inside tmux, the stamp on the window is healed to this session.
        run(
            "UserPromptSubmit",
            r#"{"session_id":"beat-1","cwd":"/w/x"}"#,
            Some("%9"),
        );
        let stamped = std::fs::read_to_string(home.path().join("tmux.log")).unwrap_or_default();
        assert!(
            stamped.contains("set-option -w -t %9 @claude_session_id beat-1"),
            "no healing call: {stamped:?}"
        );

        // A hostile id cannot write outside the live directory.
        run(
            "SessionStart",
            r#"{"session_id":"../../escape","cwd":"/w"}"#,
            None,
        );
        assert!(
            !home.path().join("escape.json").exists()
                && !home.path().join(".config/escape.json").exists(),
            "a traversal id escaped the live dir"
        );

        // SessionEnd sweeps the heartbeat away.
        run("SessionEnd", r#"{"session_id":"beat-1"}"#, None);
        assert!(!live.join("beat-1.json").exists(), "SessionEnd must sweep");
    }

    fn beat(sid: &str, pid: u32, pane: &str) -> Heartbeat {
        Heartbeat {
            session_id: sid.into(),
            pid,
            tmux_pane: pane.into(),
            cwd: String::new(),
        }
    }

    fn window(sid: &str, running: bool) -> tmux::ClaudeWindow {
        tmux::ClaudeWindow {
            window_id: "@1".into(),
            window_name: format!("claude:{sid}"),
            running,
            session_id: sid.into(),
        }
    }

    /// The merge's whole contract: exited windows stay out (that session is
    /// free), stamps outrank heartbeats for the same session, a heartbeat
    /// pane on our own tmux session resolves to a switchable window, and
    /// everything else is elsewhere.
    #[test]
    fn the_merge_ranks_stamps_resolves_panes_and_frees_exited_windows() {
        let windows = vec![window("in-window", true), window("exited", false)];
        let beats = vec![
            beat("in-window", 10, "%1"),  // covered by its stamp already
            beat("our-pane", 20, "%2"),   // resolves to our session
            beat("other-pane", 30, "%3"), // another tmux session
            beat("no-tmux", 40, ""),      // no pane at all
        ];
        let resolve = |pane: &str| match pane {
            "%2" => Some(tmux::PaneWindow {
                tmux_session: "$0".into(),
                window_id: "@7".into(),
                window_name: "hand-made".into(),
            }),
            "%3" => Some(tmux::PaneWindow {
                tmux_session: "$9".into(),
                window_id: "@8".into(),
                window_name: "foreign".into(),
            }),
            _ => None,
        };

        let merged = merge(windows, beats, Some("$0".into()), resolve);
        let by_id: std::collections::HashMap<&str, &LivePlace> = merged
            .iter()
            .map(|s| (s.session_id.as_str(), &s.place))
            .collect();

        assert_eq!(merged.len(), 4, "the exited window must not appear");
        assert!(!by_id.contains_key("exited"), "exited sessions are free");
        assert!(matches!(
            by_id["in-window"],
            LivePlace::Window { window_name, .. } if window_name == "claude:in-window"
        ));
        assert!(matches!(
            by_id["our-pane"],
            LivePlace::Window { window_id, window_name } if window_id == "@7" && window_name == "hand-made"
        ));
        assert!(matches!(
            by_id["other-pane"],
            LivePlace::Elsewhere { pid: 30 }
        ));
        assert!(matches!(by_id["no-tmux"], LivePlace::Elsewhere { pid: 40 }));

        // Outside tmux entirely, every heartbeat is elsewhere.
        let merged = merge(Vec::new(), vec![beat("x", 5, "%2")], None, |_| None);
        assert!(matches!(merged[0].place, LivePlace::Elsewhere { pid: 5 }));
    }
}
