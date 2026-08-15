//! Startup self-update.
//!
//! The Python build shelled out to `uv tool upgrade forestui` on every launch.
//! There is no sane Cargo equivalent — `cargo install` would recompile the crate
//! at startup — so the binary path replaces it: download the release asset for
//! this platform and swap it over the running executable.
//!
//! Two things make that safe to do automatically rather than behind a
//! subcommand, which is where this differs from `lineark` and `terminal-use`:
//!
//! - It never blocks the UI. The check runs on a background task after the
//!   terminal is up, and the only thing the user sees is a notification once a
//!   new version is already installed.
//! - It only ever replaces a binary that came from a release. A `cargo install`
//!   build defers to cargo, and a source build (version `0.0.0`) does nothing at
//!   all, so `cargo run` in a checkout is never overwritten.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const GITHUB_REPO: &str = "flipbit03/forestui";
const CACHE_TTL_SECS: u64 = 24 * 60 * 60;
const USER_AGENT: &str = "forestui-self-update";

/// The compiled-in version, stamped from the git tag by the release workflow.
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// A build from the repository rather than a release. The checked-in version is
/// `0.0.0`, which is also what auto-enables dev mode, so this is the same signal
/// that already keeps a source build out of the release window naming.
pub fn is_dev_build() -> bool {
    current_version() == "0.0.0"
}

/// Is `latest` newer than `current`?
///
/// Anything that will not parse is treated as "not newer". The failure mode
/// worth avoiding is a malformed tag upstream convincing a working build to
/// download something over itself.
pub fn is_newer(current: &str, latest: &str) -> bool {
    match (
        semver::Version::parse(current),
        semver::Version::parse(latest),
    ) {
        (Ok(c), Ok(l)) => l > c,
        _ => false,
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct VersionCache {
    checked_at: u64,
    latest_version: String,
}

fn cache_path() -> PathBuf {
    crate::util::home_dir()
        .join(".config")
        .join("forestui")
        .join("latest_version_check.json")
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn read_cache() -> Option<VersionCache> {
    serde_json::from_str(&std::fs::read_to_string(cache_path()).ok()?).ok()
}

fn write_cache(cache: &VersionCache) {
    let path = cache_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(cache) {
        let _ = std::fs::write(&path, json);
    }
}

/// The newest published version, from cache when it is under a day old.
///
/// The cache is what keeps this from calling GitHub on every launch. forestui is
/// opened many times a day, and a per-launch network round trip is the part of
/// the Python build's behaviour worth not reproducing.
pub async fn latest_version() -> Option<String> {
    if let Some(cache) = read_cache()
        && now_secs().saturating_sub(cache.checked_at) < CACHE_TTL_SECS
    {
        return Some(cache.latest_version);
    }

    match fetch_latest_version().await {
        Ok(version) => {
            write_cache(&VersionCache {
                checked_at: now_secs(),
                latest_version: version.clone(),
            });
            Some(version)
        }
        // Offline is the common case, not an error worth surfacing: fall back to
        // whatever was last seen rather than nagging.
        Err(_) => read_cache().map(|c| c.latest_version),
    }
}

async fn fetch_latest_version() -> Result<String> {
    let url = format!("https://api.github.com/repos/{GITHUB_REPO}/releases/latest");
    let body: serde_json::Value = reqwest::Client::new()
        .get(&url)
        .header("User-Agent", USER_AGENT)
        .send()
        .await
        .context("could not reach the GitHub API")?
        .error_for_status()
        .context("the GitHub API returned an error")?
        .json()
        .await
        .context("could not parse the GitHub API response")?;

    let tag = body["tag_name"]
        .as_str()
        .context("no tag_name in the latest release")?;
    Ok(tag.strip_prefix('v').unwrap_or(tag).to_string())
}

/// Release asset for the platform this binary is running on.
///
/// Only the in-place updater needs this, so without `binary-release` it would be
/// dead code — but the tests assert it against the layout `install.sh` and the
/// release workflow use, and that agreement is worth checking on every build.
#[cfg(any(feature = "binary-release", test))]
pub fn release_asset_url(version: &str) -> Result<String> {
    let os = match std::env::consts::OS {
        "linux" => "linux",
        "macos" => "macos",
        other => anyhow::bail!("unsupported OS: {other}"),
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        other => anyhow::bail!("unsupported architecture: {other}"),
    };
    if os == "macos" && arch == "x86_64" {
        anyhow::bail!("macOS x86_64 binaries are not published — use `cargo install forestui`");
    }
    Ok(format!(
        "https://github.com/{GITHUB_REPO}/releases/download/v{version}/forestui_{os}_{arch}"
    ))
}

/// Download `url` and put it at `target`, atomically.
///
/// Staged beside the target and renamed into place, so the swap is atomic and a
/// half-downloaded file can never end up executable. Replacing the file under a
/// running process is safe on Unix — the running image is already mapped — which
/// is why this can happen while the UI is up, with the new build taking effect
/// on the next launch.
///
/// Takes its URL and destination rather than deriving them, so the download and
/// the swap can be tested against a local server and a temporary file. Deciding
/// *whether* to run it is the caller's job, and that is what carries the
/// `binary-release` gate.
#[cfg(any(feature = "binary-release", test))]
pub(crate) async fn install_from(url: &str, target: &std::path::Path) -> Result<()> {
    let bytes = reqwest::Client::new()
        .get(url)
        .header("User-Agent", USER_AGENT)
        .send()
        .await
        .context("could not download the release")?
        .error_for_status()
        .context("the release download failed")?
        .bytes()
        .await
        .context("could not read the downloaded release")?;

    let staged = target.with_extension("tmp-update");
    tokio::fs::write(&staged, &bytes)
        .await
        .context("could not write the downloaded release")?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))
            .await
            .context("could not mark the downloaded release executable")?;
    }

    // A rename across filesystems fails, so clean the staged file up rather than
    // leaving a stray binary beside the real one.
    if let Err(error) = tokio::fs::rename(&staged, target).await {
        let _ = tokio::fs::remove_file(&staged).await;
        return Err(error).context("could not replace the running binary");
    }
    Ok(())
}

