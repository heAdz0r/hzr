use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

use anyhow::{Context, Result, bail};
use directories::BaseDirs;
use serde::Serialize;

use crate::adoption::atomic_write;
use crate::cli::ServiceCommand;

const LABEL: &str = "dev.headz0r.hzr.hzrd";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceManager {
    Launchd,
    SystemdUser,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ServiceReport {
    pub manager: ServiceManager,
    pub action: String,
    pub definition: PathBuf,
    pub binary: PathBuf,
    pub active: bool,
    pub changed: bool,
}

pub fn execute(action: ServiceCommand) -> Result<ServiceReport> {
    let base = BaseDirs::new().context("cannot determine the user home directory")?;
    let home = base.home_dir();
    let binary = service_binary()?;
    if cfg!(target_os = "macos") {
        launchd(action, home, &binary)
    } else if cfg!(target_os = "linux") {
        systemd(action, home, &binary)
    } else {
        bail!("production daemon service is supported only on macOS and Linux")
    }
}

/// Ensure the production daemon (and therefore the bundled visualizer) is running.
/// Source/debug binaries return `None`; they must use `hzr daemon serve` and must never
/// write a production user-service definition implicitly.
pub fn ensure_running_if_installed() -> Result<Option<ServiceReport>> {
    let executable = std::env::current_exe().context("cannot resolve the HZR executable")?;
    let physical = executable.canonicalize().unwrap_or(executable);
    if !is_versioned_install(&physical) {
        return Ok(None);
    }
    let status = execute(ServiceCommand::Status)?;
    if status.active {
        return Ok(Some(status));
    }
    let action = if status.definition.is_file() {
        ServiceCommand::Start
    } else {
        ServiceCommand::Install
    };
    execute(action).map(Some)
}

fn service_binary() -> Result<PathBuf> {
    if let Some(binary) = std::env::var_os("HZR_SERVICE_BINARY") {
        return validate_service_binary(PathBuf::from(binary));
    }
    let current = std::env::current_exe().context("cannot resolve the HZR executable")?;
    let physical = current.canonicalize().unwrap_or(current);
    stable_service_binary(&physical).and_then(validate_service_binary)
}

fn validate_service_binary(binary: PathBuf) -> Result<PathBuf> {
    if !binary.is_file() {
        bail!("production daemon binary is missing: {}", binary.display());
    }
    let rendered = binary.to_string_lossy();
    if rendered.contains("/target/debug/") || rendered.contains("/target/release/") {
        bail!(
            "refusing production service binary in a build directory: {}",
            binary.display()
        );
    }
    Ok(binary)
}

fn stable_service_binary(executable: &Path) -> Result<PathBuf> {
    let bin = executable
        .parent()
        .context("HZR executable has no bin directory")?;
    let release = bin
        .parent()
        .context("HZR executable has no release directory")?;
    if release.file_name().and_then(|name| name.to_str()) == Some("current") {
        let install_root = release
            .parent()
            .context("HZR current link has no installation root")?;
        return Ok(install_root.join("current/bin/hzrd"));
    }
    let versions = release
        .parent()
        .context("HZR executable has no versions directory")?;
    if versions.file_name().and_then(|name| name.to_str()) != Some("versions") {
        bail!(
            "{} is not inside a versioned HZR bundle; install a release archive first",
            executable.display()
        );
    }
    let install_root = versions
        .parent()
        .context("HZR versions directory has no installation root")?;
    Ok(install_root.join("current/bin/hzrd"))
}

fn is_versioned_install(executable: &Path) -> bool {
    let Some(release) = executable.parent().and_then(Path::parent) else {
        return false;
    };
    release.file_name().is_some_and(|name| name == "current")
        || release
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|name| name == "versions")
}

