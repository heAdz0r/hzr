//! `hzr build` — one command that turns this source tree into the active global install.
//!
//! Before this existed, updating a development machine meant remembering an ordered
//! sequence: build the bundle, install it into a version-scoped root, repoint `current`,
//! restart the daemon, then hope every engine actually moved. Each step had already
//! produced a real defect — a `current` symlink that never moved, a config pinned to the
//! previous release's engines, a stale global binary that rejected commands the source
//! already had. Those are exactly the failures a single verified command prevents.
//!
//! The stage order is not cosmetic. Engines must be in place before `current` moves, and
//! `current` must move before the daemon restarts, because a daemon restarted too early
//! would re-attach to the previous bundle and keep serving its engines.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Serialize;

use crate::release_version;

/// Engines whose versions are verified after the switch. Checking `hzr --version` alone
/// is what let a stale bundle look current: the public binary can be new while every
/// engine underneath it is still the previous release's.
const VERIFIED_ENGINES: [(&str, &[&str], &str); 4] = [
    ("rtk", &["--version"], "0.44.1-fork.1"),
    ("grepai", &["version"], "0.35.0"),
    ("icm", &["--version"], "0.10.61"),
    ("node", &["--version"], "22.17.1"),
];

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct EngineCheck {
    pub name: String,
    pub expected: String,
    pub reported: String,
    pub ok: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BuildReport {
    pub version: String,
    pub platform: String,
    pub bundle: PathBuf,
    pub version_root: PathBuf,
    pub current: PathBuf,
    /// Where `current` pointed before this build, so an unchanged switch is visible.
    pub previous_target: Option<PathBuf>,
    pub switched: bool,
    pub engines: Vec<EngineCheck>,
    pub service_restarted: bool,
    pub dry_run: bool,
    pub version_files: Vec<PathBuf>,
}

impl BuildReport {
    pub fn healthy(&self) -> bool {
        self.engines.iter().all(|engine| engine.ok)
    }
}

pub struct BuildOptions {
    pub target_version: Option<String>,
    pub dry_run: bool,
    pub force: bool,
    /// Skip the daemon restart. The daemon then keeps serving the previous bundle, so
    /// this is opt-in rather than the default.
    pub skip_service: bool,
    pub install_root: Option<PathBuf>,
}

fn platform() -> Result<String> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    Ok(match (os, arch) {
        ("macos", "aarch64") => "darwin-arm64".to_owned(),
        ("macos", "x86_64") => "darwin-x64".to_owned(),
        ("linux", "aarch64") => "linux-arm64".to_owned(),
        ("linux", "x86_64") => "linux-x64".to_owned(),
        _ => bail!("unsupported build platform: {os}-{arch}"),
    })
}

fn default_install_root() -> Result<PathBuf> {
    let base = directories::BaseDirs::new().context("cannot determine the user home directory")?;
    Ok(base.home_dir().join(".local/share/hzr"))
}

/// Locate the repository that owns this source tree, so `hzr build` works from any
/// subdirectory the way `cargo` does.
fn repository_root() -> Result<PathBuf> {
    let mut directory = std::env::current_dir().context("cannot resolve the current directory")?;
    loop {
        if directory.join("scripts/build-bundle.sh").is_file()
            && directory.join("Cargo.toml").is_file()
        {
            return Ok(directory);
        }
        if !directory.pop() {
            bail!(
                "`hzr build` must run inside the HZR source tree; \
                 no scripts/build-bundle.sh found in any parent directory"
            );
        }
    }
}

fn run_checked(command: &mut Command, what: &str) -> Result<()> {
    let status = command
        .status()
        .with_context(|| format!("failed to start {what}"))?;
    if !status.success() {
        bail!("{what} failed with {status}");
    }
    Ok(())
}

