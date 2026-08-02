use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Serialize;
use sha2::{Digest, Sha256};

const ROOT_VERSION_SURFACES: &[&str] = &[
    "AGENTS.md",
    "Cargo.toml",
    "FORK_PARITY.md",
    "HZR.md",
    "README.md",
    "install.sh",
];

const VERSION_SURFACES: &[&str] = &[
    ".github/workflows/ci.yml",
    "crates/hzr-cli/src/adoption.rs",
    "crates/hzr-cli/src/build.rs",
    "crates/hzr-cli/src/client.rs",
    "crates/hzr-cli/src/main.rs",
    "crates/hzr-cli/src/service.rs",
    "crates/hzr-cli/src/stats_output.rs",
    "crates/hzr-core/src/config.rs",
    "crates/hzr-core/src/ledger.rs",
    "integrations/caveman-code/bridge.mjs",
    "integrations/caveman-code/bridge.test.mjs",
    "integrations/caveman-code/package.json",
    "scripts/smoke-bundle.sh",
    "scripts/smoke-install.sh",
];

const CAVEMAN_PACKAGE_LOCK: &str = "integrations/caveman-code/package-lock.json";
const CARGO_LOCK: &str = "Cargo.lock";

const RUNTIME_DIGEST_SURFACES: &[(&str, &str)] = &[
    (
        "integrations/caveman-code/bridge.mjs",
        "${HZR_CAVEMAN_ROOT}/bridge.mjs",
    ),
    (
        "integrations/caveman-code/package.json",
        "${HZR_CAVEMAN_ROOT}/package.json",
    ),
    (
        "integrations/caveman-code/package-lock.json",
        "${HZR_CAVEMAN_ROOT}/package-lock.json",
    ),
];

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct VersionReport {
    pub previous: String,
    pub target: String,
    pub changed_files: Vec<PathBuf>,
    pub dry_run: bool,
}

pub fn current_version(repository: &Path) -> Result<String> {
    let cargo_text =
        fs::read_to_string(repository.join("Cargo.toml")).context("failed to read Cargo.toml")?;
    let document = cargo_text
        .parse::<toml_edit::DocumentMut>()
        .context("failed to parse Cargo.toml")?;
    let version = document["workspace"]["package"]["version"]
        .as_str()
        .context("Cargo.toml is missing workspace.package.version")?
        .to_owned();
    validate_version(&version)?;
    Ok(version)
}

pub fn synchronize(repository: &Path, target: &str, dry_run: bool) -> Result<VersionReport> {
    validate_version(target)?;
    let cargo_path = repository.join("Cargo.toml");
    let previous = current_version(repository)?;

    let mut changed_files = Vec::new();
    if previous != target {
        for relative in tracked_version_surfaces(repository)? {
            let path = repository.join(&relative);
            let Ok(before) = fs::read_to_string(&path) else {
                continue;
            };
            if !before.contains(&previous) {
                continue;
            }
            let after = if relative == Path::new("Cargo.toml") {
                replace_cargo_manifest_version(&before, &previous, target)?
            } else {
                before.replace(&previous, target)
            };
            if !dry_run {
                atomic_replace(&path, after.as_bytes())?;
            }
            changed_files.push(relative);
        }
        synchronize_cargo_lock(repository, &previous, target, dry_run, &mut changed_files)?;
        synchronize_caveman_package_lock(
            repository,
            &previous,
            target,
            dry_run,
            &mut changed_files,
        )?;
        let release_line_path = repository.join("scripts/refresh-current-engine.sh");
        let before = fs::read_to_string(&release_line_path)
            .context("failed to read scripts/refresh-current-engine.sh")?;
        let after = synchronize_release_line(&before, &previous, target)?;
        if before != after {
            if !dry_run {
                atomic_replace(&release_line_path, after.as_bytes())?;
            }
            changed_files.push(PathBuf::from("scripts/refresh-current-engine.sh"));
        }
        let changelog = repository.join("CHANGELOG.md");
        let before = fs::read_to_string(&changelog).context("failed to read CHANGELOG.md")?;
        let after = release_changelog(&before, &previous, target, release_date()?)?;
        if before != after {
            if !dry_run {
                atomic_replace(&changelog, after.as_bytes())?;
            }
            changed_files.push(PathBuf::from("CHANGELOG.md"));
        }
    }
    synchronize_runtime_digests(repository, &previous, target, dry_run, &mut changed_files)?;
    changed_files.sort();
    changed_files.dedup();
    if !dry_run && previous != target {
        let cargo_after = fs::read_to_string(&cargo_path).context("failed to reread Cargo.toml")?;
        let expected = format!("version = \"{target}\"");
        if !cargo_after.contains(&expected) {
            bail!("version synchronization did not update workspace.package.version");
        }
    }

    Ok(VersionReport {
        previous,
        target: target.to_owned(),
        changed_files,
        dry_run,
    })
}