fn launchd(action: ServiceCommand, home: &Path, binary: &Path) -> Result<ServiceReport> {
    let definition = home
        .join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist"));
    let rendered = launchd_definition(home, binary);
    let changed = install_definition(action, &definition, rendered.as_bytes())?;
    if action == ServiceCommand::Install {
        std::fs::create_dir_all(home.join("Library/Logs/HZR"))
            .context("failed to create HZR launchd log directory")?;
    }
    let domain = format!("gui/{}", user_id()?);
    let service = format!("{domain}/{LABEL}");
    let manager = manager_command("HZR_LAUNCHCTL", "launchctl");

    match action {
        ServiceCommand::Install => {
            let _ = run_status(&manager, ["bootout", &service]);
            run(&manager, ["bootstrap", &domain, path_str(&definition)?])?;
            run(&manager, ["kickstart", "-k", &service])?;
        }
        ServiceCommand::Start | ServiceCommand::Restart => {
            if !run_status(&manager, ["print", &service]).success() {
                run(&manager, ["bootstrap", &domain, path_str(&definition)?])?;
            }
            let arguments = if action == ServiceCommand::Restart {
                vec!["kickstart", "-k", service.as_str()]
            } else {
                vec!["kickstart", service.as_str()]
            };
            run(&manager, arguments)?;
        }
        ServiceCommand::Stop => {
            if run_status(&manager, ["print", &service]).success() {
                run(&manager, ["bootout", &service])?;
            }
        }
        ServiceCommand::Status => {}
    }
    let active = run_status(&manager, ["print", &service]).success();
    Ok(ServiceReport {
        manager: ServiceManager::Launchd,
        action: action_name(action).to_owned(),
        definition,
        binary: binary.to_path_buf(),
        active,
        changed,
    })
}

fn systemd(action: ServiceCommand, home: &Path, binary: &Path) -> Result<ServiceReport> {
    let definition = home
        .join(".config/systemd/user")
        .join(format!("{LABEL}.service"));
    let rendered = systemd_definition(home, binary);
    let changed = install_definition(action, &definition, rendered.as_bytes())?;
    let manager = manager_command("HZR_SYSTEMCTL", "systemctl");

    match action {
        ServiceCommand::Install => {
            run(&manager, ["--user", "daemon-reload"])?;
            run(&manager, ["--user", "enable", "--now", LABEL])?;
        }
        ServiceCommand::Start => run(&manager, ["--user", "start", LABEL])?,
        ServiceCommand::Stop => run(&manager, ["--user", "stop", LABEL])?,
        ServiceCommand::Restart => run(&manager, ["--user", "restart", LABEL])?,
        ServiceCommand::Status => {}
    }
    let active = run_status(&manager, ["--user", "is-active", "--quiet", LABEL]).success();
    Ok(ServiceReport {
        manager: ServiceManager::SystemdUser,
        action: action_name(action).to_owned(),
        definition,
        binary: binary.to_path_buf(),
        active,
        changed,
    })
}

fn install_definition(action: ServiceCommand, path: &Path, rendered: &[u8]) -> Result<bool> {
    if action != ServiceCommand::Install {
        if action != ServiceCommand::Status && !path.is_file() {
            bail!(
                "service definition is missing: {}; run install first",
                path.display()
            );
        }
        return Ok(false);
    }
    let existing = std::fs::read(path).ok();
    let changed = existing.as_deref() != Some(rendered);
    if changed {
        atomic_write(path, rendered)?;
    }
    Ok(changed)
}

fn manager_command(variable: &str, default: &str) -> String {
    std::env::var(variable).unwrap_or_else(|_| default.to_owned())
}

