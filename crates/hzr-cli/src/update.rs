use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use reqwest::{StatusCode, header};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

const RELEASES_API: &str = "https://api.github.com/repos/heAdz0r/hzr/releases?per_page=30";
const RELEASE_DOWNLOAD_ROOT: &str = "https://github.com/heAdz0r/hzr/releases/download";
const CURRENT_CACHE_TTL_SECONDS: u64 = 3_600;
const AVAILABLE_CACHE_TTL_SECONDS: u64 = 86_400;
const STARTUP_TIMEOUT: Duration = Duration::from_secs(2);
const UPDATE_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_API_BYTES: usize = 1_048_576;
const MAX_CHECKSUM_BYTES: usize = 1_048_576;
const MAX_ARCHIVE_BYTES: u64 = 2 * 1_024 * 1_024 * 1_024;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ReleaseVersion([u64; 3]);

impl fmt::Display for ReleaseVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.0[0], self.0[1], self.0[2])
    }
}

#[derive(Clone, Debug, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Clone, Debug, Deserialize)]
struct GithubRelease {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<GithubAsset>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AvailableRelease {
    version: ReleaseVersion,
    tag: String,
    prerelease: bool,
    archive_name: String,
    archive_url: String,
    checksums_url: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct CachedRelease {
    checked_at_unix_seconds: u64,
    latest_version: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CacheStatus {
    FreshCurrent,
    FreshNewer(ReleaseVersion),
    Expired,
}

pub async fn execute(json: bool) -> Result<ExitCode> {
    let current = parse_release_version(env!("CARGO_PKG_VERSION"))?;
    let platform = current_platform()?;
    let release = fetch_latest_release(current, platform, UPDATE_TIMEOUT).await?;
    let data_dir = hzr_core::ConfigPaths::discover().data_dir;
    write_cache(
        &data_dir,
        release.as_ref().map(|available| available.version),
    )?;

    let Some(release) = release else {
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "outcome": "current",
                    "current_version": current.to_string(),
                })
            );
        } else {
            println!("HZR {current} is already the newest published release.");
        }
        return Ok(ExitCode::SUCCESS);
    };

    let temporary = tempfile::tempdir().context("failed to create update staging directory")?;
    let archive_path = temporary.path().join(&release.archive_name);
    let checksums_path = temporary.path().join("SHA256SUMS");
    let checksums = download_small_file(&release.checksums_url, MAX_CHECKSUM_BYTES).await?;
    let checksums_text =
        std::str::from_utf8(&checksums).context("the release SHA256SUMS asset is not UTF-8")?;
    let expected =
        checksum_for_artifact(checksums_text, &release.archive_name)?.to_ascii_lowercase();
    write_private_file(&checksums_path, &checksums)?;
    let actual = download_archive(&release.archive_url, &archive_path).await?;
    if actual != expected {
        bail!(
            "checksum mismatch for {}: expected {expected}, got {actual}",
            release.archive_name
        );
    }

    let installer = locate_installer()?;
    invoke_installer(&installer, &release, &archive_path, &checksums_path, json)?;
    if json {
        println!(
            "{}",
            serde_json::json!({
                "outcome": "updated",
                "previous_version": current.to_string(),
                "installed_version": release.version.to_string(),
                "prerelease": release.prerelease,
            })
        );
    } else {
        println!("Updated HZR from {current} to {}.", release.version);
    }
    Ok(ExitCode::SUCCESS)
}

pub async fn startup_notice(data_dir: &Path) -> Option<String> {
    let current = parse_release_version(env!("CARGO_PKG_VERSION")).ok()?;
    let now = unix_seconds();
    if let Ok(cached) = read_cache(data_dir) {
        match classify_cache(&cached, now, current) {
            CacheStatus::FreshNewer(version) => return Some(notice(current, version)),
            CacheStatus::FreshCurrent => return None,
            CacheStatus::Expired => {}
        }
    }

    let platform = current_platform().ok()?;
    let release = fetch_latest_release(current, platform, STARTUP_TIMEOUT)
        .await
        .ok()?;
    let latest = release.as_ref().map(|available| available.version);
    let _ = write_cache(data_dir, latest);
    latest.map(|version| notice(current, version))
}

fn notice(current: ReleaseVersion, latest: ReleaseVersion) -> String {
    format!("HZR {latest} is available (current {current}). Run `hzr update` to install it.")
}

