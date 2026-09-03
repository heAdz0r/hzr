use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Serialize;

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
const FIXED_VERSION_WORKSPACE_PACKAGES: &[(&str, &str)] = &[("hzr-engine-contract", "0.0.0")];

const CURRENT_VERSION_MARKERS: &[(&str, &str)] = &[
    ("AGENTS.md", "Product version is "),
    ("FORK_PARITY.md", "# HZR "),
    ("README.md", "alt=\"Version "),
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
    validate_declared_version_markers(repository, &previous, Some(target))?;

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
    changed_files.sort();
    changed_files.dedup();
    if !dry_run && previous != target {
        let cargo_after = fs::read_to_string(&cargo_path).context("failed to reread Cargo.toml")?;
        let expected = format!("version = \"{target}\"");
        if !cargo_after.contains(&expected) {
            bail!("version synchronization did not update workspace.package.version");
        }
        validate_declared_version_markers(repository, target, None)?;
    }

    Ok(VersionReport {
        previous,
        target: target.to_owned(),
        changed_files,
        dry_run,
    })
}

fn validate_declared_version_markers(
    repository: &Path,
    expected: &str,
    also_allowed: Option<&str>,
) -> Result<()> {
    for (relative, marker) in CURRENT_VERSION_MARKERS {
        let path = repository.join(relative);
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let Some(start) = text.find(marker).map(|index| index + marker.len()) else {
            continue;
        };
        let version = text[start..]
            .split(|character: char| !character.is_ascii_digit() && character != '.')
            .next()
            .unwrap_or_default();
        if validate_version(version).is_err() {
            bail!(
                "declared current-version marker in {} is malformed after `{marker}`",
                path.display()
            );
        }
        if version != expected && also_allowed != Some(version) {
            bail!(
                "declared current-version marker in {} is stale: expected {expected}, found {version}",
                path.display()
            );
        }
    }
    Ok(())
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
        if let Some((_, fixed)) = FIXED_VERSION_WORKSPACE_PACKAGES
            .iter()
            .find(|(fixed_name, _)| *fixed_name == name)
        {
            if version != *fixed {
                bail!(
                    "Cargo.lock fixed-version workspace package {name:?} is {version:?}, expected {fixed:?}"
                );
            }
            continue;
        }
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
        synchronize_release_line, validate_declared_version_markers, validate_version,
    };

    #[test]
    fn same_minor_release_keeps_the_stable_release_line() {
        let before = "hzr_release_line = \"0.3.x\"\n";
        assert_eq!(
            synchronize_release_line(before, "0.3.0", "0.3.2").expect("release line"),
            before
        );
    }

    /// One-shot helper for release assembly; ignore in CI.
    #[test]
    #[ignore = "manual: cargo test -p hzr-cli --bin hzr apply_version_sync_0_3_8 -- --ignored --exact"]
    fn apply_version_sync_0_3_8() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("repository root");
        super::synchronize(&root, "0.3.8", false).expect("synchronize 0.3.8");
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
            "name = \"hzr-engine-contract\"\n",
            "version = \"0.0.0\"\n\n",
            "[[package]]\n",
            "name = \"third-party\"\n",
            "version = \"0.3.2\"\n",
            "source = \"registry+https://example.invalid\"\n",
        );
        let after =
            replace_cargo_lock_versions(before, "0.3.2", "0.3.3").expect("Cargo.lock update");

        assert!(after.contains("name = \"hzr-core\"\nversion = \"0.3.3\""));
        assert!(after.contains("name = \"hzr-engine-contract\"\nversion = \"0.0.0\""));
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
    fn stale_declared_current_version_names_the_surface() {
        let directory = tempfile::tempdir().expect("temporary directory");
        std::fs::write(
            directory.path().join("AGENTS.md"),
            "Product version is 0.6.3;\n",
        )
        .expect("stale marker");
        let error = validate_declared_version_markers(directory.path(), "0.6.5", Some("0.6.6"))
            .expect_err("stale marker must fail");
        assert!(error.to_string().contains("AGENTS.md"));
        assert!(error.to_string().contains("found 0.6.3"));
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
}
