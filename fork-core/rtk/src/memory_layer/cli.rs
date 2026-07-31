//! M1: CLI handler functions extracted from mod.rs.
//! Contains run_explore, run_delta, run_refresh, run_watch, run_plan,
//! run_status, run_clear, run_serve, run_gain, run_doctor, run_setup, run_devenv.

use anyhow::{Context, Result};
use std::path::Path;
use std::time::{Duration, SystemTime};

use super::helpers::*;
use super::hooks::{is_block_explore_entry, is_mem_hook_entry};
use super::*;

pub fn run_explore(
    project: &Path,
    refresh: bool,
    strict: bool, // P1: strict dirty-blocking per PRD §8
    detail: DetailLevel,
    format: &str,
    query_type: QueryType, // E2.3: relevance-layer filtering
    verbose: u8,
) -> Result<()> {
    let cfg = mem_config(); // E6.4: read feature flags once per invocation
    let effective_strict = strict || cfg.features.strict_by_default; // E6.4: strict_by_default
    let state = build_state(project, refresh, cfg.features.cascade_invalidation, verbose)?; // E6.4: cascade flag

    // P1: Strict dirty-blocking (PRD §8) — refuse to serve stale/dirty data
    if effective_strict && !refresh {
        // E6.4: use effective_strict
        if state.stale_previous {
            anyhow::bail!(
                "memory.explore --strict: artifact is STALE (TTL expired). \
                 Run `rtk memory refresh` or omit --strict to auto-rebuild."
            );
        }
        if state.previous_exists && !state.delta.changes.is_empty() {
            anyhow::bail!(
                "memory.explore --strict: artifact is DIRTY ({} files changed since last index). \
                 Run `rtk memory refresh` or omit --strict to auto-rebuild.",
                state.delta.changes.len()
            );
        }
    }

    let should_store = refresh || !state.cache_hit;
    if should_store {
        store_artifact(&state.artifact)?;
        // E3.2: store import edges for cascade invalidation
        store_import_edges(&state.artifact);
    }

    // Warn on stderr when serving rebuilt data (PRD §8)
    if state.stale_previous && verbose > 0 {
        eprintln!("memory.explore WARNING: stale artifact rebuilt from current FS");
    }
    if state.previous_exists && !state.stale_previous && !state.delta.changes.is_empty() {
        eprintln!(
            "memory.explore NOTICE: {} files changed since last index, rebuilt",
            state.delta.changes.len()
        );
    }

    // E1.4: record cache event for analytics
    let event_label = cache_status_event_label(&state, refresh);
    let _ = record_cache_event(&state.project_id, event_label);
    let _ = record_event(&state.project_id, "explore", None);

    let response = build_response(
        "explore",
        &state,
        detail,
        refresh,
        &state.delta,
        query_type,
        &cfg.features,
    ); // E6.4
    print_response(&response, format)
}

pub fn run_delta(
    project: &Path,
    since: Option<&str>,
    detail: DetailLevel,
    format: &str,
    query_type: QueryType, // E2.3
    verbose: u8,
) -> Result<()> {
    let cfg = mem_config(); // E6.4: read feature flags once
    let state = build_state(project, false, cfg.features.cascade_invalidation, verbose)?; // E6.4
    if !state.delta.changes.is_empty() {
        store_artifact(&state.artifact)?;
    }

    let response_delta = if let Some(rev) = since {
        // E6.4: git_delta feature flag guard
        if !cfg.features.git_delta {
            anyhow::bail!(
                "memory.delta --since: git delta is disabled via [mem.features] git_delta = false. \
                 Enable it in ~/.config/rtk/config.toml or omit --since to use FS delta."
            );
        }
        build_git_delta(&state.project_root, rev, verbose)?
    } else {
        state.delta.clone()
    };

    // E1.4: record cache event
    let _ = record_cache_event(&state.project_id, "delta");

    let response = build_response(
        "delta",
        &state,
        detail,
        false,
        &response_delta,
        query_type,
        &cfg.features,
    ); // E6.4
    print_response(&response, format)
}