pub(crate) fn agent_notice(message: &str) -> String {
    format!(
        "{message} Inform the user once that this update is available. Do not install it without explicit approval."
    )
}

pub(crate) fn session_start_payload(message: &str) -> serde_json::Value {
    serde_json::json!({
        "systemMessage": message,
        "hookSpecificOutput": {
            "hookEventName": "SessionStart",
            "additionalContext": agent_notice(message),
        },
    })
}

async fn fetch_latest_release(
    current: ReleaseVersion,
    platform: &str,
    timeout: Duration,
) -> Result<Option<AvailableRelease>> {
    let client = github_client(timeout)?;
    let response = client
        .get(RELEASES_API)
        .header(header::ACCEPT, "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await
        .context("failed to query GitHub releases")?;
    let bytes = response_bytes(response, MAX_API_BYTES, "GitHub releases response").await?;
    select_release(&bytes, current, platform)
}

fn github_client(timeout: Duration) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::limited(5))
        .user_agent(concat!("hzr/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(timeout.min(Duration::from_secs(10)))
        .timeout(timeout)
        .build()
        .context("failed to construct the GitHub update client")
}

async fn response_bytes(
    mut response: reqwest::Response,
    limit: usize,
    label: &str,
) -> Result<Vec<u8>> {
    require_success(&response, label)?;
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        bail!("{label} exceeds the {limit}-byte limit");
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .with_context(|| format!("failed to read {label}"))?
    {
        if bytes.len().saturating_add(chunk.len()) > limit {
            bail!("{label} exceeds the {limit}-byte limit");
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn download_small_file(url: &str, limit: usize) -> Result<Vec<u8>> {
    let response = github_client(UPDATE_TIMEOUT)?
        .get(url)
        .send()
        .await
        .with_context(|| format!("failed to download {url}"))?;
    response_bytes(response, limit, "release checksum manifest").await
}

async fn download_archive(url: &str, path: &Path) -> Result<String> {
    let mut response = github_client(UPDATE_TIMEOUT)?
        .get(url)
        .send()
        .await
        .with_context(|| format!("failed to download {url}"))?;
    require_success(&response, "release archive")?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_ARCHIVE_BYTES)
    {
        bail!("release archive exceeds the {MAX_ARCHIVE_BYTES}-byte limit");
    }
    let mut file = tokio::fs::File::create(path)
        .await
        .with_context(|| format!("failed to create {}", path.display()))?;
    let mut digest = Sha256::new();
    let mut downloaded = 0_u64;
    while let Some(chunk) = response
        .chunk()
        .await
        .context("failed while downloading the release archive")?
    {
        downloaded = downloaded.saturating_add(chunk.len() as u64);
        if downloaded > MAX_ARCHIVE_BYTES {
            bail!("release archive exceeds the {MAX_ARCHIVE_BYTES}-byte limit");
        }
        digest.update(&chunk);
        file.write_all(&chunk)
            .await
            .with_context(|| format!("failed to write {}", path.display()))?;
    }
    file.sync_all()
        .await
        .with_context(|| format!("failed to sync {}", path.display()))?;
    Ok(format!("{:x}", digest.finalize()))
}

fn require_success(response: &reqwest::Response, label: &str) -> Result<()> {
    let status = response.status();
    if status == StatusCode::OK {
        Ok(())
    } else {
        bail!("{label} request failed with HTTP {status}")
    }
}

fn locate_installer() -> Result<PathBuf> {
    let executable = std::env::current_exe().context("failed to locate the current hzr binary")?;
    let bundle_candidate = executable
        .parent()
        .and_then(Path::parent)
        .map(|root| root.join("share/hzr/install.sh"));
    let source_candidate = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("install.sh");
    for candidate in bundle_candidate.into_iter().chain([source_candidate]) {
        let Ok(metadata) = fs::symlink_metadata(&candidate) else {
            continue;
        };
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            continue;
        }
        return candidate
            .canonicalize()
            .with_context(|| format!("failed to resolve {}", candidate.display()));
    }
    bail!("the verified HZR installer is absent; reinstall HZR from the official release")
}

fn invoke_installer(
    installer: &Path,
    release: &AvailableRelease,
    archive: &Path,
    checksums: &Path,
    json: bool,
) -> Result<()> {
    let executable = std::env::current_exe().context("failed to locate the current hzr binary")?;
    let mut command = Command::new("/bin/sh");
    command
        .arg(installer)
        .env("HZR_VERSION", release.version.to_string())
        .env("HZR_ARCHIVE_PATH", archive)
        .env("HZR_CHECKSUMS_PATH", checksums);
    if let Some(install_root) = infer_install_root(&executable) {
        command.env("HZR_INSTALL_ROOT", install_root);
    }
    if let Some(bin_dir) = infer_bin_dir(&executable) {
        command.env("HZR_BIN_DIR", bin_dir);
    }
    if json {
        command.stdout(Stdio::null()).stderr(Stdio::null());
    }
    let status = command
        .status()
        .with_context(|| format!("failed to run {}", installer.display()))?;
    if !status.success() {
        bail!("verified HZR installer exited with {status}");
    }
    Ok(())
}

fn infer_install_root(executable: &Path) -> Option<PathBuf> {
    executable.ancestors().find_map(|ancestor| {
        (ancestor.file_name().is_some_and(|name| name == "versions"))
            .then(|| ancestor.parent().map(Path::to_path_buf))
            .flatten()
    })
}

fn infer_bin_dir(executable: &Path) -> Option<PathBuf> {
    let argument = std::env::args_os()
        .next()
        .unwrap_or_else(|| OsString::from("hzr"));
    let candidates = if Path::new(&argument).components().count() > 1 {
        vec![PathBuf::from(argument)]
    } else {
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .map(|directory| directory.join(&argument))
            .collect()
    };
    candidates.into_iter().find_map(|candidate| {
        let resolved = candidate.canonicalize().ok()?;
        (resolved == executable)
            .then(|| candidate.parent().map(Path::to_path_buf))
            .flatten()
    })
}

fn current_platform() -> Result<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Ok("darwin-arm64"),
        ("macos", "x86_64") => Ok("darwin-x64"),
        ("linux", "aarch64") => Ok("linux-arm64"),
        ("linux", "x86_64") => Ok("linux-x64"),
        (os, architecture) => bail!("unsupported update platform: {os}-{architecture}"),
    }
}

