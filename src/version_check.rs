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
    /// Set when downloading or installing `latest_version` failed after a
    /// successful lookup. Remembered so the next launch does not re-spend the
    /// multi-MB download on a failure that will repeat (an unwritable install
    /// dir fails identically every time); cleared by expiry, a newer release,
    /// or a successful install.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    install_failed: Option<InstallFailure>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct InstallFailure {
    version: String,
    reason: String,
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
                install_failed: None,
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
    // Without a timeout, a network that accepts the connection but never
    // answers parks this task forever — and with it the whole check, since a
    // hung lookup is cached by nobody.
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .context("could not build the HTTP client")?;
    let body: serde_json::Value = client
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

/// Download `url`, verify it against its published `.sha256`, and put it at
/// `target`, atomically.
///
/// The release workflow publishes a checksum beside every asset and
/// `install.sh` verifies it; the updater holds itself to the same standard —
/// stricter, in fact: a missing or malformed checksum aborts the update rather
/// than trusting whatever arrived over the wire, because the updater lives
/// inside the binary it would break and the user cannot self-recover from a
/// corrupt install.
///
/// Staged beside the target under a per-process name and renamed into place, so
/// the swap is atomic, a half-downloaded file can never end up executable, and
/// two instances updating at once cannot rename each other's half-written
/// staging file over the real binary. Replacing the file under a running
/// process is safe on Unix — the running image is already mapped — which is why
/// this can happen while the UI is up, with the new build taking effect on the
/// next launch.
///
/// Takes its URL and destination rather than deriving them, so the download and
/// the swap can be tested against a local server and a temporary file. Deciding
/// *whether* to run it is the caller's job, and that is what carries the
/// `binary-release` gate.
#[cfg(any(feature = "binary-release", test))]
pub(crate) async fn install_from(url: &str, target: &std::path::Path) -> Result<()> {
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(90))
        .build()
        .context("could not build the HTTP client")?;

    let bytes = client
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

    // Verify before anything touches disk.
    let checksum_body = client
        .get(format!("{url}.sha256"))
        .header("User-Agent", USER_AGENT)
        .send()
        .await
        .context("could not fetch the release checksum")?
        .error_for_status()
        .context("the release has no checksum")?
        .text()
        .await
        .context("could not read the release checksum")?;
    // Coreutils format: `<64 hex>  <filename>`. Only the hash is trusted.
    let expected = checksum_body
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()) {
        anyhow::bail!("the release checksum file is malformed");
    }
    use sha2::Digest;
    let actual = format!("{:x}", sha2::Sha256::digest(&bytes));
    if actual != expected {
        anyhow::bail!("the downloaded release does not match its checksum");
    }

    let staged = staging_path(target);
    // All the fallible steps live in one helper so every failure path — write,
    // chmod, rename — cleans the staged file up rather than leaving a stray
    // binary beside the real one. Same contract as `util::write_atomically`.
    let result = write_staged_then_rename(&staged, target, &bytes).await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&staged).await;
    }
    result
}

/// Sibling of `target` with a per-process suffix, so concurrent instances
/// stage privately: the last rename wins whole-file, which is fine — both
/// downloaded (and verified) the same asset.
#[cfg(any(feature = "binary-release", test))]
fn staging_path(target: &std::path::Path) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".tmp-update-{}", std::process::id()));
    target.with_file_name(name)
}

#[cfg(any(feature = "binary-release", test))]
async fn write_staged_then_rename(
    staged: &std::path::Path,
    target: &std::path::Path,
    bytes: &[u8],
) -> Result<()> {
    tokio::fs::write(staged, bytes)
        .await
        .context("could not write the downloaded release")?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(staged, std::fs::Permissions::from_mode(0o755))
            .await
            .context("could not mark the downloaded release executable")?;
    }

    tokio::fs::rename(staged, target)
        .await
        .context("could not replace the running binary")?;
    Ok(())
}

