use anyhow::{bail, Context, Result};
use regex::Regex;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use toml_edit::{value, DocumentMut};

#[derive(Debug, Clone)]
pub struct BuildShOptions {
    pub root: PathBuf,
    pub build_debug: bool,
    pub build_release: bool,
    pub install_user: bool,
    pub install_usr_local: bool,
    pub verify: bool,
    pub symlink_usr_local: bool,
    pub use_sudo: bool,
    pub set_version: Option<String>,
    pub sync_global_init: bool,
}

#[derive(Debug, Clone)]
struct BuildPaths {
    root: PathBuf,
    cargo_toml: PathBuf,
    main_rs: PathBuf,
    debug_bin: PathBuf,
    release_bin: PathBuf,
    user_bin: PathBuf,
    usr_local_bin: PathBuf,
    local_bin: PathBuf, // ~/.local/bin/rtk (wins in PATH on some systems)
}

pub fn run_sh(opts: BuildShOptions, verbose: u8) -> Result<()> {
    let root = resolve_root(&opts.root)?;
    let paths = build_paths(root)?;

    if verbose > 0 {
        log(&format!("root={}", paths.root.display()));
    }

    if let Some(version) = opts.set_version.as_deref() {
        validate_version(version)?;
        set_project_version(&paths, version)?;
    }

    if opts.build_debug {
        log("cargo build");
        let mut command = Command::new("cargo");
        command.arg("build").current_dir(&paths.root);
        run_checked(&mut command, "cargo build")?;
    }

    if opts.build_release {
        log("cargo build --release");
        let mut command = Command::new("cargo");
        command
            .arg("build")
            .arg("--release")
            .current_dir(&paths.root);
        run_checked(&mut command, "cargo build --release")?;
        // Auto-cleanup: remove debug artifacts (not needed after release build)
        let debug_dir = paths.root.join("target/debug");
        if debug_dir.exists() {
            match fs::remove_dir_all(&debug_dir) {
                Ok(()) => log("clean -> target/debug"),
                Err(e) => warn(&format!("clean target/debug failed: {}", e)),
            }
        }
    }

    if !paths.release_bin.exists() {
        bail!("Release binary not found: {}", paths.release_bin.display());
    }

    if opts.install_user {
        log(&format!("install -> {}", paths.user_bin.display()));
        install_with_optional_sudo(&paths.release_bin, &paths.user_bin, false, opts.use_sudo)?;
    }

    // Also install to ~/.local/bin/rtk when the directory exists (it may precede ~/.cargo/bin in PATH)
    if paths
        .local_bin
        .parent()
        .map(|p| p.exists())
        .unwrap_or(false)
    {
        log(&format!("install -> {}", paths.local_bin.display()));
        install_with_optional_sudo(&paths.release_bin, &paths.local_bin, false, opts.use_sudo)?;
    }

    if opts.install_usr_local {
        if opts.symlink_usr_local {
            log(&format!(
                "symlink -> {} -> {}",
                paths.usr_local_bin.display(),
                paths.user_bin.display()
            ));
            link_with_optional_sudo(
                &paths.user_bin,
                &paths.usr_local_bin,
                opts.use_sudo,
                "No permissions to update /usr/local/bin path (skipped)",
            )?;
        } else {
            log(&format!("install -> {}", paths.usr_local_bin.display()));
            install_with_optional_sudo(
                &paths.release_bin,
                &paths.usr_local_bin,
                true,
                opts.use_sudo,
            )?;
        }
    }

    if opts.verify {
        log("verification (5 binaries)");
        verify_binary(&paths.debug_bin)?;
        verify_binary(&paths.release_bin)?;
        verify_binary(&paths.user_bin)?;
        verify_binary(&paths.local_bin)?; // ~/.local/bin/rtk
        verify_binary(&paths.usr_local_bin)?;
    }

    if opts.sync_global_init {
        sync_global_init(&paths, opts.install_user)?;
    }

    log("done");
    Ok(())
}

fn sync_global_init(paths: &BuildPaths, prefer_user_bin: bool) -> Result<()> {
    let sync_bin = if prefer_user_bin && paths.user_bin.exists() {
        &paths.user_bin
    } else {
        &paths.release_bin
    };

    log(&format!(
        "sync global Claude hooks/settings via `{}`",
        sync_bin.display()
    ));

    let status = Command::new(sync_bin)
        .arg("init")
        .arg("-g")
        .arg("--auto-patch")
        // Build should be deterministic and fast; skip optional grepai install.
        .env("RTK_SKIP_GREPAI", "1")
        .status()
        .with_context(|| format!("Failed to run `{}` init sync", sync_bin.display()))?;

    if !status.success() {
        warn("Global init sync failed (build succeeded). Run `rtk init -g --auto-patch` manually.");
    }

    Ok(())
}