/// Bring the installed binary up to date, returning the version involved.
///
/// `Ok(None)` means there was nothing to do: already current, a source build, or
/// no answer from GitHub.
pub async fn update_if_stale() -> Result<Option<String>> {
    if is_dev_build() {
        return Ok(None);
    }
    let Some(latest) = latest_version().await else {
        return Ok(None);
    };
    if !is_newer(current_version(), &latest) {
        return Ok(None);
    }

    #[cfg(feature = "binary-release")]
    {
        let current = std::env::current_exe().context("could not locate the running executable")?;
        install_from(&release_asset_url(&latest)?, &current).await?;
        Ok(Some(latest))
    }
    // Installed by cargo: replacing the binary in place would be undone by the
    // next `cargo install`, and recompiling the crate underneath a running TUI
    // is not something to do unasked. Report it instead.
    #[cfg(not(feature = "binary-release"))]
    {
        Ok(Some(latest))
    }
}

/// Whether an update installs itself or is merely announced, for the wording of
/// the notification.
pub const fn installs_in_place() -> bool {
    cfg!(feature = "binary-release")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_ordering_decides_staleness() {
        assert!(is_newer("2.0.0", "2.0.1"));
        assert!(is_newer("2.0.0", "2.1.0"));
        assert!(is_newer("1.9.9", "2.0.0"));
        assert!(!is_newer("2.0.1", "2.0.0"));
        assert!(!is_newer("2.0.0", "2.0.0"));
        // A pre-release precedes the release it names.
        assert!(!is_newer("2.0.0", "2.0.0-rc1"));
    }

    /// A tag that will not parse must never look newer: the failure mode is
    /// downloading something unknown over a working binary.
    #[test]
    fn unparseable_versions_never_look_newer() {
        assert!(!is_newer("2.0.0", "not-a-version"));
        assert!(!is_newer("not-a-version", "2.0.0"));
        assert!(!is_newer("", ""));
    }

    /// The updater, `install.sh` and the release workflow all have to agree on
    /// this name. Drift is a silent 404 on every update.
    #[test]
    fn asset_url_matches_the_release_layout() {
        let url = release_asset_url("2.0.0").expect("supported platform");
        assert!(url.starts_with("https://github.com/flipbit03/forestui/releases/download/v2.0.0/"));
        let expected = format!(
            "forestui_{}_{}",
            if cfg!(target_os = "macos") {
                "macos"
            } else {
                "linux"
            },
            if cfg!(target_arch = "aarch64") {
                "aarch64"
            } else {
                "x86_64"
            }
        );
        assert!(url.ends_with(&expected), "{url} should end with {expected}");
    }

    #[test]
    fn a_source_build_never_updates() {
        // The checked-in version is 0.0.0 and only the release workflow stamps
        // it, so this is what keeps `cargo run` from replacing itself.
        assert_eq!(is_dev_build(), current_version() == "0.0.0");
    }

    /// The download-and-swap is the one step that can destroy a working install,
    /// so it is exercised for real: a local server, a temporary file standing in
    /// for the running binary, and assertions on the bytes, the permissions and
    /// the absence of leftovers.
    #[tokio::test]
    async fn install_from_replaces_the_target_atomically() {
        use std::io::{Read, Write};
        let payload = b"#!/bin/sh\necho new version\n";

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("accept");
            let mut buf = [0u8; 1024];
            let _ = socket.read(&mut buf);
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                payload.len()
            );
            socket.write_all(head.as_bytes()).expect("head");
            socket.write_all(payload).expect("body");
        });

        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("forestui");
        std::fs::write(&target, b"old version").expect("seed the target");

        install_from(&format!("http://127.0.0.1:{port}/asset"), &target)
            .await
            .expect("install");
        server.join().expect("server");

        assert_eq!(std::fs::read(&target).expect("read back"), payload);
        assert!(
            !target.with_extension("tmp-update").exists(),
            "the staging file was left behind"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&target)
                .expect("stat")
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111, "the replacement is not executable");
        }
    }

    /// A failed download must leave the existing binary alone — a missed update
    /// is a non-event, a broken install is not.
    #[tokio::test]
    async fn a_failed_download_leaves_the_target_untouched() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        // Nothing is ever served: dropping the listener refuses the connection,
        // which is the same shape as an unreachable GitHub.
        drop(listener);

        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("forestui");
        std::fs::write(&target, b"old version").expect("seed the target");

        assert!(
            install_from(&format!("http://127.0.0.1:{port}/asset"), &target)
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(&target).expect("read back"), b"old version");
        assert!(!target.with_extension("tmp-update").exists());
    }

    #[tokio::test]
    async fn a_dev_build_does_no_work() {
        if is_dev_build() {
            assert!(update_if_stale().await.expect("no error").is_none());
        }
    }
}
