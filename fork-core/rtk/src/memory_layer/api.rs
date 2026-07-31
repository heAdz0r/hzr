// E4.1: Minimal HTTP/1.1 API server — localhost only, no async deps required.
// Endpoints: GET /v1/health, POST /v1/{explore,delta,refresh,context}
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering}; // T2.2: bounded thread pool
use std::sync::{Arc, Mutex}; // Fix 1: OnceLock removed (CG_CACHE gone)
use std::time::{Duration, Instant};

use super::cache::record_event;
use super::indexer::build_git_delta;
use super::renderer::{build_response, render_text};
// Fix 1: budget/git_churn/intent/ranker used only via super::plan_context_graph_first
use super::{indexer, DetailLevel, QueryType, ARTIFACT_VERSION};
use super::{record_cache_event, store_artifact, store_import_edges};

// Keep poll sleep small to avoid adding ~50ms queueing delay to localhost requests.
const ACCEPT_POLL_SLEEP: Duration = Duration::from_millis(5);
const MAX_BODY_SIZE: usize = 1_048_576; // T2.1: 1 MB — OOM protection
const MAX_CONCURRENT_CONNECTIONS: usize = 32; // T2.2: bounded thread pool

// ── Request / response structs ────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ApiRequest {
    project_root: String,
    #[serde(default)]
    query_type: QueryType,
    #[serde(default)]
    detail: DetailLevel,
    /// For /v1/delta: git ref (e.g. "HEAD~5") or None for FS delta
    since: Option<String>,
    /// "json" (default) or "text"
    #[serde(default = "default_format")]
    format: String,
}

fn default_format() -> String {
    "json".to_string()
}

// ── Plan-context request ──────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct PlanRequest {
    project_root: String,
    /// Task description (e.g. "fix jwt token refresh bug")
    #[serde(default)]
    task: String,
    /// Max tokens to include. 0 = use default (12000).
    #[serde(default)]
    token_budget: u32,
    /// Output format: reserved for future use (HTTP API always returns JSON; text format is CLI-only)
    #[serde(default = "default_format")]
    #[allow(dead_code)]
    format: String,
    /// PRD: force legacy pipeline (default: false)
    #[serde(default)]
    legacy: bool, // ADDED: PRD additive field
    /// PRD: include pipeline trace in response (default: false)
    #[serde(default)]
    trace: bool, // ADDED: PRD additive field
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
    version: &'static str,
    artifact_version: u32,
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
}

// ── PID file lifecycle guard ──────────────────────────────────────────────────

struct PidGuard {
    path: PathBuf,
}

impl PidGuard {
    fn write(port: u16) -> Self {
        let path = dirs::data_local_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("rtk")
            .join(format!("mem-server-{port}.pid"));
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, std::process::id().to_string());
        Self { path }
    }
}

impl Drop for PidGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

// ── HTTP parsing helpers ──────────────────────────────────────────────────────

struct Request {
    method: String,
    path: String,
    body: Vec<u8>,
}

fn parse_request(stream: &TcpStream) -> Result<Request> {
    let mut reader = BufReader::new(stream);

    // Read request line
    let mut request_line = String::new();
    reader
        .read_line(&mut request_line)
        .context("Failed to read request line")?;
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 2 {
        bail!("Malformed request line: {}", request_line.trim());
    }
    let method = parts[0].to_uppercase();
    let path = parts[1].to_string();

    // Read headers
    let mut content_length: usize = 0;
    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .context("Failed to read header")?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            break;
        }
        let lower = trimmed.to_lowercase();
        if lower.starts_with("content-length:") {
            content_length = trimmed["content-length:".len()..]
                .trim()
                .parse()
                .unwrap_or(0);
        }
    }

    // Read body
    if content_length > MAX_BODY_SIZE {
        // T2.1: OOM protection
        bail!(
            "Request body too large: {} bytes (max {})",
            content_length,
            MAX_BODY_SIZE
        );
    }
    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        use std::io::Read;
        reader
            .read_exact(&mut body)
            .context("Failed to read request body")?;
    }

    Ok(Request { method, path, body })
}

fn write_response(stream: &TcpStream, status: u16, body: &str) -> Result<()> {
    let status_text = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        500 => "Internal Server Error",
        _ => "Unknown",
    };
    let response = format!(
        "HTTP/1.1 {status} {status_text}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {len}\r\n\
         Connection: close\r\n\
         \r\n\
         {body}",
        len = body.len(),
    );
    let mut w = stream;
    w.write_all(response.as_bytes())
        .context("Failed to write HTTP response")?;
    Ok(())
}

fn json_error(msg: &str) -> String {
    serde_json::to_string(&ErrorResponse {
        error: msg.to_string(),
    })
    .unwrap_or_else(|_| r#"{"error":"serialization failed"}"#.to_string())
}

// ── Endpoint handlers ─────────────────────────────────────────────────────────

fn handle_health() -> Result<String> {
    serde_json::to_string(&HealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
        artifact_version: ARTIFACT_VERSION,
    })
    .context("Failed to serialize health response")
}