fn synchronize_runtime_digests(
    repository: &Path,
    previous: &str,
    target: &str,
    dry_run: bool,
    changed_files: &mut Vec<PathBuf>,
) -> Result<()> {
    let smoke_relative = PathBuf::from("scripts/smoke-bundle.sh");
    let smoke_path = repository.join(&smoke_relative);
    let smoke_before = fs::read_to_string(&smoke_path)
        .context("failed to read scripts/smoke-bundle.sh for runtime digest synchronization")?;
    let mut smoke_after = if dry_run && previous != target {
        smoke_before.replace(previous, target)
    } else {
        smoke_before.clone()
    };

    let mut package_lock_digest = None;
    for (source_relative, bundle_artifact) in RUNTIME_DIGEST_SURFACES {
        let source_path = repository.join(source_relative);
        let source_before = fs::read_to_string(&source_path).with_context(|| {
            format!("failed to read {source_relative} for digest synchronization")
        })?;
        let source_after = if dry_run && previous != target {
            if *source_relative == CAVEMAN_PACKAGE_LOCK {
                replace_caveman_package_lock_versions(&source_before, previous, target)?
            } else {
                source_before.replace(previous, target)
            }
        } else {
            source_before
        };
        let digest = format!("{:x}", Sha256::digest(source_after.as_bytes()));
        smoke_after = replace_digest_for_artifact(&smoke_after, bundle_artifact, &digest)?;
        if *source_relative == "integrations/caveman-code/package-lock.json" {
            package_lock_digest = Some(digest);
        }
    }

    if smoke_after != smoke_before {
        if !dry_run {
            atomic_replace(&smoke_path, smoke_after.as_bytes())?;
        }
        changed_files.push(smoke_relative);
    }

    let package_lock_digest = package_lock_digest.context("package-lock digest surface missing")?;
    let preflight_relative = PathBuf::from("crates/hzr-agent/src/preflight.rs");
    let preflight_path = repository.join(&preflight_relative);
    let preflight_before = fs::read_to_string(&preflight_path)
        .context("failed to read Caveman preflight digest pin")?;
    let (preflight_after, previous_digest) = replace_digest_after_marker(
        &preflight_before,
        "pub const PACKAGE_LOCK_SHA256",
        &package_lock_digest,
    )?;
    if preflight_after != preflight_before {
        if !dry_run {
            atomic_replace(&preflight_path, preflight_after.as_bytes())?;
        }
        changed_files.push(preflight_relative);
    }

    let readme_relative = PathBuf::from("integrations/caveman-code/README.md");
    let readme_path = repository.join(&readme_relative);
    let readme_before = fs::read_to_string(&readme_path)
        .context("failed to read Caveman bridge digest documentation")?;
    let readme_after = readme_before.replace(&previous_digest, &package_lock_digest);
    if readme_after != readme_before {
        if !dry_run {
            atomic_replace(&readme_path, readme_after.as_bytes())?;
        }
        changed_files.push(readme_relative);
    } else if !readme_before.contains(&package_lock_digest) {
        bail!("Caveman bridge README is missing its package-lock digest pin");
    }
    Ok(())
}