pub fn run_refresh(
    project: &Path,
    detail: DetailLevel,
    format: &str,
    query_type: QueryType, // E2.3
    verbose: u8,
) -> Result<()> {
    let cfg = mem_config(); // E6.4: read feature flags once
    let state = build_state(project, true, cfg.features.cascade_invalidation, verbose)?; // E6.4
    store_artifact(&state.artifact)?;
    store_import_edges(&state.artifact); // E3.2: refresh edges
    let _ = record_cache_event(&state.project_id, "refreshed"); // E1.4

    let response = build_response(
        "refresh",
        &state,
        detail,
        true,
        &state.delta,
        query_type,
        &cfg.features,
    ); // E6.4
    print_response(&response, format)
}

pub fn run_watch(
    project: &Path,
    interval_secs: u64, // E3.1: debounce window in seconds (was: poll interval)
    detail: DetailLevel,
    format: &str,
    query_type: QueryType, // E2.3
    verbose: u8,
) -> Result<()> {
    use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher}; // E3.1
    use std::sync::mpsc;
    use std::time::Instant;

    let cfg = mem_config(); // E6.4: read feature flags once for watch lifecycle
    let debounce = Duration::from_secs(interval_secs.max(1)); // E3.1: debounce window
    let project = project
        .canonicalize()
        .unwrap_or_else(|_| project.to_path_buf());

    // Initial snapshot before registering the watcher
    {
        let state = build_state(&project, false, cfg.features.cascade_invalidation, verbose)?; // E6.4
        if !state.delta.changes.is_empty() || state.stale_previous || !state.previous_exists {
            store_artifact(&state.artifact)?;
            let response = build_response(
                "watch",
                &state,
                detail,
                false,
                &state.delta,
                query_type,
                &cfg.features,
            ); // E6.4
            print_response(&response, format)?;
        }
    }

    // E3.1: set up event-driven watcher (kqueue on macOS, inotify on Linux)
    let (tx, rx) = mpsc::channel::<notify::Result<Event>>();
    let mut watcher = RecommendedWatcher::new(
        move |res| {
            let _ = tx.send(res);
        },
        Config::default(),
    )
    .context("Failed to create filesystem watcher")?;
    watcher
        .watch(&project, RecursiveMode::Recursive)
        .context("Failed to watch project directory")?;

    if verbose > 0 {
        eprintln!(
            "memory.watch start project={} debounce={}s backend=notify",
            project.to_string_lossy(),
            debounce.as_secs(),
        );
    }

    loop {
        // Block until first relevant FS event arrives
        let got_relevant = loop {
            match rx.recv().context("Watcher channel closed")? {
                Ok(event) => {
                    // E3.1: filter out events from excluded dirs
                    if event
                        .paths
                        .iter()
                        .any(|p| should_watch_abs_path(&project, p))
                    {
                        break true;
                    }
                    // irrelevant path (target/, node_modules/, etc.) — keep waiting
                }
                Err(e) => {
                    if verbose > 0 {
                        eprintln!("memory.watch error: {e}");
                    }
                    break false; // log error, try again
                }
            }
        };

        if !got_relevant {
            continue;
        }

        // E3.1: coalesce additional events within the debounce window
        let deadline = Instant::now() + debounce;
        while Instant::now() < deadline {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(anyhow::anyhow!("Watcher channel disconnected"));
                }
            }
        }

        // Build updated state and emit if anything changed
        let state = build_state(&project, false, cfg.features.cascade_invalidation, verbose)?; // E6.4
        if !state.delta.changes.is_empty() || state.stale_previous {
            store_artifact(&state.artifact)?;
            let response = build_response(
                "watch",
                &state,
                detail,
                false,
                &state.delta,
                query_type,
                &cfg.features,
            ); // E6.4
            print_response(&response, format)?;
        } else if verbose > 0 {
            eprintln!("memory.watch project={} clean", project.to_string_lossy());
        }
    }
}

