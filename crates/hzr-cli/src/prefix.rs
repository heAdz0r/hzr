//! Durable placement of the `hzr`/`hzrd` binaries into a PATH directory.
//!
//! Hooks and agent instructions both name `hzr`, so the binary has to exist at a
//! stable location that is already on the user's PATH. This module copies the
//! running executables into that prefix atomically and reports whether the prefix
//! is actually reachable, because a silent PATH miss would leave every instruction
//! in `CLAUDE.md` pointing at a command the agent cannot run.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use directories::BaseDirs;
use serde::Serialize;

use crate::adoption::{atomic_write, sha256};

/// Binaries that must be reachable by name. `rtk` is deliberately absent: the
/// compatibility alias is a bundle artifact, and creating it here would reintroduce
/// a second entry point on PATH.
const MANAGED_BINARIES: [&str; 2] = ["hzr", "hzrd"];

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BinaryPlacement {
    pub name: String,
    pub target: PathBuf,
    pub changed: bool,
    pub sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PrefixReport {
    pub prefix: PathBuf,
    pub on_path: bool,
    pub changed: bool,
    pub binaries: Vec<BinaryPlacement>,
}

/// Default prefix. `~/.local/bin` is where the user's other engines already live,
/// so it is the least surprising destination and usually already on PATH.
pub fn default_prefix() -> Result<PathBuf> {
    let base = BaseDirs::new().context("cannot determine the user home directory")?;
    Ok(base.home_dir().join(".local/bin"))
}

pub fn is_on_path(prefix: &Path) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    // Compare canonicalized forms so `~/.local/bin` and a symlinked equivalent match.
    let target = prefix
        .canonicalize()
        .unwrap_or_else(|_| prefix.to_path_buf());
    std::env::split_paths(&path).any(|entry| entry.canonicalize().unwrap_or(entry) == target)
}

/// Locate the sibling binary next to the running executable. Both `hzr` and `hzrd`
/// are produced into the same directory by every supported build and bundle layout.
fn source_for(name: &str, source_dir: &Path) -> Result<PathBuf> {
    let candidate = source_dir.join(name);
    if !candidate.is_file() {
        bail!(
            "cannot find `{name}` next to the running executable ({}); \
             build the workspace or run from an assembled bundle",
            source_dir.display()
        );
    }
    Ok(candidate)
}

