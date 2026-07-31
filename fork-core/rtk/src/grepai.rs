use anyhow::{Context, Result};
use serde::Deserialize; // added: for GrepaiHit JSON parsing
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock; // CR-03: binary-path cache
use std::time::Duration; // CR-02: Instant не нужен после замены polling на channel

/// Default timeout for grepai operations (seconds).
/// Prevents infinite hangs when grepai backend (qdrant, ollama) is unreachable.
const GREPAI_SEARCH_TIMEOUT_SECS: u64 = 30;
const GREPAI_INIT_TIMEOUT_SECS: u64 = 60;

const INSTALL_SCRIPT_URL: &str =
    "https://raw.githubusercontent.com/yoanbernabeu/grepai/main/install.sh";

/// A single search result from grepai JSON output
#[derive(Debug, Clone, Deserialize)]
pub struct GrepaiHit {
    pub file_path: String,
    pub start_line: usize,
    pub end_line: usize,
    pub score: f64,
    #[serde(default)]
    pub content: Option<String>,
}

/// Parse grepai --json output into structured hits
pub fn parse_grepai_json(raw: &str) -> Result<Vec<GrepaiHit>> {
    serde_json::from_str(raw).context("Failed to parse grepai JSON")
}

/// State of grepai availability for a given project
#[derive(Debug, Clone)]
pub enum GrepaiState {
    /// Binary found + .grepai/config.yaml exists in project
    Ready(PathBuf),
    /// Binary found, but project not initialized (.grepai/config.yaml missing)
    NotInitialized(PathBuf),
    /// Binary not found anywhere
    NotInstalled,
}

/// Search PATH + well-known locations for the grepai binary
/// CR-03: cache binary discovery for the lifetime of the process
/// The grepai binary does not move while rtk is running; `which` is ~10ms on macOS.
static GREPAI_BINARY_CACHE: OnceLock<Option<PathBuf>> = OnceLock::new();

pub fn find_grepai_binary() -> Option<PathBuf> {
    GREPAI_BINARY_CACHE
        .get_or_init(|| {
            find_grepai_binary_with_candidates(
                find_grepai_binary_from_path(),
                dirs::home_dir(),
                PathBuf::from("/usr/local/bin/grepai"),
            )
        })
        .clone() // CR-03: cheap clone of Option<PathBuf>
}

/// Detect grepai state: binary presence + project initialization
pub fn detect_grepai(project_path: &Path) -> GrepaiState {
    detect_grepai_with_binary(project_path, find_grepai_binary())
}

fn find_grepai_binary_from_path() -> Option<PathBuf> {
    let output = Command::new("which").arg("grepai").output().ok()?;
    if !output.status.success() {
        return None;
    }

    let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if path_str.is_empty() {
        return None;
    }

    Some(PathBuf::from(path_str))
}

fn find_grepai_binary_with_candidates(
    path_candidate: Option<PathBuf>,
    home_dir: Option<PathBuf>,
    global_candidate: PathBuf,
) -> Option<PathBuf> {
    if let Some(candidate) = path_candidate.filter(|p| p.exists()) {
        return Some(candidate);
    }

    if let Some(home) = home_dir {
        let local_candidate = home.join(".local").join("bin").join("grepai");
        if local_candidate.exists() {
            return Some(local_candidate);
        }
    }

    if global_candidate.exists() {
        return Some(global_candidate);
    }

    None
}

fn detect_grepai_with_binary(project_path: &Path, binary: Option<PathBuf>) -> GrepaiState {
    match binary {
        Some(binary_path) => {
            let config_file = project_path.join(".grepai").join("config.yaml");
            if config_file.exists() {
                GrepaiState::Ready(binary_path)
            } else {
                GrepaiState::NotInitialized(binary_path)
            }
        }
        None => GrepaiState::NotInstalled,
    }
}