fn cache_path(data_dir: &Path) -> PathBuf {
    data_dir.join("runtime/update-check.json")
}

fn read_cache(data_dir: &Path) -> Result<CachedRelease> {
    let path = cache_path(data_dir);
    let metadata = fs::symlink_metadata(&path)
        .with_context(|| format!("failed to inspect {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > 16_384 {
        bail!("invalid update cache file: {}", path.display());
    }
    let bytes = fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_slice(&bytes).context("failed to parse the update cache")
}

fn write_cache(data_dir: &Path, latest: Option<ReleaseVersion>) -> Result<()> {
    let path = cache_path(data_dir);
    let parent = path.parent().context("update cache path has no parent")?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    set_private_directory(parent)?;
    let cached = CachedRelease {
        checked_at_unix_seconds: unix_seconds(),
        latest_version: latest.map(|version| version.to_string()),
    };
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("failed to stage {}", path.display()))?;
    serde_json::to_writer(&mut temporary, &cached).context("failed to serialize update cache")?;
    temporary
        .write_all(b"\n")
        .context("failed to finish update cache")?;
    temporary
        .as_file()
        .sync_all()
        .context("failed to sync update cache")?;
    set_private_file(temporary.path())?;
    temporary
        .persist(&path)
        .map_err(|error| error.error)
        .with_context(|| format!("failed to atomically replace {}", path.display()))?;
    Ok(())
}

fn write_private_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file =
        fs::File::create(path).with_context(|| format!("failed to create {}", path.display()))?;
    file.write_all(bytes)
        .with_context(|| format!("failed to write {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("failed to sync {}", path.display()))?;
    set_private_file(path)
}

#[cfg(unix)]
fn set_private_directory(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("failed to protect {}", path.display()))
}

#[cfg(not(unix))]
fn set_private_directory(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_private_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("failed to protect {}", path.display()))
}