fn replace_digest_for_artifact(before: &str, artifact: &str, digest: &str) -> Result<String> {
    let artifact_marker = format!("  \"{artifact}\"");
    let artifact_position = before
        .find(&artifact_marker)
        .with_context(|| format!("smoke bundle is missing digest target {artifact}"))?;
    let prefix = &before[..artifact_position];
    let digest_end = prefix
        .rfind('"')
        .context("smoke bundle digest is missing a closing quote")?;
    let digest_start = prefix[..digest_end]
        .rfind('"')
        .context("smoke bundle digest is missing an opening quote")?
        + 1;
    let existing = &before[digest_start..digest_end];
    if existing.len() != 64 || !existing.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("invalid pinned SHA-256 for {artifact}: {existing:?}");
    }
    let mut after = before.to_owned();
    after.replace_range(digest_start..digest_end, digest);
    Ok(after)
}

fn replace_digest_after_marker(
    before: &str,
    marker: &str,
    digest: &str,
) -> Result<(String, String)> {
    let marker_position = before
        .find(marker)
        .with_context(|| format!("digest source is missing marker {marker:?}"))?;
    let relative_start = before[marker_position..]
        .find('"')
        .with_context(|| format!("digest marker {marker:?} has no opening quote"))?;
    let digest_start = marker_position + relative_start + 1;
    let relative_end = before[digest_start..]
        .find('"')
        .with_context(|| format!("digest marker {marker:?} has no closing quote"))?;
    let digest_end = digest_start + relative_end;
    let existing = &before[digest_start..digest_end];
    if existing.len() != 64 || !existing.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("invalid SHA-256 after {marker:?}: {existing:?}");
    }
    let mut after = before.to_owned();
    after.replace_range(digest_start..digest_end, digest);
    Ok((after, existing.to_owned()))
}

fn tracked_version_surfaces(repository: &Path) -> Result<Vec<PathBuf>> {
    let output = Command::new("git")
        .args([
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ])
        .current_dir(repository)
        .output()
        .context("failed to enumerate repository files for version synchronization")?;
    if !output.status.success() {
        bail!("git ls-files failed with {}", output.status);
    }
    let mut paths = Vec::new();
    for bytes in output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
    {
        let relative = PathBuf::from(
            std::str::from_utf8(bytes).context("repository contains a non-UTF-8 tracked path")?,
        );
        if is_version_surface(&relative) {
            paths.push(relative);
        }
    }
    for surface in ROOT_VERSION_SURFACES {
        let relative = PathBuf::from(surface);
        if repository.join(&relative).is_file() && !paths.contains(&relative) {
            paths.push(relative);
        }
    }
    Ok(paths)
}

fn is_version_surface(path: &Path) -> bool {
    ROOT_VERSION_SURFACES
        .iter()
        .any(|surface| path == Path::new(surface))
        || VERSION_SURFACES
            .iter()
            .any(|surface| path == Path::new(surface))
}

fn synchronize_caveman_package_lock(
    repository: &Path,
    previous: &str,
    target: &str,
    dry_run: bool,
    changed_files: &mut Vec<PathBuf>,
) -> Result<()> {
    let relative = PathBuf::from(CAVEMAN_PACKAGE_LOCK);
    let path = repository.join(&relative);
    let before = fs::read_to_string(&path).context("failed to read Caveman package-lock.json")?;
    let after = replace_caveman_package_lock_versions(&before, previous, target)?;
    if after != before {
        if !dry_run {
            atomic_replace(&path, after.as_bytes())?;
        }
        changed_files.push(relative);
    }
    Ok(())
}

fn replace_cargo_manifest_version(before: &str, previous: &str, target: &str) -> Result<String> {
    let mut document = before
        .parse::<toml_edit::DocumentMut>()
        .context("failed to parse Cargo.toml")?;
    let version = document["workspace"]["package"]["version"]
        .as_str()
        .context("Cargo.toml is missing workspace.package.version")?;
    if version != previous {
        bail!(
            "Cargo.toml workspace version is {version:?}, expected previous release {previous:?}"
        );
    }
    document["workspace"]["package"]["version"] = toml_edit::value(target);
    Ok(document.to_string())
}