fn resolve_root(root: &Path) -> Result<PathBuf> {
    let root = fs::canonicalize(root)
        .with_context(|| format!("Failed to resolve root path {}", root.display()))?;
    if !root.join("Cargo.toml").exists() {
        bail!(
            "Cargo.toml not found in root path {}. Pass --root <repo-path>",
            root.display()
        );
    }
    Ok(root)
}

fn build_paths(root: PathBuf) -> Result<BuildPaths> {
    let home = dirs::home_dir().context("Failed to resolve home directory")?;
    Ok(BuildPaths {
        cargo_toml: root.join("Cargo.toml"),
        main_rs: root.join("src/main.rs"),
        debug_bin: root.join("target/debug/rtk"),
        release_bin: root.join("target/release/rtk"),
        user_bin: home.join(".cargo/bin/rtk"),
        usr_local_bin: PathBuf::from("/usr/local/bin/rtk"),
        local_bin: home.join(".local/bin/rtk"), // also install here if dir exists
        root,
    })
}

fn validate_version(v: &str) -> Result<()> {
    let re =
        Regex::new(r"^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z._-]+)?$").expect("version regex is valid");
    if !re.is_match(v) {
        bail!("Invalid version: {}", v);
    }
    Ok(())
}

fn set_project_version(paths: &BuildPaths, version: &str) -> Result<()> {
    log(&format!("set version -> {}", version));
    let cargo_raw = fs::read_to_string(&paths.cargo_toml)
        .with_context(|| format!("Failed to read {}", paths.cargo_toml.display()))?;
    let mut cargo_doc = cargo_raw
        .parse::<DocumentMut>()
        .with_context(|| format!("Failed to parse {}", paths.cargo_toml.display()))?;
    cargo_doc["package"]["version"] = value(version);
    fs::write(&paths.cargo_toml, cargo_doc.to_string())
        .with_context(|| format!("Failed to write {}", paths.cargo_toml.display()))?;

    if paths.main_rs.exists() {
        maybe_replace_version_assignment(&paths.main_rs, version)?;
    }

    Ok(())
}

fn maybe_replace_version_assignment(path: &Path, version: &str) -> Result<()> {
    let src =
        fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?;
    let Some(updated) = replace_first_version_assignment(&src, version) else {
        return Ok(());
    };
    fs::write(path, updated).with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(())
}

fn replace_first_version_assignment(src: &str, version: &str) -> Option<String> {
    let re = Regex::new(r#"version\s*=\s*"[^"]+""#).expect("assignment regex is valid");
    let m = re.find(src)?;
    let mut out = String::with_capacity(src.len() + version.len());
    out.push_str(&src[..m.start()]);
    out.push_str(&format!("version = \"{}\"", version));
    out.push_str(&src[m.end()..]);
    Some(out)
}

fn run_checked(command: &mut Command, label: &str) -> Result<()> {
    let status = command
        .status()
        .with_context(|| format!("Failed to run {}", label))?;
    if !status.success() {
        bail!(
            "{} failed with exit code {}",
            label,
            status.code().unwrap_or(1)
        );
    }
    Ok(())
}

fn install_with_optional_sudo(
    src: &Path,
    dst: &Path,
    allow_skip: bool,
    use_sudo: bool,
) -> Result<()> {
    match install_direct(src, dst) {
        Ok(()) => return Ok(()),
        Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {}
        Err(err) => {
            return Err(err).with_context(|| format!("Failed to install {}", dst.display()));
        }
    }

    if use_sudo && command_exists("sudo") {
        let status = Command::new("sudo")
            .arg("install")
            .arg("-m")
            .arg("755")
            .arg(src)
            .arg(dst)
            .status()
            .with_context(|| format!("Failed to run sudo install for {}", dst.display()))?;
        if status.success() {
            return Ok(());
        }
        if allow_skip {
            warn(&format!(
                "sudo install failed for {} (skipped)",
                dst.display()
            ));
            return Ok(());
        }
        bail!("sudo install failed for {}", dst.display());
    }

    if allow_skip {
        warn(&format!(
            "No permissions to update {} (skipped)",
            dst.display()
        ));
        return Ok(());
    }
    bail!("No permissions to update {}", dst.display());
}

fn install_direct(src: &Path, dst: &Path) -> io::Result<()> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(src, dst)?;
    set_executable(dst)?;
    Ok(())
}