#[cfg(not(unix))]
fn set_private_file(_path: &Path) -> Result<()> {
    Ok(())
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn parse_release_version(value: &str) -> Result<ReleaseVersion> {
    let value = value.strip_prefix('v').unwrap_or(value);
    let components = value.split('.').collect::<Vec<_>>();
    if components.len() != 3 {
        bail!("release version must have exactly three numeric components: {value:?}");
    }
    let mut parsed = [0_u64; 3];
    for (index, component) in components.into_iter().enumerate() {
        if component.is_empty()
            || !component.bytes().all(|byte| byte.is_ascii_digit())
            || (component.len() > 1 && component.starts_with('0'))
        {
            bail!("invalid release version component: {component:?}");
        }
        parsed[index] = component
            .parse::<u64>()
            .with_context(|| format!("release version component is too large: {component:?}"))?;
    }
    Ok(ReleaseVersion(parsed))
}

fn select_release(
    response: &[u8],
    current: ReleaseVersion,
    platform: &str,
) -> Result<Option<AvailableRelease>> {
    let releases = serde_json::from_slice::<Vec<GithubRelease>>(response)
        .context("GitHub returned an invalid releases response")?;
    let mut selected = None;
    for release in releases {
        if release.draft {
            continue;
        }
        let Ok(version) = parse_release_version(&release.tag_name) else {
            continue;
        };
        if version <= current {
            continue;
        }
        let tag = format!("v{version}");
        if release.tag_name != tag {
            continue;
        }
        let archive_name = format!("hzr-v{version}-{platform}.tar.gz");
        let release_root = format!("{RELEASE_DOWNLOAD_ROOT}/{tag}");
        let archive_url = format!("{release_root}/{archive_name}");
        let checksums_url = format!("{release_root}/SHA256SUMS");
        let has_archive = release
            .assets
            .iter()
            .any(|asset| asset.name == archive_name && asset.browser_download_url == archive_url);
        let has_checksums = release
            .assets
            .iter()
            .any(|asset| asset.name == "SHA256SUMS" && asset.browser_download_url == checksums_url);
        if !has_archive || !has_checksums {
            continue;
        }
        let candidate = AvailableRelease {
            version,
            tag,
            prerelease: release.prerelease,
            archive_name,
            archive_url,
            checksums_url,
        };
        if selected
            .as_ref()
            .is_none_or(|existing: &AvailableRelease| candidate.version > existing.version)
        {
            selected = Some(candidate);
        }
    }
    Ok(selected)
}

fn checksum_for_artifact<'a>(manifest: &'a str, artifact: &str) -> Result<&'a str> {
    let matches = manifest
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let digest = fields.next()?;
            let name = fields.next()?;
            (fields.next().is_none() && name == artifact).then_some(digest)
        })
        .collect::<Vec<_>>();
    if matches.len() != 1 {
        bail!("SHA256SUMS must contain exactly one entry for {artifact}");
    }
    let digest = matches[0];
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("SHA256SUMS contains an invalid SHA-256 for {artifact}");
    }
    Ok(digest)
}