pub fn install(
    prefix: &Path,
    source_dir: &Path,
    dry_run: bool,
    confirmed: bool,
) -> Result<PrefixReport> {
    let mut binaries = Vec::with_capacity(MANAGED_BINARIES.len());
    let mut changed = false;

    for name in MANAGED_BINARIES {
        let source = source_for(name, source_dir)?;
        let bytes = std::fs::read(&source)
            .with_context(|| format!("failed to read {}", source.display()))?;
        let target = prefix.join(name);
        let existing = match std::fs::read(&target) {
            Ok(existing) => Some(existing),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(error).with_context(|| format!("failed to read {}", target.display()));
            }
        };
        let entry_changed = existing.as_deref() != Some(bytes.as_slice());
        changed |= entry_changed;

        if entry_changed && !dry_run {
            if !confirmed {
                bail!(
                    "installing `{name}` into {} changes the filesystem; inspect \
                     `hzr install --dry-run`, then rerun with `--force` to confirm",
                    prefix.display()
                );
            }
            std::fs::create_dir_all(prefix)
                .with_context(|| format!("failed to create {}", prefix.display()))?;
            atomic_write(&target, &bytes)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;

                // atomic_write lands 0600; an executable on PATH needs the exec bit.
                std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755))
                    .with_context(|| format!("failed to mark {} executable", target.display()))?;
            }
        }

        binaries.push(BinaryPlacement {
            name: name.to_owned(),
            target,
            changed: entry_changed,
            sha256: sha256(&bytes),
        });
    }

    Ok(PrefixReport {
        prefix: prefix.to_path_buf(),
        on_path: is_on_path(prefix),
        changed,
        binaries,
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{MANAGED_BINARIES, install, is_on_path, source_for};

    fn write_fake(dir: &std::path::Path, name: &str, body: &[u8]) {
        std::fs::write(dir.join(name), body).expect("fake binary");
    }

    #[test]
    fn test_managed_binaries_exclude_rtk_alias() {
        assert!(
            !MANAGED_BINARIES.contains(&"rtk"),
            "installing an rtk alias on PATH would create a second entry point"
        );
    }

    #[test]
    fn test_source_requires_sibling_binary() {
        let temp = tempfile::tempdir().expect("temp");
        let error = source_for("hzrd", temp.path()).expect_err("missing binary must fail");
        assert!(error.to_string().contains("cannot find `hzrd`"));
    }

    #[test]
    fn test_dry_run_reports_change_without_writing() {
        let temp = tempfile::tempdir().expect("temp");
        let source = temp.path().join("src");
        let prefix = temp.path().join("bin");
        std::fs::create_dir_all(&source).expect("source dir");
        write_fake(&source, "hzr", b"one");
        write_fake(&source, "hzrd", b"two");

        let report = install(&prefix, &source, true, false).expect("dry run succeeds");
        assert!(report.changed);
        assert!(!prefix.join("hzr").exists(), "dry run must not write");
    }

    #[test]
    fn test_install_requires_confirmation() {
        let temp = tempfile::tempdir().expect("temp");
        let source = temp.path().join("src");
        let prefix = temp.path().join("bin");
        std::fs::create_dir_all(&source).expect("source dir");
        write_fake(&source, "hzr", b"one");
        write_fake(&source, "hzrd", b"two");

        let error = install(&prefix, &source, false, false).expect_err("must require --force");
        assert!(error.to_string().contains("--force"));
        assert!(!prefix.join("hzr").exists());
    }

    #[test]
    fn test_install_is_idempotent_and_executable() {
        let temp = tempfile::tempdir().expect("temp");
        let source = temp.path().join("src");
        let prefix = temp.path().join("bin");
        std::fs::create_dir_all(&source).expect("source dir");
        write_fake(&source, "hzr", b"one");
        write_fake(&source, "hzrd", b"two");

        let first = install(&prefix, &source, false, true).expect("install succeeds");
        assert!(first.changed);
        assert_eq!(std::fs::read(prefix.join("hzr")).expect("copied"), b"one");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mode = std::fs::metadata(prefix.join("hzr"))
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111, "installed binary must be executable");
        }

        let second = install(&prefix, &source, false, true).expect("reinstall succeeds");
        assert!(!second.changed, "identical bytes must be a no-op");
    }

    #[test]
    fn test_install_replaces_stale_binary() {
        let temp = tempfile::tempdir().expect("temp");
        let source = temp.path().join("src");
        let prefix = temp.path().join("bin");
        std::fs::create_dir_all(&source).expect("source dir");
        std::fs::create_dir_all(&prefix).expect("prefix dir");
        write_fake(&source, "hzr", b"new");
        write_fake(&source, "hzrd", b"new-d");
        write_fake(&prefix, "hzr", b"old");
        write_fake(&prefix, "hzrd", b"new-d");

        let report = install(&prefix, &source, false, true).expect("install succeeds");
        assert!(report.changed);
        assert_eq!(std::fs::read(prefix.join("hzr")).expect("replaced"), b"new");
        let hzrd = report
            .binaries
            .iter()
            .find(|entry| entry.name == "hzrd")
            .expect("hzrd entry");
        assert!(
            !hzrd.changed,
            "already-current binary must not be rewritten"
        );
    }

    #[test]
    fn test_path_detection_rejects_absent_directory() {
        assert!(!is_on_path(&PathBuf::from("/nonexistent/hzr/prefix")));
    }
}