fn link_with_optional_sudo(
    src: &Path,
    dst: &Path,
    use_sudo: bool,
    fallback_warning: &str,
) -> Result<()> {
    match symlink_force(src, dst) {
        Ok(()) => return Ok(()),
        Err(err) if err.kind() == io::ErrorKind::PermissionDenied => {}
        Err(err) => {
            return Err(err).with_context(|| format!("Failed to create symlink {}", dst.display()));
        }
    }

    if use_sudo && command_exists("sudo") {
        let status = Command::new("sudo")
            .arg("ln")
            .arg("-sfn")
            .arg(src)
            .arg(dst)
            .status()
            .with_context(|| format!("Failed to run sudo ln for {}", dst.display()))?;
        if status.success() {
            return Ok(());
        }
    }

    warn(fallback_warning);
    Ok(())
}

#[cfg(unix)]
fn symlink_force(src: &Path, dst: &Path) -> io::Result<()> {
    use std::os::unix::fs::symlink;

    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }

    if let Ok(meta) = fs::symlink_metadata(dst) {
        if meta.file_type().is_dir() && !meta.file_type().is_symlink() {
            fs::remove_dir_all(dst)?;
        } else {
            fs::remove_file(dst)?;
        }
    }

    symlink(src, dst)
}

#[cfg(not(unix))]
fn symlink_force(src: &Path, dst: &Path) -> io::Result<()> {
    install_direct(src, dst)
}

fn set_executable(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms)?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn command_exists(cmd: &str) -> bool {
    Command::new("which")
        .arg(cmd)
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

fn verify_binary(path: &Path) -> Result<()> {
    println!("== {} ==", path.display());
    if !path.exists() {
        println!("missing");
        println!();
        return Ok(());
    }

    let meta = fs::metadata(path).with_context(|| format!("Failed to stat {}", path.display()))?;
    println!("size: {} bytes", meta.len());
    if let Ok(real) = fs::canonicalize(path) {
        println!("{}", real.display());
    }
    if let Some(sha) = sha256_file(path) {
        println!("sha256: {}", sha);
    } else {
        println!("sha256: unavailable");
    }

    match Command::new(path).arg("--version").output() {
        Ok(out) => {
            let version = String::from_utf8_lossy(&out.stdout);
            print!("{}", version);
            if !version.ends_with('\n') {
                println!();
            }
        }
        Err(err) => warn(&format!(
            "Failed to read version from {}: {}",
            path.display(),
            err
        )),
    }

    let ssh_subcommand_present = Command::new(path)
        .arg("ssh")
        .arg("--help")
        .output()
        .map(|out| {
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            text.contains("SSH with smart output filtering")
        })
        .unwrap_or(false);

    println!(
        "ssh-subcommand: {}",
        if ssh_subcommand_present {
            "present"
        } else {
            "NOT present"
        }
    );
    println!();
    Ok(())
}

fn sha256_file(path: &Path) -> Option<String> {
    if let Ok(out) = Command::new("shasum")
        .arg("-a")
        .arg("256")
        .arg(path)
        .output()
    {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout);
            return s.split_whitespace().next().map(|v| v.to_string());
        }
    }
    if let Ok(out) = Command::new("sha256sum").arg(path).output() {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout);
            return s.split_whitespace().next().map(|v| v.to_string());
        }
    }
    None
}

fn log(msg: &str) {
    println!("[rtk-build] {}", msg);
}

fn warn(msg: &str) {
    eprintln!("[rtk-build] WARN: {}", msg);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_semver_with_optional_suffix() {
        assert!(validate_version("0.20.1").is_ok());
        assert!(validate_version("0.20.1-fork.7").is_ok());
        assert!(validate_version("0.20.1-fork_7").is_ok());
        assert!(validate_version("x.y.z").is_err());
    }

    #[test]
    fn replaces_first_version_assignment_only() {
        let src = r#"let a = "x";
version = "0.1.0"
version = "0.2.0"
"#;
        let out = replace_first_version_assignment(src, "9.9.9").expect("replacement expected");
        assert!(out.contains("version = \"9.9.9\""));
        assert!(out.contains("version = \"0.2.0\""));
    }
}