fn synchronize_cargo_lock(
    repository: &Path,
    previous: &str,
    target: &str,
    dry_run: bool,
    changed_files: &mut Vec<PathBuf>,
) -> Result<()> {
    let relative = PathBuf::from(CARGO_LOCK);
    let path = repository.join(&relative);
    let before = fs::read_to_string(&path).context("failed to read Cargo.lock")?;
    let after = replace_cargo_lock_versions(&before, previous, target)?;
    if after != before {
        if !dry_run {
            atomic_replace(&path, after.as_bytes())?;
        }
        changed_files.push(relative);
    }
    Ok(())
}

fn replace_cargo_lock_versions(before: &str, previous: &str, target: &str) -> Result<String> {
    let mut document = before
        .parse::<toml_edit::DocumentMut>()
        .context("failed to parse Cargo.lock")?;
    let packages = document["package"]
        .as_array_of_tables_mut()
        .context("Cargo.lock is missing its package array")?;
    let mut updated = 0;
    for package in packages.iter_mut() {
        let Some(name) = package.get("name").and_then(toml_edit::Item::as_str) else {
            continue;
        };
        if !name.starts_with("hzr-") || package.contains_key("source") {
            continue;
        }
        let version = package
            .get("version")
            .and_then(toml_edit::Item::as_str)
            .with_context(|| format!("Cargo.lock workspace package {name:?} has no version"))?;
        if version != previous {
            bail!("Cargo.lock workspace package {name:?} is {version:?}, expected {previous:?}");
        }
        package["version"] = toml_edit::value(target);
        updated += 1;
    }
    if updated == 0 {
        bail!("Cargo.lock contains no HZR workspace packages at version {previous}");
    }
    Ok(document.to_string())
}

fn replace_caveman_package_lock_versions(
    before: &str,
    previous: &str,
    target: &str,
) -> Result<String> {
    let document: serde_json::Value =
        serde_json::from_str(before).context("failed to parse Caveman package-lock.json")?;
    let root_version = document
        .get("version")
        .and_then(serde_json::Value::as_str)
        .context("Caveman package-lock.json is missing its root version")?;
    let package_version = document
        .pointer("/packages//version")
        .and_then(serde_json::Value::as_str)
        .context("Caveman package-lock.json is missing packages[''].version")?;
    if root_version != previous || package_version != previous {
        bail!(
            "Caveman package-lock versions are not synchronized: root={root_version:?}, package={package_version:?}, expected={previous:?}"
        );
    }

    let root_marker = format!("  \"version\": \"{previous}\",");
    let root_replacement = format!("  \"version\": \"{target}\",");
    let package_marker = format!(
        "    \"\": {{\n      \"name\": \"@headz0r/hzr-caveman-code-bridge\",\n      \"version\": \"{previous}\","
    );
    let package_replacement = format!(
        "    \"\": {{\n      \"name\": \"@headz0r/hzr-caveman-code-bridge\",\n      \"version\": \"{target}\","
    );
    if before.lines().filter(|line| *line == root_marker).count() != 1 {
        bail!("Caveman package-lock root version marker is missing or ambiguous");
    }
    if before.matches(&package_marker).count() != 1 {
        bail!("Caveman package-lock package version marker is missing or ambiguous");
    }
    let after = before
        .replacen(&root_marker, &root_replacement, 1)
        .replacen(&package_marker, &package_replacement, 1);
    let updated: serde_json::Value =
        serde_json::from_str(&after).context("updated Caveman package-lock.json is invalid")?;
    if updated.get("version").and_then(serde_json::Value::as_str) != Some(target)
        || updated
            .pointer("/packages//version")
            .and_then(serde_json::Value::as_str)
            != Some(target)
    {
        bail!("Caveman package-lock version update did not reach both root fields");
    }
    Ok(after)
}