pub fn run_status(project: &Path, verbose: u8) -> Result<()> {
    let project_root = canonical_project_root(project)?;
    let artifact = load_artifact(&project_root)?;
    match artifact {
        None => {
            println!(
                "memory.status project={} cache=miss",
                project_root.display()
            );
        }
        Some(a) => {
            let freshness = if is_artifact_stale(&a) {
                ArtifactFreshness::Stale
            } else if artifact_is_dirty(&project_root, &a)? {
                ArtifactFreshness::Dirty
            } else {
                ArtifactFreshness::Fresh
            };
            let age_secs = epoch_secs(SystemTime::now()).saturating_sub(a.updated_at);
            println!(
                "memory.status project={} id={} cache={} files={} bytes={} updated={}s ago",
                project_root.display(),
                a.project_id,
                freshness_label(freshness),
                a.file_count,
                format_bytes(a.total_bytes),
                age_secs
            );
            if verbose > 0 {
                let db = mem_db_path(); // SQLite WAL — show db path instead of json file
                println!("  db: {}", db.display());
                println!("  version: {}", a.version);
                // E1.4: show cache_stats aggregate
                if let Ok(stats) = query_cache_stats(&a.project_id) {
                    if !stats.is_empty() {
                        let pairs: Vec<String> =
                            stats.iter().map(|(e, c)| format!("{}={}", e, c)).collect();
                        println!("  stats: {}", pairs.join(" "));
                    }
                }
            }
        }
    }
    Ok(())
}

fn freshness_label(freshness: ArtifactFreshness) -> &'static str {
    match freshness {
        ArtifactFreshness::Fresh => "fresh",
        ArtifactFreshness::Stale => "stale",
        ArtifactFreshness::Dirty => "dirty",
    }
}

/// E1.4: Derive cache event label from build state for cache_stats recording.
fn cache_status_event_label(state: &BuildState, refresh: bool) -> &'static str {
    if refresh {
        "refreshed"
    } else if state.stale_previous {
        "stale_rebuild"
    } else if state.cache_hit {
        "hit"
    } else if state.previous_exists && !state.delta.changes.is_empty() {
        "dirty_rebuild"
    } else {
        "miss"
    }
}

/// E3.2: Extract import edges from artifact and store in artifact_edges table.
fn store_import_edges(artifact: &ProjectArtifact) {
    let mut edges: Vec<(String, String)> = Vec::new();
    for file in &artifact.files {
        for import in &file.imports {
            if import.starts_with("self:") {
                continue; // skip synthetic anchors
            }
            edges.push((file.rel_path.clone(), import.clone()));
        }
    }
    let _ = store_artifact_edges(&artifact.project_id, &edges);
}

/// E4.1: Start localhost HTTP API server with idle-timeout daemon lifecycle.
pub fn run_serve(port: u16, idle_secs: u64, verbose: u8) -> Result<()> {
    super::api::serve(port, idle_secs, verbose)
}

pub fn run_clear(project: &Path, _verbose: u8) -> Result<()> {
    let project_root = canonical_project_root(project)?;
    // SQLite WAL: delete rows instead of removing json file
    if delete_artifact(&project_root)? {
        println!("memory.clear project={} ok", project_root.display());
    } else {
        println!(
            "memory.clear project={} nothing to clear",
            project_root.display()
        );
    }
    Ok(())
}

/// E6.3: Show token savings — raw source bytes vs compact context bytes.
/// E4.1: Start localhost HTTP API server with idle-timeout daemon lifecycle.