/// Install grepai via the official install script
/// Installs to ~/.local/bin/grepai
pub fn install_grepai(verbose: u8) -> Result<PathBuf> {
    let install_dir = dirs::home_dir()
        .context("Cannot determine home directory")?
        .join(".local")
        .join("bin");

    // Ensure install dir exists
    std::fs::create_dir_all(&install_dir)
        .with_context(|| format!("Failed to create {}", install_dir.display()))?;

    if verbose > 0 {
        eprintln!("Installing grepai to {}...", install_dir.display());
    }

    // Fetch script safely (no shell interpolation) and pass to `sh` via stdin.
    let script = Command::new("curl")
        .args(["-fsSL", INSTALL_SCRIPT_URL])
        .output()
        .context("Failed to download grepai install script (is curl available?)")?;
    if !script.status.success() {
        let stderr = String::from_utf8_lossy(&script.stderr);
        anyhow::bail!(
            "failed to download grepai install script: {}",
            stderr.trim()
        );
    }

    let mut installer = Command::new("sh")
        .env("INSTALL_DIR", &install_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to start grepai installer shell")?;

    let stdin = installer
        .stdin
        .as_mut()
        .context("Failed to open stdin for grepai installer shell")?;
    stdin
        .write_all(&script.stdout)
        .context("Failed to stream install script to shell")?;

    let output = installer
        .wait_with_output()
        .context("Failed while running grepai installer shell")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("grepai install failed: {}", stderr.trim());
    }

    let binary_path = install_dir.join("grepai");
    if !binary_path.exists() {
        anyhow::bail!(
            "grepai install completed but binary not found at {}",
            binary_path.display()
        );
    }

    if verbose > 0 {
        eprintln!("grepai installed: {}", binary_path.display());
    }

    Ok(binary_path)
}

/// Initialize grepai in a project directory
/// Runs: grepai init --provider ollama --backend gob --yes
/// Then: grepai watch --background
pub fn init_project(binary: &Path, project_path: &Path, verbose: u8) -> Result<()> {
    if verbose > 0 {
        eprintln!("Initializing grepai in {}...", project_path.display());
    }

    // grepai init with defaults (with timeout to prevent hangs)
    let mut init_cmd = Command::new(binary); // use full path to avoid hook rewriting
    init_cmd
        .args(["init", "--provider", "ollama", "--backend", "gob", "--yes"])
        .current_dir(project_path);

    let output = match run_with_timeout(init_cmd, GREPAI_INIT_TIMEOUT_SECS)? {
        Some(o) => o,
        None => {
            anyhow::bail!("grepai init timed out after {}s", GREPAI_INIT_TIMEOUT_SECS);
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if verbose > 0 {
            eprintln!("grepai init failed: {}", stderr.trim());
        }
        anyhow::bail!("grepai init failed: {}", stderr.trim());
    }

    // Start background watcher (with timeout)
    let mut watch_cmd = Command::new(binary);
    watch_cmd
        .args(["watch", "--background"])
        .current_dir(project_path);

    match run_with_timeout(watch_cmd, GREPAI_INIT_TIMEOUT_SECS)? {
        Some(watch_output) => {
            if !watch_output.status.success() && verbose > 0 {
                let stderr = String::from_utf8_lossy(&watch_output.stderr);
                eprintln!("grepai watch --background warning: {}", stderr.trim());
            }
        }
        None => {
            if verbose > 0 {
                eprintln!("grepai watch --background timed out, skipping");
            }
        }
    }

    if verbose > 0 {
        eprintln!("grepai initialized in {}", project_path.display());
    }

    Ok(())
}

/// Run a Command with a timeout. Returns None if the process was killed due to timeout.
/// Drains stdout/stderr in separate threads to avoid pipe deadlocks.
///
/// CR-02: заменили busy-wait polling (sleep 100ms) на mpsc::channel + recv_timeout.
/// child.wait() вызывается в отдельном потоке — OS-уровневая блокировка без CPU waste.
/// Для быстрых команд (<100ms) это экономит до 100ms лишней задержки per вызов.
fn run_with_timeout(mut cmd: Command, timeout_secs: u64) -> Result<Option<std::process::Output>> {
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to spawn process")?;

    // Drain stdout/stderr in background threads to prevent pipe buffer deadlock
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();

    let stdout_thread = std::thread::spawn(move || -> Vec<u8> {
        let mut buf = Vec::new();
        if let Some(mut out) = stdout_pipe {
            use std::io::Read;
            let _ = out.read_to_end(&mut buf);
        }
        buf
    });

    let stderr_thread = std::thread::spawn(move || -> Vec<u8> {
        let mut buf = Vec::new();
        if let Some(mut err) = stderr_pipe {
            use std::io::Read;
            let _ = err.read_to_end(&mut buf);
        }
        buf
    });

    // CR-02: child.wait() в отдельном потоке — OS-level блокировка без sleep-polling.
    // PID захватываем до move, чтобы иметь возможность убить процесс при таймауте.
    let child_pid = child.id();
    let (tx, rx) = std::sync::mpsc::channel::<std::io::Result<std::process::ExitStatus>>();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait()); // blocks until process exits — no CPU polling
    });

    let timeout = Duration::from_secs(timeout_secs);
    match rx.recv_timeout(timeout) {
        Ok(Ok(status)) => {
            // Процесс завершился в пределах таймаута
            let stdout = stdout_thread.join().unwrap_or_default();
            let stderr = stderr_thread.join().unwrap_or_default();
            Ok(Some(std::process::Output {
                status,
                stdout,
                stderr,
            }))
        }
        Ok(Err(e)) => Err(e.into()),
        Err(_) => {
            // Таймаут — убиваем процесс по PID (child перемещён в поток, PID уже захвачен)
            // После kill() stdout/stderr pipes закрываются, reader threads разблокируются.
            #[cfg(unix)]
            {
                let _ = std::process::Command::new("kill")
                    .args(["-TERM", &child_pid.to_string()])
                    .status();
            }
            #[cfg(not(unix))]
            {
                let _ = std::process::Command::new("taskkill")
                    .args(["/PID", &child_pid.to_string(), "/F"])
                    .status();
            }
            Ok(None)
        }
    }
}