fn atomic_replace(path: &Path, bytes: &[u8]) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!(
            "refusing to replace non-regular version surface {}",
            path.display()
        );
    }
    let parent = path
        .parent()
        .with_context(|| format!("version surface has no parent: {}", path.display()))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("failed to stage {}", path.display()))?;
    temporary.write_all(bytes)?;
    temporary
        .as_file()
        .set_permissions(metadata.permissions())?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("failed to replace {}", path.display()))?;
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

fn validate_version(version: &str) -> Result<()> {
    let parts = version.split('.').collect::<Vec<_>>();
    if parts.len() != 3
        || parts.iter().any(|part| {
            part.is_empty()
                || !part.bytes().all(|byte| byte.is_ascii_digit())
                || (part.len() > 1 && part.starts_with('0'))
        })
    {
        bail!("release version must be canonical MAJOR.MINOR.PATCH, got {version:?}");
    }
    Ok(())
}

fn release_line(version: &str) -> Result<String> {
    validate_version(version)?;
    let mut parts = version.split('.');
    let major = parts.next().context("validated version has no major")?;
    let minor = parts.next().context("validated version has no minor")?;
    Ok(format!("{major}.{minor}.x"))
}

fn synchronize_release_line(before: &str, previous: &str, target: &str) -> Result<String> {
    let previous_line = release_line(previous)?;
    let target_line = release_line(target)?;
    if previous_line == target_line {
        return Ok(before.to_owned());
    }
    let after = before.replace(
        &format!("hzr_release_line = \"{previous_line}\""),
        &format!("hzr_release_line = \"{target_line}\""),
    );
    if before == after {
        bail!("scripts/refresh-current-engine.sh did not contain release line {previous_line}");
    }
    Ok(after)
}

fn release_date() -> Result<String> {
    let output = Command::new("date")
        .arg("+%Y-%m-%d")
        .output()
        .context("failed to determine release date")?;
    if !output.status.success() {
        bail!("date failed with {}", output.status);
    }
    Ok(String::from_utf8(output.stdout)
        .context("release date was not UTF-8")?
        .trim()
        .to_owned())
}