pub fn run_plan(
    project: &Path,
    task: &str,
    token_budget: u32,
    format: &str,
    top: usize,   // ADDED: cap candidate count for --format paths
    legacy: bool, // ADDED: PRD --legacy flag
    trace: bool,  // ADDED: PRD --trace flag
    _verbose: u8,
) -> Result<()> {
    let display_budget = if token_budget == 0 {
        12_000 // CHANGED: was 4000 — match plan_context_inner default
    } else {
        token_budget
    };
    let result = plan_context_graph_first(project, task, token_budget, legacy)?; // CHANGED: pass legacy flag

    if format == "json" {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else if format == "paths" {
        // ADDED: paths format — one file path per line (for two-stage memory pipeline)
        for c in result.selected.iter().take(top) {
            println!("{}", c.rel_path);
        }
    } else {
        // ADDED: --trace emits pipeline stage sections
        if trace {
            println!(
                "## Graph Seeds (pipeline: {})",
                if legacy {
                    "legacy_v0"
                } else {
                    "graph_first_v1"
                }
            );
            for c in result.selected.iter().take(top.min(result.selected.len())) {
                println!("  [{:.2}] {}", c.score, c.rel_path);
            }
            println!("## Semantic Hits");
            // semantic evidence embedded in candidate sources when available
            for c in &result.selected {
                if c.sources.iter().any(|s| s.starts_with("semantic:")) {
                    println!(
                        "  [{:.2}] {} ({})",
                        c.score,
                        c.rel_path,
                        c.sources
                            .iter()
                            .find(|s| s.starts_with("semantic:"))
                            .unwrap()
                    );
                }
            }
            println!("## Final Context Files");
        }
        println!(
            "# Plan Context ({} selected, {}/{} tokens)",
            result.budget_report.candidates_selected,
            result.budget_report.estimated_used,
            display_budget
        );
        for c in &result.selected {
            println!("  [{:.2}] {}", c.score, c.rel_path);
        }
        if !result.dropped.is_empty() {
            println!("# Dropped: {}", result.dropped.len());
        }
    }
    Ok(())
}

pub fn run_gain(project: &Path, verbose: u8) -> Result<()> {
    let project_root = canonical_project_root(project)?;
    let artifact = load_artifact(&project_root)?;

    match artifact {
        None => {
            println!(
                "memory.gain project={} cache=miss (run `rtk memory explore` first)",
                project_root.display()
            );
        }
        Some(a) => {
            let freshness = if is_artifact_stale(&a) {
                ArtifactFreshness::Stale
            } else if artifact_is_dirty(&project_root, &a)? {
                ArtifactFreshness::Dirty
            } else {
                ArtifactFreshness::Fresh
            };
            let compact = compute_gain_stats(&a, DetailLevel::Compact);

            println!(
                "memory.gain project={} cache={} files={}",
                project_root.display(),
                freshness_label(freshness),
                compact.files_indexed,
            );
            println!(
                "  raw_source: {} ({} bytes)",
                format_bytes(compact.raw_bytes),
                compact.raw_bytes,
            );
            println!(
                "  context:    {} ({} bytes)",
                format_bytes(compact.context_bytes),
                compact.context_bytes,
            );
            println!("  savings:    {:.1}%", compact.savings_pct);

            // -v: compare all detail levels
            if verbose > 0 {
                let normal = compute_gain_stats(&a, DetailLevel::Normal);
                let full = compute_gain_stats(&a, DetailLevel::Verbose);
                println!("  --- detail level comparison ---");
                println!(
                    "  compact:  {} ({:.1}% savings)",
                    format_bytes(compact.context_bytes),
                    compact.savings_pct,
                );
                println!(
                    "  normal:   {} ({:.1}% savings)",
                    format_bytes(normal.context_bytes),
                    normal.savings_pct,
                );
                println!(
                    "  verbose:  {} ({:.1}% savings)",
                    format_bytes(full.context_bytes),
                    full.savings_pct,
                );
            }
        }
    }
    Ok(())
}

// ── T1: rtk memory doctor ──────────────────────────────────────────────────

/// Inner diagnostic logic — returns (has_fail, has_warn). // T1
fn doctor_inner(project: &Path) -> Result<(bool, bool)> {
    let mut has_fail = false;
    let mut has_warn = false;

    // 1. Check settings.json hooks
    let settings_path = dirs::home_dir()
        .context("Cannot find home directory")?
        .join(".claude")
        .join("settings.json");

    let pre_hooks: Vec<serde_json::Value> = if settings_path.exists() {
        let raw = fs::read_to_string(&settings_path).unwrap_or_default();
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap_or(serde_json::json!({}));
        v.get("hooks")
            .and_then(|h| h.get("PreToolUse"))
            .and_then(|a| a.as_array())
            .cloned()
            .unwrap_or_default()
    } else {
        vec![]
    };

    let mem_hook_present = pre_hooks.iter().any(is_mem_hook_entry);
    let block_hook_present = pre_hooks.iter().any(is_block_explore_entry);

    if mem_hook_present {
        println!("[ok] hook: rtk-mem-context.sh registered (PreToolUse:Task)");
    } else {
        println!("[FAIL] hook: rtk-mem-context.sh - NOT in settings.json");
        println!("       Fix: rtk memory install-hook");
        has_fail = true;
    }

    if block_hook_present {
        println!("[ok] hook: rtk-block-native-explore.sh registered (PreToolUse:Task)");
    } else {
        println!("[FAIL] hook: rtk-block-native-explore.sh - NOT in settings.json");
        println!("       Fix: rtk memory install-hook");
        has_fail = true;
    }

    // 2. Check cache status
    let project_root = canonical_project_root(project).unwrap_or_else(|_| project.to_path_buf());
    match load_artifact(&project_root) {
        Ok(None) => {
            println!("[WARN] cache: no artifact found");
            println!("       Fix: rtk memory explore .");
            has_warn = true;
        }
        Ok(Some(a)) => {
            let freshness = if is_artifact_stale(&a) {
                ArtifactFreshness::Stale
            } else if artifact_is_dirty(&project_root, &a).unwrap_or(false) {
                ArtifactFreshness::Dirty
            } else {
                ArtifactFreshness::Fresh
            };
            let age_secs = epoch_secs(SystemTime::now()).saturating_sub(a.updated_at);
            match freshness {
                ArtifactFreshness::Fresh => {
                    println!(
                        "[ok] cache: fresh, files={}, updated={}s ago",
                        a.file_count, age_secs
                    );
                }
                ArtifactFreshness::Stale | ArtifactFreshness::Dirty => {
                    println!(
                        "[WARN] cache: {}, files={}, updated={}s ago",
                        freshness_label(freshness),
                        a.file_count,
                        age_secs
                    );
                    println!("       Fix: rtk memory refresh .");
                    has_warn = true;
                }
            }

            // 3. Gain stats (informational)
            let gain = compute_gain_stats(&a, DetailLevel::Compact);
            println!(
                "[ok] memory.gain: raw={} -> context={} ({:.1}% savings)",
                format_bytes(gain.raw_bytes),
                format_bytes(gain.context_bytes),
                gain.savings_pct
            );
        }
        Err(_) => {
            println!("[WARN] cache: failed to load artifact");
            has_warn = true;
        }
    }

    // 4. rtk binary in PATH
    match std::process::Command::new("rtk")
        .arg("--version")
        .env("RTK_ALLOW_NATIVE_READ", "1") // avoid re-entrancy with hooks
        .output()
    {
        Ok(out) => {
            let ver = String::from_utf8_lossy(&out.stdout);
            let ver = ver.trim().trim_start_matches("rtk ");
            println!("[ok] rtk binary: {}", ver);
        }
        Err(_) => {
            println!("[WARN] rtk binary not found in PATH");
            has_warn = true;
        }
    }

    Ok((has_fail, has_warn))
}

/// Diagnose memory layer health: hooks, cache, gain, rtk binary.
/// Exit 0 = all ok, 1 = has [FAIL], 2 = only [WARN].
pub fn run_doctor(project: &Path, _verbose: u8) -> Result<()> {
    let (has_fail, has_warn) = doctor_inner(project)?;
    if has_fail {
        std::process::exit(1);
    } else if has_warn {
        std::process::exit(2);
    }
    Ok(())
}

// ── T2: rtk memory setup ───────────────────────────────────────────────────

/// Idempotent 4-step installer: policy hooks -> memory hook -> cache -> doctor.
pub fn run_setup(project: &Path, auto_patch: bool, _no_watch: bool, verbose: u8) -> Result<()> {
    // [P2] fix: use auto_patch
    use std::io::Write as IoWrite;
    println!("RTK Memory Layer Setup\n");

    // [1/4] policy hooks
    print!("[1/4] installing policy hooks...     ");
    let _ = std::io::stdout().flush();
    let patch_mode = if auto_patch {
        crate::init::PatchMode::Auto
    } else {
        crate::init::PatchMode::Ask
    }; // [P2] fix
    match crate::init::run(true, false, false, false, patch_mode, false, verbose) {
        Ok(_) => println!("ok"),
        Err(e) => println!("warn: {}", e),
    }

    // [2/4] memory context hook
    print!("[2/4] installing memory context...   ");
    let _ = std::io::stdout().flush();
    match run_install_hook(false, false, verbose) {
        Ok(_) => println!("ok (rtk-mem-context.sh registered)"),
        Err(e) => println!("warn: {}", e),
    }

    // [3/4] build memory cache
    print!("[3/4] building memory cache...       ");
    let _ = std::io::stdout().flush();
    let project_root = canonical_project_root(project).unwrap_or_else(|_| project.to_path_buf());
    match run_refresh(
        &project_root,
        DetailLevel::Compact,
        "text",
        QueryType::General,
        verbose,
    ) {
        Ok(_) => {}
        Err(e) => println!("warn: {}", e),
    }

    // [4/4] doctor
    println!("[4/4] running doctor...");
    println!();
    let (has_fail, has_warn) = doctor_inner(&project_root).unwrap_or((true, false)); // [P1] fix: check result

    println!();
    if has_fail || has_warn {
        // [P1] fix: conditional completion message
        println!("Setup completed with warnings. See [FAIL]/[WARN] above.");
    } else {
        println!("Setup complete. Restart Claude Code if hooks were just added.");
    }
    Ok(())
}

// ── T5: rtk memory devenv ─────────────────────────────────────────────────

/// Launch a tmux session with 3 panes: grepai watch, rtk memory watch, health loop.
pub fn run_devenv(project: &Path, interval: u64, session_name: &str, _verbose: u8) -> Result<()> {
    use std::process::Command;

    // [P2] fix: walk up to .git root for accurate project root
    let canonical = canonical_project_root(project).unwrap_or_else(|_| project.to_path_buf());
    let project_root = {
        let mut cur = canonical.clone();
        loop {
            if cur.join(".git").exists() {
                break cur.clone();
            }
            match cur.parent() {
                Some(p) => cur = p.to_path_buf(),
                None => break canonical.clone(),
            }
        }
    };
    let project_str = project_root.to_string_lossy().to_string();

    // Check tmux availability
    if Command::new("tmux").arg("-V").output().is_err() {
        println!("tmux not found. Start these in three separate terminals:\n");
        println!("  grepai watch");
        println!("  rtk memory watch {} --interval {}", project_str, interval);
        println!("  while true; do clear; rtk memory status; rtk memory doctor; rtk gain -p; sleep 10; done"); // [P2] fix: add status
        return Ok(());
    }

    // Check if session already exists -> attach
    let session_exists = Command::new("tmux")
        .args(["has-session", "-t", session_name])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if session_exists {
        println!("Attaching to existing tmux session: {}", session_name);
        let _ = Command::new("tmux")
            .args(["attach-session", "-t", session_name])
            .status();
        return Ok(());
    }

    // Create new session (detached)
    Command::new("tmux")
        .args(["new-session", "-d", "-s", session_name])
        .status()
        .context("Failed to create tmux session")?;

    // Pane 0: grepai watch
    Command::new("tmux")
        .args([
            "send-keys",
            "-t",
            &format!("{}:0.0", session_name),
            "grepai watch",
            "Enter",
        ])
        .status()
        .ok();

    // Pane 1: rtk memory watch
    Command::new("tmux")
        .args(["split-window", "-h", "-t", &format!("{}:0", session_name)])
        .status()
        .ok();
    Command::new("tmux")
        .args([
            "send-keys",
            "-t",
            &format!("{}:0.1", session_name),
            &format!("rtk memory watch {} --interval {}", project_str, interval),
            "Enter",
        ])
        .status()
        .ok();

    // Pane 2: health loop
    Command::new("tmux")
        .args(["split-window", "-v", "-t", &format!("{}:0.1", session_name)])
        .status()
        .ok();
    let health_cmd =
        // [P2] fix: add memory status to health loop
        "while true; do clear; rtk memory status; echo; rtk memory doctor; echo; rtk gain -p; sleep 10; done".to_string();
    Command::new("tmux")
        .args([
            "send-keys",
            "-t",
            &format!("{}:0.2", session_name),
            &health_cmd,
            "Enter",
        ])
        .status()
        .ok();

    // Balance panes
    Command::new("tmux")
        .args([
            "select-layout",
            "-t",
            &format!("{}:0", session_name),
            "even-horizontal",
        ])
        .status()
        .ok();

    // Attach
    println!("Launching tmux session: {}", session_name);
    let _ = Command::new("tmux")
        .args(["attach-session", "-t", session_name])
        .status();

    Ok(())
}