fn classify_cache(cached: &CachedRelease, now: u64, current: ReleaseVersion) -> CacheStatus {
    let age = now.saturating_sub(cached.checked_at_unix_seconds);
    let latest = cached
        .latest_version
        .as_deref()
        .and_then(|version| parse_release_version(version).ok());
    let ttl = match latest {
        Some(version) if version > current => AVAILABLE_CACHE_TTL_SECONDS,
        _ => CURRENT_CACHE_TTL_SECONDS,
    };
    if now < cached.checked_at_unix_seconds || age >= ttl {
        return CacheStatus::Expired;
    }
    match latest {
        Some(version) if version > current => CacheStatus::FreshNewer(version),
        _ => CacheStatus::FreshCurrent,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CacheStatus, CachedRelease, checksum_for_artifact, classify_cache, notice,
        parse_release_version, select_release, session_start_payload, startup_notice, write_cache,
    };
    use tempfile::tempdir;

    #[test]
    fn selects_highest_usable_release_including_github_prereleases() {
        let releases = br#"[
            {"tag_name":"v0.4.0","draft":false,"prerelease":true,"assets":[
                {"name":"hzr-v0.4.0-darwin-arm64.tar.gz","browser_download_url":"https://github.com/heAdz0r/hzr/releases/download/v0.4.0/hzr-v0.4.0-darwin-arm64.tar.gz"},
                {"name":"SHA256SUMS","browser_download_url":"https://github.com/heAdz0r/hzr/releases/download/v0.4.0/SHA256SUMS"}
            ]},
            {"tag_name":"v0.5.0","draft":true,"prerelease":false,"assets":[]},
            {"tag_name":"not-a-version","draft":false,"prerelease":false,"assets":[]},
            {"tag_name":"v0.3.2","draft":false,"prerelease":false,"assets":[]}
        ]"#;

        let current = parse_release_version("0.3.2").expect("current version");
        let release = select_release(releases, current, "darwin-arm64")
            .expect("valid GitHub response")
            .expect("newer usable release");

        assert_eq!(release.version.to_string(), "0.4.0");
        assert_eq!(release.tag, "v0.4.0");
        assert!(release.prerelease);
    }

    #[test]
    fn checksum_manifest_requires_one_exact_artifact_entry() {
        let artifact = "hzr-v0.4.0-linux-x64.tar.gz";
        let digest = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let manifest = format!("{digest}  {artifact}\n");

        assert_eq!(
            checksum_for_artifact(&manifest, artifact).expect("exact checksum"),
            digest
        );
        assert!(checksum_for_artifact(&format!("{manifest}{manifest}"), artifact).is_err());
        assert!(checksum_for_artifact(&manifest, "hzr-v0.4.0-linux-arm64.tar.gz").is_err());
    }

    #[test]
    fn available_cache_lasts_one_day_but_negative_cache_expires_hourly() {
        let cached = CachedRelease {
            checked_at_unix_seconds: 1_000,
            latest_version: Some("0.4.0".to_owned()),
        };
        let current = parse_release_version("0.3.2").expect("current version");

        assert_eq!(
            classify_cache(&cached, 1_000 + 86_399, current),
            CacheStatus::FreshNewer(parse_release_version("0.4.0").expect("cached version"))
        );
        assert_eq!(
            classify_cache(&cached, 1_000 + 86_400, current),
            CacheStatus::Expired
        );

        let no_update = CachedRelease {
            checked_at_unix_seconds: 1_000,
            latest_version: None,
        };
        assert_eq!(
            classify_cache(&no_update, 1_000 + 3_599, current),
            CacheStatus::FreshCurrent
        );
        assert_eq!(
            classify_cache(&no_update, 1_000 + 3_600, current),
            CacheStatus::Expired,
            "a check made before a release must not suppress notifications for a full day"
        );
    }

    #[test]
    fn startup_notice_names_the_only_install_command() {
        let current = parse_release_version("0.3.2").expect("current version");
        let latest = parse_release_version("0.4.0").expect("latest version");

        assert_eq!(
            notice(current, latest),
            "HZR 0.4.0 is available (current 0.3.2). Run `hzr update` to install it."
        );
    }

    #[test]
    fn session_start_notice_is_visible_to_both_user_and_agent() {
        let message = "HZR 0.4.0 is available (current 0.3.2). Run `hzr update` to install it.";
        let payload = session_start_payload(message);

        assert_eq!(payload["systemMessage"], message);
        assert_eq!(
            payload["hookSpecificOutput"]["hookEventName"],
            "SessionStart"
        );
        let context = payload["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .expect("agent context");
        assert!(context.contains(message));
        assert!(context.contains("Inform the user once"));
        assert!(context.contains("Do not install it without explicit approval"));
    }

    #[tokio::test]
    async fn startup_notice_uses_a_cached_newer_release_without_network() {
        let data = tempdir().expect("temporary data directory");
        write_cache(data.path(), Some(super::ReleaseVersion([99, 0, 0]))).expect("update cache");

        let message = startup_notice(data.path())
            .await
            .expect("cached update notice");

        assert!(message.contains("HZR 99.0.0 is available"));
        assert!(message.contains("Run `hzr update`"));
    }

    #[test]
    fn release_assets_must_use_the_official_github_download_url() {
        let releases = br#"[{"tag_name":"v0.4.0","draft":false,"prerelease":false,"assets":[
            {"name":"hzr-v0.4.0-linux-x64.tar.gz","browser_download_url":"https://example.invalid/hzr-v0.4.0-linux-x64.tar.gz"},
            {"name":"SHA256SUMS","browser_download_url":"https://github.com/heAdz0r/hzr/releases/download/v0.4.0/SHA256SUMS"}
        ]}]"#;
        let current = parse_release_version("0.3.2").expect("current version");

        assert!(
            select_release(releases, current, "linux-x64")
                .expect("valid GitHub response")
                .is_none()
        );
    }
}