fn release_changelog(before: &str, previous: &str, target: &str, date: String) -> Result<String> {
    if before.contains(&format!("## [{target}]")) {
        return Ok(before.to_owned());
    }
    let marker = "## [Unreleased]\n\n";
    if !before.contains(marker) {
        bail!("CHANGELOG.md is missing the Unreleased section");
    }
    let mut after = before.replacen(marker, &format!("{marker}## [{target}] - {date}\n\n"), 1);
    let link =
        format!("[{target}]: https://github.com/heAdz0r/hzr/compare/v{previous}...v{target}\n");
    let first_link = format!("[{previous}]:");
    if let Some(position) = after.find(&first_link) {
        after.insert_str(position, &link);
    } else {
        after.push_str(&link);
    }
    Ok(after)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        is_version_surface, release_changelog, replace_cargo_lock_versions,
        replace_cargo_manifest_version, replace_caveman_package_lock_versions,
        replace_digest_after_marker, replace_digest_for_artifact, synchronize_release_line,
        validate_version,
    };

    #[test]
    fn same_minor_release_keeps_the_stable_release_line() {
        let before = "hzr_release_line = \"0.3.x\"\n";
        assert_eq!(
            synchronize_release_line(before, "0.3.0", "0.3.2").expect("release line"),
            before
        );
    }

    #[test]
    fn version_surfaces_exclude_internal_planning_documents() {
        assert!(is_version_surface(Path::new("README.md")));
        assert!(!is_version_surface(Path::new("PRD.md")));
        assert!(!is_version_surface(Path::new("PRD_ADOPTION.md")));
        assert!(!is_version_surface(Path::new(
            "integrations/caveman-code/package-lock.json"
        )));
        assert!(!is_version_surface(Path::new(
            "fork-core/rtk/src/filters/bundle-install.toml"
        )));
    }

    #[test]
    fn package_lock_update_preserves_dependency_versions() {
        let before = concat!(
            "{\n",
            "  \"name\": \"@headz0r/hzr-caveman-code-bridge\",\n",
            "  \"version\": \"0.3.0\",\n",
            "  \"packages\": {\n",
            "    \"\": {\n",
            "      \"name\": \"@headz0r/hzr-caveman-code-bridge\",\n",
            "      \"version\": \"0.3.0\",\n",
            "      \"dependencies\": {\"dependency\": \"^0.3.0\"}\n",
            "    },\n",
            "    \"node_modules/dependency\": {\"version\": \"0.3.0\"}\n",
            "  }\n",
            "}\n",
        );
        let after = replace_caveman_package_lock_versions(before, "0.3.0", "0.3.2")
            .expect("package-lock update");
        assert_eq!(after.matches("\"version\": \"0.3.2\"").count(), 2);
        assert!(after.contains("\"dependency\": \"^0.3.0\""));
        assert!(after.contains("\"node_modules/dependency\": {\"version\": \"0.3.0\"}"));
    }

    #[test]
    fn cargo_manifest_update_preserves_dependency_versions() {
        let before = concat!(
            "[workspace.package]\n",
            "version = \"0.3.2\"\n\n",
            "[workspace.dependencies]\n",
            "tracing-subscriber = \"0.3.29\"\n",
        );
        let after =
            replace_cargo_manifest_version(before, "0.3.2", "0.3.3").expect("Cargo.toml update");

        assert!(after.contains("version = \"0.3.3\""));
        assert!(after.contains("tracing-subscriber = \"0.3.29\""));
    }

    #[test]
    fn cargo_lock_update_changes_only_workspace_package_versions() {
        let before = concat!(
            "version = 4\n\n",
            "[[package]]\n",
            "name = \"hzr-core\"\n",
            "version = \"0.3.2\"\n\n",
            "[[package]]\n",
            "name = \"third-party\"\n",
            "version = \"0.3.2\"\n",
            "source = \"registry+https://example.invalid\"\n",
        );
        let after =
            replace_cargo_lock_versions(before, "0.3.2", "0.3.3").expect("Cargo.lock update");

        assert!(after.contains("name = \"hzr-core\"\nversion = \"0.3.3\""));
        assert!(after.contains("name = \"third-party\"\nversion = \"0.3.2\""));
    }

    #[test]
    fn version_validation_rejects_ambiguous_or_partial_versions() {
        assert!(validate_version("1.2.3").is_ok());
        for invalid in ["v1.2.3", "1.2", "1.02.3", "1.2.3-beta"] {
            assert!(validate_version(invalid).is_err(), "accepted {invalid}");
        }
    }

    #[test]
    fn changelog_moves_unreleased_entries_under_target_once() {
        let before = "# Changelog\n\n## [Unreleased]\n\n### Added\n\n- fix\n\n[1.2.3]: old\n";
        let after =
            release_changelog(before, "1.2.3", "1.3.0", "2026-08-01".into()).expect("changelog");
        assert!(after.contains("## [1.3.0] - 2026-08-01\n\n### Added"));
        assert_eq!(after.matches("## [1.3.0]").count(), 1);
        assert!(after.contains("[1.3.0]: https://github.com/heAdz0r/hzr/compare/v1.2.3...v1.3.0"));
    }

    #[test]
    fn runtime_digest_pin_is_replaced_for_the_exact_artifact() {
        let before = concat!(
            "verify_sha256 \\\n",
            "  \"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\" \\\n",
            "  \"${HZR_CAVEMAN_ROOT}/bridge.mjs\"\n",
        );
        let digest = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let after = replace_digest_for_artifact(before, "${HZR_CAVEMAN_ROOT}/bridge.mjs", digest)
            .expect("digest replacement");

        assert!(after.contains(digest));
        assert!(
            !after.contains("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
    }

    #[test]
    fn compiled_digest_pin_is_replaced_after_its_named_marker() {
        let before = concat!(
            "pub const PACKAGE_LOCK_SHA256: &str =\n",
            "    \"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\";\n",
        );
        let digest = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let (after, previous) =
            replace_digest_after_marker(before, "pub const PACKAGE_LOCK_SHA256", digest)
                .expect("compiled digest replacement");

        assert_eq!(
            previous,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
        assert!(after.contains(digest));
    }
}