/// What the startup update check concluded.
///
/// The enum is the cfg-independent contract: which variants a build actually
/// constructs depends on the `binary-release` gate (`Installed`/`InstallFailed`
/// with it, `Available` without), so each side sees the others as dead.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateStatus {
    /// Nothing to say: already current, a source build, or no answer from
    /// GitHub. Being offline is the common case, not the user's problem.
    Silent,
    /// The new build is already in place; takes effect on the next launch.
    Installed(String),
    /// A newer version exists. Installed by cargo, so replacing the binary in
    /// place would be undone by the next `cargo install`, and recompiling the
    /// crate underneath a running TUI is not something to do unasked.
    Available(String),
    /// A newer version exists but could not be installed. Unlike being
    /// offline, this is actionable (an unwritable install dir, a corrupted
    /// mirror) and would otherwise repeat invisibly on every launch.
    InstallFailed { version: String, reason: String },
}

/// Bring the installed binary up to date.
pub async fn update_if_stale() -> UpdateStatus {
    if is_dev_build() {
        return UpdateStatus::Silent;
    }
    let Some(latest) = latest_version().await else {
        return UpdateStatus::Silent;
    };
    if !is_newer(current_version(), &latest) {
        return UpdateStatus::Silent;
    }

    #[cfg(feature = "binary-release")]
    {
        // A fresh cache remembering a failed install of this same version
        // means the download would fail the same way — surface the standing
        // failure without re-spending the bandwidth. Retried when the cache
        // expires or a newer release appears.
        if let Some(cache) = read_cache()
            && now_secs().saturating_sub(cache.checked_at) < CACHE_TTL_SECS
            && let Some(failure) = cache.install_failed
            && failure.version == latest
        {
            return UpdateStatus::InstallFailed {
                version: latest,
                reason: failure.reason,
            };
        }

        let result = async {
            let current =
                std::env::current_exe().context("could not locate the running executable")?;
            install_from(&release_asset_url(&latest)?, &current).await
        }
        .await;

        match result {
            Ok(()) => {
                // Clear a failure memo left by an older release's failed install.
                if let Some(mut cache) = read_cache()
                    && cache.install_failed.is_some()
                {
                    cache.install_failed = None;
                    write_cache(&cache);
                }
                UpdateStatus::Installed(latest)
            }
            Err(error) => {
                let reason = format!("{error:#}");
                if let Some(mut cache) = read_cache() {
                    cache.install_failed = Some(InstallFailure {
                        version: latest.clone(),
                        reason: reason.clone(),
                    });
                    write_cache(&cache);
                }
                UpdateStatus::InstallFailed {
                    version: latest,
                    reason,
                }
            }
        }
    }
    #[cfg(not(feature = "binary-release"))]
    {
        UpdateStatus::Available(latest)
    }
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

    /// Serve the asset and its sibling `.sha256` from a throwaway local
    /// server. `checksum` is the full text of the checksum file; `None` serves
    /// a 404 for it, the shape of a release published without one.
    fn serve_release(
        payload: &'static [u8],
        checksum: Option<String>,
    ) -> (u16, std::thread::JoinHandle<()>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let handle = std::thread::spawn(move || {
            // One connection for the asset, one for the checksum.
            for _ in 0..2 {
                let Ok((mut socket, _)) = listener.accept() else {
                    return;
                };
                let mut buf = [0u8; 1024];
                let n = socket.read(&mut buf).unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]).to_string();
                let wants_checksum = request
                    .split_whitespace()
                    .nth(1)
                    .is_some_and(|path| path.ends_with(".sha256"));
                let response = if wants_checksum {
                    match &checksum {
                        Some(text) => format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            text.len(),
                            text
                        )
                        .into_bytes(),
                        None => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
                    }
                } else {
                    let mut head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        payload.len()
                    )
                    .into_bytes();
                    head.extend_from_slice(payload);
                    head
                };
                socket.write_all(&response).expect("respond");
            }
        });
        (port, handle)
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        use sha2::Digest;
        format!("{:x}", sha2::Sha256::digest(bytes))
    }

    /// The directory must hold exactly the target — no staging leftovers under
    /// any name.
    fn assert_no_leftovers(dir: &std::path::Path) {
        let entries: Vec<_> = std::fs::read_dir(dir)
            .expect("read dir")
            .map(|e| e.expect("entry").file_name())
            .collect();
        assert_eq!(
            entries,
            vec![std::ffi::OsString::from("forestui")],
            "leftover staging files"
        );
    }

    /// The download-and-swap is the one step that can destroy a working install,
    /// so it is exercised for real: a local server, a temporary file standing in
    /// for the running binary, and assertions on the bytes, the permissions and
    /// the absence of leftovers.
    #[tokio::test]
    async fn install_from_replaces_the_target_atomically() {
        let payload = b"#!/bin/sh\necho new version\n";
        let checksum = format!("{}  forestui_linux_x86_64\n", sha256_hex(payload));
        let (port, server) = serve_release(payload, Some(checksum));

        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("forestui");
        std::fs::write(&target, b"old version").expect("seed the target");

        install_from(&format!("http://127.0.0.1:{port}/asset"), &target)
            .await
            .expect("install");
        server.join().expect("server");

        assert_eq!(std::fs::read(&target).expect("read back"), payload);
        assert_no_leftovers(dir.path());
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

    /// A download that does not match its published checksum must leave the
    /// running install byte-identical and no staging file behind.
    #[tokio::test]
    async fn a_corrupted_download_leaves_the_target_untouched() {
        let payload = b"corrupted-in-transit";
        let checksum = format!(
            "{}  forestui_linux_x86_64\n",
            sha256_hex(b"what was published")
        );
        let (port, server) = serve_release(payload, Some(checksum));

        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("forestui");
        std::fs::write(&target, b"old version").expect("seed the target");

        let error = install_from(&format!("http://127.0.0.1:{port}/asset"), &target)
            .await
            .expect_err("a checksum mismatch must fail");
        server.join().expect("server");

        assert!(error.to_string().contains("checksum"), "got: {error:#}");
        assert_eq!(std::fs::read(&target).expect("read back"), b"old version");
        assert_no_leftovers(dir.path());
    }

    /// No checksum, no update: trusting whatever arrived over the wire is the
    /// failure mode the verification exists to prevent.
    #[tokio::test]
    async fn a_missing_checksum_aborts_the_update() {
        let payload = b"plausible payload";
        let (port, server) = serve_release(payload, None);

        let dir = tempfile::tempdir().expect("tempdir");
        let target = dir.path().join("forestui");
        std::fs::write(&target, b"old version").expect("seed the target");

        assert!(
            install_from(&format!("http://127.0.0.1:{port}/asset"), &target)
                .await
                .is_err()
        );
        server.join().expect("server");
        assert_eq!(std::fs::read(&target).expect("read back"), b"old version");
        assert_no_leftovers(dir.path());
    }

    /// Concurrent instances must stage privately — a shared staging path lets
    /// one process rename another's half-written file into place.
    #[test]
    fn staging_is_per_process() {
        let staged = staging_path(std::path::Path::new("/opt/bin/forestui"));
        assert_eq!(staged.parent(), Some(std::path::Path::new("/opt/bin")));
        let name = staged.file_name().unwrap().to_string_lossy().to_string();
        assert_eq!(name, format!("forestui.tmp-update-{}", std::process::id()));
    }

    /// Every variant renders into the notification the app shows; constructing
    /// them all here also keeps the cfg-gated arms from tripping dead-code.
    #[test]
    fn update_status_covers_every_outcome() {
        let statuses = [
            UpdateStatus::Silent,
            UpdateStatus::Installed("2.0.0".into()),
            UpdateStatus::Available("2.0.0".into()),
            UpdateStatus::InstallFailed {
                version: "2.0.0".into(),
                reason: "permission denied".into(),
            },
        ];
        assert_eq!(
            statuses
                .iter()
                .filter(|s| **s == UpdateStatus::Silent)
                .count(),
            1
        );
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
        assert_no_leftovers(dir.path());
    }

    #[tokio::test]
    async fn a_dev_build_does_no_work() {
        if is_dev_build() {
            assert_eq!(update_if_stale().await, UpdateStatus::Silent);
        }
    }
}