fn handle_explore(body: &[u8]) -> Result<String> {
    let req: ApiRequest =
        serde_json::from_slice(body).context("Invalid JSON in explore request")?;
    let project = PathBuf::from(&req.project_root);
    let t = Instant::now();
    let cfg = super::mem_config(); // E6.4: read feature flags

    let state = indexer::build_state(&project, false, cfg.features.cascade_invalidation, 0)?; // E6.4
    if !state.cache_hit {
        store_artifact(&state.artifact)?;
        store_import_edges(&state.artifact);
    }
    let _ = record_cache_event(&state.project_id, "hit");
    let duration_ms = t.elapsed().as_millis() as u64;
    let _ = record_event(&state.project_id, "api:explore", Some(duration_ms));

    let response = build_response(
        "explore",
        &state,
        req.detail,
        false,
        &state.delta,
        req.query_type,
        &cfg.features, // E6.4
    );
    if req.format == "text" {
        return Ok(serde_json::json!({ "text": render_text(&response) }).to_string());
    }
    serde_json::to_string(&response).context("Failed to serialize explore response")
}

fn handle_delta(body: &[u8]) -> Result<String> {
    let req: ApiRequest = serde_json::from_slice(body).context("Invalid JSON in delta request")?;
    let project = PathBuf::from(&req.project_root);
    let t = Instant::now();
    let cfg = super::mem_config(); // E6.4: read feature flags

    let state = indexer::build_state(&project, false, cfg.features.cascade_invalidation, 0)?; // E6.4
    if !state.delta.changes.is_empty() {
        store_artifact(&state.artifact)?;
    }
    let response_delta = if let Some(ref rev) = req.since {
        if !cfg.features.git_delta {
            // E6.4: git_delta feature flag guard in API
            bail!(
                "git delta is disabled via [mem.features] git_delta = false. \
                 Omit 'since' to use FS delta."
            );
        }
        build_git_delta(&state.project_root, rev, 0)?
    } else {
        state.delta.clone()
    };
    let _ = record_cache_event(&state.project_id, "delta");
    let duration_ms = t.elapsed().as_millis() as u64;
    let _ = record_event(&state.project_id, "api:delta", Some(duration_ms));

    let response = build_response(
        "delta",
        &state,
        req.detail,
        false,
        &response_delta,
        req.query_type,
        &cfg.features, // E6.4
    );
    serde_json::to_string(&response).context("Failed to serialize delta response")
}

fn handle_refresh(body: &[u8]) -> Result<String> {
    let req: ApiRequest =
        serde_json::from_slice(body).context("Invalid JSON in refresh request")?;
    let project = PathBuf::from(&req.project_root);
    let t = Instant::now();
    let cfg = super::mem_config(); // E6.4: read feature flags

    let state = indexer::build_state(&project, true, cfg.features.cascade_invalidation, 0)?; // E6.4
    store_artifact(&state.artifact)?;
    store_import_edges(&state.artifact);
    let _ = record_cache_event(&state.project_id, "refreshed");
    let duration_ms = t.elapsed().as_millis() as u64;
    let _ = record_event(&state.project_id, "api:refresh", Some(duration_ms));

    let response = build_response(
        "refresh",
        &state,
        req.detail,
        true,
        &state.delta,
        req.query_type,
        &cfg.features, // E6.4
    );
    serde_json::to_string(&response).context("Failed to serialize refresh response")
}

// ── Plan-context handler ─────────────────────────────────────────────────────

fn handle_plan_context(body: &[u8]) -> Result<String> {
    let req: PlanRequest =
        serde_json::from_slice(body).context("Invalid JSON in plan-context request")?;
    let project = std::path::PathBuf::from(&req.project_root);
    let mut result =
        super::plan_context_graph_first(&project, &req.task, req.token_budget, req.legacy)?;
    // CHANGED: when trace=false (default), strip verbose decision_trace to save tokens
    if !req.trace {
        result.decision_trace.clear();
    }
    // format field is accepted but HTTP API always returns JSON (text format is CLI-only)
    serde_json::to_string(&result).context("Failed to serialize plan-context response")
}

// ── Connection handler ────────────────────────────────────────────────────────

fn handle_connection(stream: TcpStream, verbose: u8) {
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok();

    let req = match parse_request(&stream) {
        Ok(r) => r,
        Err(e) => {
            if verbose > 0 {
                eprintln!("memory.serve: parse error: {e}");
            }
            let _ = write_response(&stream, 400, &json_error(&e.to_string()));
            return;
        }
    };

    if verbose > 0 {
        eprintln!("memory.serve: {} {}", req.method, req.path);
    }

    let (status, body) = match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/v1/health") => match handle_health() {
            Ok(b) => (200, b),
            Err(e) => (500, json_error(&e.to_string())),
        },
        ("POST", "/v1/explore") | ("POST", "/v1/context") => match handle_explore(&req.body) {
            Ok(b) => (200, b),
            Err(e) => (500, json_error(&e.to_string())),
        },
        ("POST", "/v1/delta") => match handle_delta(&req.body) {
            Ok(b) => (200, b),
            Err(e) => (500, json_error(&e.to_string())),
        },
        ("POST", "/v1/refresh") => match handle_refresh(&req.body) {
            Ok(b) => (200, b),
            Err(e) => (500, json_error(&e.to_string())),
        },
        ("POST", "/v1/plan-context") => match handle_plan_context(&req.body) {
            Ok(b) => (200, b),
            Err(e) => (500, json_error(&e.to_string())),
        },
        ("GET", _) | ("POST", _) => (
            404,
            json_error(
                "Not found. Available: GET /v1/health, POST /v1/{explore,delta,refresh,context,plan-context}",
            ),
        ),
        _ => (405, json_error("Method not allowed")),
    };

    let _ = write_response(&stream, status, &body);
}