fn run<I, S>(program: impl AsRef<std::ffi::OsStr>, arguments: I) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    let program = program.as_ref();
    let output = Command::new(program)
        .args(arguments)
        .output()
        .with_context(|| format!("failed to run {}", Path::new(program).display()))?;
    if !output.status.success() {
        bail!(
            "{} exited with {}: {}",
            Path::new(program).display(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn run_status<I, S>(program: impl AsRef<std::ffi::OsStr>, arguments: I) -> ExitStatus
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    Command::new(program)
        .args(arguments)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap_or_else(|_| failure_status())
}

#[cfg(unix)]
fn failure_status() -> ExitStatus {
    use std::os::unix::process::ExitStatusExt;

    ExitStatus::from_raw(1 << 8)
}

fn action_name(action: ServiceCommand) -> &'static str {
    match action {
        ServiceCommand::Install => "install",
        ServiceCommand::Start => "start",
        ServiceCommand::Stop => "stop",
        ServiceCommand::Restart => "restart",
        ServiceCommand::Status => "status",
    }
}

fn path_str(path: &Path) -> Result<&str> {
    path.to_str()
        .with_context(|| format!("path is not UTF-8: {}", path.display()))
}

fn xml(value: &Path) -> String {
    value
        .to_string_lossy()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn systemd_escape(value: &Path) -> String {
    value
        .to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

fn launchd_definition(home: &Path, binary: &Path) -> String {
    let log = home.join("Library/Logs/HZR/hzrd.log");
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n<dict>\n\
         <key>Label</key><string>{LABEL}</string>\n\
         <key>ProgramArguments</key><array><string>{}</string></array>\n\
         <key>EnvironmentVariables</key><dict><key>HOME</key><string>{}</string></dict>\n\
         <key>RunAtLoad</key><true/>\n<key>KeepAlive</key><true/>\n\
         <key>StandardOutPath</key><string>{}</string>\n\
         <key>StandardErrorPath</key><string>{}</string>\n\
         </dict>\n</plist>\n",
        xml(binary),
        xml(home),
        xml(&log),
        xml(&log)
    )
}

fn systemd_definition(home: &Path, binary: &Path) -> String {
    format!(
        "[Unit]\nDescription=HZR zero-redundancy daemon\n\
         After=network.target\n\n[Service]\nType=simple\n\
         ExecStart=\"{}\"\nEnvironment=\"HOME={}\"\n\
         Restart=on-failure\nRestartSec=1\n\n[Install]\nWantedBy=default.target\n",
        systemd_escape(binary),
        systemd_escape(home)
    )
}

fn user_id() -> Result<u32> {
    let output = Command::new("id")
        .arg("-u")
        .output()
        .context("failed to resolve the launchd user id")?;
    if !output.status.success() {
        bail!("`id -u` exited with {}", output.status);
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .context("`id -u` returned a non-numeric user id")
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use tempfile::tempdir;

    use super::{
        is_versioned_install, launchd_definition, stable_service_binary, systemd_definition,
    };

    #[test]
    fn test_service_definitions_use_stable_current_binary() {
        let directory = tempdir().expect("temporary directory");
        let root = directory.path();
        let executable = root.join("versions/v0.3.8-test/bin/hzr");
        fs::create_dir_all(executable.parent().expect("bin parent")).expect("bin directory");
        let stable = stable_service_binary(&executable).expect("stable path");

        assert_eq!(stable, root.join("current/bin/hzrd"));
        assert!(!stable.to_string_lossy().contains("/versions/"));
        assert!(launchd_definition(root, &stable).contains("current/bin/hzrd"));
        assert!(systemd_definition(root, &stable).contains("current/bin/hzrd"));

        let through_current = root.join("current/bin/hzr");
        fs::create_dir_all(through_current.parent().expect("current bin parent"))
            .expect("current bin directory");
        assert_eq!(
            stable_service_binary(&through_current).expect("current path"),
            root.join("current/bin/hzrd")
        );
    }

    #[test]
    fn test_only_versioned_bundle_layout_is_a_production_install() {
        assert!(is_versioned_install(Path::new(
            "/opt/hzr/versions/v0.3.8-test/bin/hzr"
        )));
        assert!(is_versioned_install(Path::new("/opt/hzr/current/bin/hzr")));
        assert!(!is_versioned_install(Path::new(
            "/workspace/target/debug/hzr"
        )));
    }

    #[test]
    fn test_service_definitions_escape_paths() {
        let home = std::path::Path::new("/tmp/a & b");
        let binary = std::path::Path::new("/tmp/a & b/current/bin/hzrd");

        assert!(launchd_definition(home, binary).contains("a &amp; b"));
        assert!(systemd_definition(home, binary).contains("a & b/current/bin/hzrd"));
    }

    #[cfg(unix)]
    #[test]
    fn test_public_binary_symlink_resolves_back_to_stable_service() {
        let directory = tempdir().expect("temporary directory");
        let root = directory.path();
        let release_bin = root.join("versions/v0.3.8-test/bin");
        fs::create_dir_all(&release_bin).expect("release bin");
        fs::write(release_bin.join("hzr"), b"hzr").expect("HZR binary");
        fs::write(release_bin.join("hzrd"), b"hzrd").expect("daemon binary");
        std::os::unix::fs::symlink(root.join("versions/v0.3.8-test"), root.join("current"))
            .expect("current link");
        let public = root.join("bin/hzr");
        fs::create_dir_all(public.parent().expect("public bin")).expect("public directory");
        std::os::unix::fs::symlink(root.join("current/bin/hzr"), &public).expect("public link");
        let physical = public.canonicalize().expect("physical executable");
        let canonical_root = root.canonicalize().expect("canonical install root");

        assert_eq!(
            stable_service_binary(&physical).expect("stable service"),
            canonical_root.join("current/bin/hzrd")
        );
    }
}