/// Execute a grepai search and return its raw JSON output
/// Always requests --json for consistent RTK filtering pipeline
/// : optional --path prefix filter (v0.33.0+ backend pushdown)
/// Returns Some(output) on success, None on failure (caller falls back to built-in)
pub fn execute_search(
    binary: &Path,
    project_path: &Path,
    query: &str,
    max: usize,
    path_filter: Option<&str>, // added: v0.33.0 --path backend pushdown
) -> Result<Option<String>> {
    let mut cmd = Command::new(binary); // use full path to avoid hook rewriting
    cmd.arg("search");

    // CHANGED: always request JSON for RTK filtering pipeline
    cmd.arg("--json");

    // Max results
    cmd.args(["-n", &max.to_string()]);

    // added: pass --path filter to grepai for subdirectory scoping (v0.33.0)
    if let Some(path) = path_filter {
        if path != "." && !path.is_empty() {
            cmd.args(["--path", path]);
        }
    }

    // Query (must be last)
    cmd.arg(query);

    cmd.current_dir(project_path); // added: set cwd before spawn

    let output = match run_with_timeout(cmd, GREPAI_SEARCH_TIMEOUT_SECS)? {
        Some(o) => o,
        None => {
            // Timeout — grepai hung (unreachable backend, missing model, etc.)
            eprintln!(
                "rgai: grepai search timed out after {}s, falling back to built-in",
                GREPAI_SEARCH_TIMEOUT_SECS
            );
            return Ok(None);
        }
    };

    if !output.status.success() {
        return Ok(None);
    }

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    if stdout.trim().is_empty() {
        return Ok(None);
    }

    Ok(Some(stdout))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant; // локальный импорт для теста таймаута (Instant убран из prod-кода CR-02)
    use tempfile::TempDir;

    fn touch(path: &Path) {
        std::fs::write(path, "#!/bin/sh\necho ok\n").unwrap();
    }

    #[test]
    fn detect_grepai_ready_when_config_exists() {
        let dir = TempDir::new().unwrap();
        let grepai_dir = dir.path().join(".grepai");
        std::fs::create_dir_all(&grepai_dir).unwrap();
        std::fs::write(grepai_dir.join("config.yaml"), "provider: ollama").unwrap();

        let fake_binary = dir.path().join("grepai");
        touch(&fake_binary);

        let state = detect_grepai_with_binary(dir.path(), Some(fake_binary.clone()));
        match state {
            GrepaiState::Ready(path) => assert_eq!(path, fake_binary),
            other => panic!("expected Ready, got {other:?}"),
        }
    }

    #[test]
    fn detect_grepai_not_initialized_when_config_missing() {
        let dir = TempDir::new().unwrap();
        let fake_binary = dir.path().join("grepai");
        touch(&fake_binary);

        let state = detect_grepai_with_binary(dir.path(), Some(fake_binary.clone()));
        match state {
            GrepaiState::NotInitialized(path) => assert_eq!(path, fake_binary),
            other => panic!("expected NotInitialized, got {other:?}"),
        }
    }

    #[test]
    fn detect_grepai_not_installed_returns_not_installed() {
        let dir = TempDir::new().unwrap();
        assert!(matches!(
            detect_grepai_with_binary(dir.path(), None),
            GrepaiState::NotInstalled
        ));
    }

    #[test]
    fn find_grepai_binary_prefers_path_candidate() {
        let dir = TempDir::new().unwrap();
        let path_candidate = dir.path().join("path-grepai");
        let home = dir.path().join("home");
        let global = dir.path().join("global-grepai");
        std::fs::create_dir_all(home.join(".local/bin")).unwrap();
        touch(&path_candidate);
        touch(&home.join(".local/bin/grepai"));
        touch(&global);

        let found =
            find_grepai_binary_with_candidates(Some(path_candidate.clone()), Some(home), global);
        assert_eq!(found, Some(path_candidate));
    }

    #[test]
    fn find_grepai_binary_falls_back_to_home_then_global() {
        let dir = TempDir::new().unwrap();
        let home = dir.path().join("home");
        let global = dir.path().join("global-grepai");
        let local = home.join(".local/bin/grepai");
        std::fs::create_dir_all(home.join(".local/bin")).unwrap();
        touch(&local);
        touch(&global);

        let from_home = find_grepai_binary_with_candidates(
            Some(dir.path().join("missing-path-grepai")),
            Some(home.clone()),
            global.clone(),
        );
        assert_eq!(from_home, Some(local));

        std::fs::remove_file(home.join(".local/bin/grepai")).unwrap();
        let from_global =
            find_grepai_binary_with_candidates(None, Some(home.clone()), global.clone());
        assert_eq!(from_global, Some(global));
    }

    // --- GrepaiHit JSON parsing tests ---

    #[test]
    fn parse_grepai_json_full_hits() {
        let json = r#"[
            {
                "file_path": "src/auth.rs",
                "start_line": 10,
                "end_line": 15,
                "score": 0.87,
                "content": "pub fn refresh_token() {\n    // logic\n}"
            },
            {
                "file_path": "src/session.rs",
                "start_line": 1,
                "end_line": 5,
                "score": 0.65,
                "content": "struct Session {}"
            }
        ]"#;
        let hits = parse_grepai_json(json).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].file_path, "src/auth.rs");
        assert_eq!(hits[0].start_line, 10);
        assert!((hits[0].score - 0.87).abs() < 0.001);
        assert!(hits[0].content.is_some());
    }

    #[test]
    fn parse_grepai_json_without_content() {
        let json = r#"[
            {"file_path": "src/main.rs", "start_line": 1, "end_line": 3, "score": 0.5}
        ]"#;
        let hits = parse_grepai_json(json).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].content.is_none());
    }

    #[test]
    fn parse_grepai_json_empty_array() {
        let hits = parse_grepai_json("[]").unwrap();
        assert!(hits.is_empty());
    }

    #[test]
    fn parse_grepai_json_invalid_returns_error() {
        assert!(parse_grepai_json("not json").is_err());
    }

    #[test]
    fn find_grepai_binary_returns_none_when_no_candidates_exist() {
        let dir = TempDir::new().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(home.join(".local/bin")).unwrap();
        let missing_global = dir.path().join("missing-global-grepai");

        let found = find_grepai_binary_with_candidates(
            Some(dir.path().join("missing-path-grepai")),
            Some(home),
            missing_global,
        );
        assert_eq!(found, None);
    }

    #[test]
    fn run_with_timeout_returns_output_for_fast_command() {
        let mut cmd = Command::new("echo");
        cmd.arg("hello");
        let result = run_with_timeout(cmd, 5).unwrap();
        assert!(result.is_some());
        let output = result.unwrap();
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("hello"));
    }

    #[test]
    fn run_with_timeout_kills_slow_command() {
        let mut cmd = Command::new("sleep");
        cmd.arg("60"); // sleeps 60s, but timeout is 1s
        let start = Instant::now();
        let result = run_with_timeout(cmd, 1).unwrap();
        let elapsed = start.elapsed();
        assert!(result.is_none(), "should return None on timeout");
        assert!(elapsed < Duration::from_secs(5), "should not wait full 60s");
    }

    #[test]
    fn run_with_timeout_captures_stderr() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "echo err >&2; echo ok"]);
        let result = run_with_timeout(cmd, 5).unwrap();
        assert!(result.is_some());
        let output = result.unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stdout.contains("ok"));
        assert!(stderr.contains("err"));
    }
}