// ── Public entry point ────────────────────────────────────────────────────────

/// E4.1: Start HTTP API server on localhost:{port}.
/// Exits after `idle_secs` of no requests (daemon lifecycle).
/// Writes PID file for external lifecycle management.
pub(super) fn serve(port: u16, idle_secs: u64, verbose: u8) -> Result<()> {
    let addr = format!("127.0.0.1:{port}");
    let listener = TcpListener::bind(&addr).with_context(|| format!("Failed to bind to {addr}"))?;
    listener
        .set_nonblocking(true)
        .context("Failed to set non-blocking mode")?;

    let _pid = PidGuard::write(port);
    let idle_timeout = Duration::from_secs(if idle_secs == 0 { u64::MAX } else { idle_secs });
    let last_request = Arc::new(Mutex::new(Instant::now()));
    let active_connections = Arc::new(AtomicUsize::new(0)); // T2.2: bounded thread pool

    eprintln!(
        "memory.serve: listening on http://{addr} (idle-timeout={}s)",
        if idle_secs == 0 {
            "∞".to_string()
        } else {
            idle_secs.to_string()
        }
    );

    loop {
        match listener.accept() {
            Ok((stream, peer)) => {
                *last_request.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now(); // P1: handle poisoned mutex
                if verbose > 0 {
                    eprintln!("memory.serve: connection from {peer}");
                }
                let conn_count = active_connections.fetch_add(1, Ordering::Relaxed); // T2.2: check limit
                if conn_count >= MAX_CONCURRENT_CONNECTIONS {
                    active_connections.fetch_sub(1, Ordering::Relaxed);
                    eprintln!("memory.serve: connection limit reached, dropping {peer}");
                    continue;
                }
                let v = verbose;
                let active = Arc::clone(&active_connections);
                std::thread::spawn(move || {
                    handle_connection(stream, v);
                    active.fetch_sub(1, Ordering::Relaxed); // T2.2: decrement on exit
                });
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if last_request
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .elapsed()
                    > idle_timeout
                {
                    // P1: handle poisoned mutex
                    eprintln!("memory.serve: idle timeout ({idle_secs}s), stopping");
                    break;
                }
                std::thread::sleep(ACCEPT_POLL_SLEEP);
            }
            Err(e) => {
                eprintln!("memory.serve: accept error: {e}");
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
    eprintln!("memory.serve: stopped");
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_response_is_valid_json() {
        let body = handle_health().expect("health should succeed");
        let val: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
        assert_eq!(val["status"], "ok");
        assert!(val["version"].is_string());
        assert!(val["artifact_version"].is_number());
    }

    #[test]
    fn invalid_project_returns_error_json() {
        // Explore a non-existent path must return an Err (not panic)
        let body = serde_json::json!({
            "project_root": "/tmp/rtk_api_nonexistent_xyz_12345"
        })
        .to_string();
        // Should produce Err (no such dir)
        let result = handle_explore(body.as_bytes());
        assert!(result.is_err(), "non-existent project must return Err");
    }

    #[test]
    fn server_binds_and_health_check_works() {
        // E4.1: bind a server on a random port, send GET /v1/health, verify response
        use std::io::{Read, Write};
        use std::net::TcpStream;

        // Bind on port 0 = OS assigns a free port
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        drop(listener); // release so serve() can re-bind — test uses direct connection approach

        // Spawn a minimal ad-hoc server for one request
        let listener2 = TcpListener::bind(format!("127.0.0.1:{port}")).expect("rebind");
        listener2.set_nonblocking(false).ok();
        let handle = std::thread::spawn(move || {
            if let Ok((stream, _)) = listener2.accept() {
                handle_connection(stream, 0);
            }
        });

        // Client sends GET /v1/health
        let mut conn = TcpStream::connect(format!("127.0.0.1:{port}")).expect("connect");
        write!(
            conn,
            "GET /v1/health HTTP/1.1\r\nHost: localhost\r\nContent-Length: 0\r\n\r\n"
        )
        .expect("write request");
        conn.flush().ok();

        let mut response = String::new();
        conn.read_to_string(&mut response).expect("read response");
        handle.join().ok();

        assert!(
            response.contains("200 OK"),
            "expected 200 OK; got: {response}"
        );
        assert!(
            response.contains("\"status\":\"ok\""),
            "expected health JSON; got: {response}"
        );
    }
}