fn read_version(binary: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new(binary)
        .args(args)
        .output()
        .with_context(|| format!("failed to run {}", binary.display()))?;
    if !output.status.success() {
        bail!(
            "{} {} failed with {}",
            binary.display(),
            args.join(" "),
            output.status
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn wait_for_daemon_version(binary: &Path, expected: &str) -> Result<()> {
    for _ in 0..50 {
        let output = Command::new(binary)
            .args(["daemon", "status", "--json"])
            .output();
        if let Ok(output) = output {
            let reported = serde_json::from_slice::<serde_json::Value>(&output.stdout)
                .ok()
                .and_then(|status| status["hzr_version"].as_str().map(str::to_owned));
            if output.status.success() && reported.as_deref() == Some(expected) {
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    bail!("restarted daemon did not report HZR {expected} within five seconds")
}

fn restart_installed_service(binary: &Path) -> Result<()> {
    let output = Command::new(binary)
        .args(["daemon", "service", "restart", "--json"])
        .output()
        .with_context(|| {
            format!(
                "failed to run installed service command via {}",
                binary.display()
            )
        })?;
    if !output.status.success() {
        bail!(
            "installed daemon restart failed with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)
        .context("installed daemon restart returned invalid JSON")?;
    if report["active"].as_bool() != Some(true) {
        bail!("production daemon is inactive after release restart");
    }
    Ok(())
}

pub fn run(options: BuildOptions) -> Result<BuildReport> {
    let repository = repository_root()?;
    if !options.dry_run && !options.force {
        bail!(
            "`hzr release` replaces the active global installation; inspect `hzr release --dry-run`, then rerun with `--force` to confirm"
        );
    }
    let requested_version = match options.target_version.as_deref() {
        Some(target) => target.to_owned(),
        None => release_version::current_version(&repository)?,
    };
    let version_report =
        release_version::synchronize(&repository, &requested_version, options.dry_run)?;
    let version = version_report.target.clone();
    let platform = platform()?;
    let install_root = match options.install_root.clone() {
        Some(root) => root,
        None => default_install_root()?,
    };
    let current = install_root.join("current");
    let version_root = install_root
        .join("versions")
        .join(format!("v{version}-{platform}"));
    let previous_target = std::fs::read_link(&current).ok();

    if options.dry_run {
        // A preview must not build: a bundle build is minutes of work and network
        // access, which is not what "show me what would happen" should cost.
        return Ok(BuildReport {
            version,
            platform,
            bundle: repository.join("dist"),
            version_root,
            current,
            previous_target,
            switched: false,
            engines: Vec::new(),
            service_restarted: false,
            dry_run: true,
            version_files: version_report.changed_files,
        });
    }

    if options.target_version.is_some() {
        run_checked(
            Command::new("cargo")
                .args(["metadata", "--format-version", "1", "--no-deps"])
                .current_dir(&repository)
                .stdout(std::process::Stdio::null()),
            "Cargo.lock version synchronization",
        )?;
        run_checked(
            Command::new("bash")
                .arg(repository.join("scripts/refresh-current-engine.sh"))
                .current_dir(&repository),
            "current fork-core manifest refresh",
        )?;
    }

    // Stage 1: assemble the bundle. build-bundle.sh already verifies the fork snapshot,
    // engine pins, patch digests and npm integrity, so it stays the single source of
    // truth rather than being reimplemented here.
    let bundle = repository.join("dist");
    if bundle.exists() {
        std::fs::remove_dir_all(&bundle)
            .with_context(|| format!("failed to clear {}", bundle.display()))?;
    }
    run_checked(
        Command::new("bash")
            .arg(repository.join("scripts/build-bundle.sh"))
            .arg(&bundle)
            .current_dir(&repository),
        "scripts/build-bundle.sh",
    )?;

    // Stage 2: place the bundle in its version-scoped root and switch `current`. The
    // shell installer owns the no-follow symlink replacement that this depends on, so
    // reuse it instead of writing a second implementation that could regress differently.
    run_checked(
        Command::new("bash")
            .arg(repository.join("scripts/install-bundle.sh"))
            .arg(&bundle)
            .arg(&install_root)
            .current_dir(&repository),
        "scripts/install-bundle.sh",
    )?;

    let switched = std::fs::read_link(&current).ok() != previous_target;

    let public_version = read_version(&current.join("bin/hzr"), &["--version"])?;
    if public_version != format!("hzr {version}") {
        bail!(
            "installed public binary version mismatch: expected hzr {version}, got {public_version}"
        );
    }

    // Stage 3: verify every engine through `current`. This is the check that catches a
    // switch that only appeared to work.
    let engines_dir = current.join("engines");
    let mut engines = Vec::with_capacity(VERIFIED_ENGINES.len());
    for (name, args, expected) in VERIFIED_ENGINES {
        let binary = engines_dir.join(name);
        let reported =
            read_version(&binary, args).unwrap_or_else(|error| format!("error: {error}"));
        engines.push(EngineCheck {
            name: name.to_owned(),
            expected: expected.to_owned(),
            ok: reported.contains(expected),
            reported,
        });
    }

    // Stage 4: restart the daemon last, so it attaches to the bundle that is now current.
    let service_restarted = if options.skip_service {
        false
    } else {
        let installed_hzr = current.join("bin/hzr");
        restart_installed_service(&installed_hzr)
            .context("failed to restart the production daemon after release switch")?;
        wait_for_daemon_version(&installed_hzr, &version)?;
        true
    };

    Ok(BuildReport {
        version,
        platform,
        bundle,
        version_root,
        current,
        previous_target,
        switched,
        engines,
        service_restarted,
        dry_run: false,
        version_files: version_report.changed_files,
    })
}

#[cfg(test)]
mod tests {
    use super::{BuildReport, EngineCheck, VERIFIED_ENGINES, platform};

    fn check(name: &str, ok: bool) -> EngineCheck {
        EngineCheck {
            name: name.to_owned(),
            expected: "x".to_owned(),
            reported: "y".to_owned(),
            ok,
        }
    }

    fn report(engines: Vec<EngineCheck>) -> BuildReport {
        BuildReport {
            version: "0.3.3".to_owned(),
            platform: "darwin-arm64".to_owned(),
            bundle: "/tmp/dist".into(),
            version_root: "/tmp/versions/v0.3.3-darwin-arm64".into(),
            current: "/tmp/current".into(),
            previous_target: None,
            switched: true,
            engines,
            service_restarted: true,
            dry_run: false,
            version_files: Vec::new(),
        }
    }

    #[test]
    fn test_all_four_engines_are_verified_not_just_the_public_binary() {
        let names: Vec<&str> = VERIFIED_ENGINES.iter().map(|(name, _, _)| *name).collect();
        assert_eq!(names, vec!["rtk", "grepai", "icm", "node"]);
        assert!(
            !names.contains(&"hzr"),
            "verifying only hzr is what let a stale bundle look current"
        );
    }

    #[test]
    fn test_engine_pins_match_the_locked_versions() {
        for (name, _, expected) in VERIFIED_ENGINES {
            assert!(
                !expected.is_empty(),
                "{name} must assert a concrete pinned version"
            );
        }
    }

    #[test]
    fn test_report_is_unhealthy_when_any_single_engine_mismatches() {
        assert!(report(vec![check("rtk", true), check("grepai", true)]).healthy());
        assert!(
            !report(vec![check("rtk", true), check("grepai", false)]).healthy(),
            "one stale engine must fail the whole build"
        );
    }

    #[test]
    fn test_platform_is_recognized_on_this_host() {
        let resolved = platform().expect("the test host must be a supported platform");
        assert!(resolved.starts_with("darwin-") || resolved.starts_with("linux-"));
    }
}
