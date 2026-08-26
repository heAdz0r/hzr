use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Context;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use hzr_memory::{
    IcmClient, IcmConfig, IcmLayout, IcmSupervisor, IcmTransport, Importance, MemoryTransport,
    RecallRequest, ServiceStatus, StartOutcome, StopOutcome, StoreRequest, verify_installation,
};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

#[derive(Clone)]
struct FakeState {
    token: Arc<str>,
}

struct FakeServer {
    task: JoinHandle<()>,
}

impl Drop for FakeServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn bind_loopback() -> anyhow::Result<(TcpListener, SocketAddr)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    Ok((listener, address))
}

fn spawn_fake_server(listener: TcpListener, token: String) -> FakeServer {
    let state = FakeState {
        token: Arc::from(token),
    };
    let router = Router::new()
        .route("/health", get(fake_health))
        .route("/stats", get(fake_stats))
        .route("/recall", post(fake_recall))
        .route("/store", post(fake_store))
        .with_state(state);
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    FakeServer { task }
}

async fn fake_health() -> Json<Value> {
    Json(json!({"status":"ok","has_embedder":true}))
}

async fn fake_stats(State(state): State<FakeState>, headers: HeaderMap) -> Response {
    if !authorized(&headers, &state.token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(json!({"total_memories":1,"total_topics":1,"avg_weight":1.0})).into_response()
}

async fn fake_recall(State(state): State<FakeState>, headers: HeaderMap) -> Response {
    if !authorized(&headers, &state.token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(json!([memory_json("01HZRRECALL", "remembered exactly")])).into_response()
}

async fn fake_store(
    State(state): State<FakeState>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if !authorized(&headers, &state.token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let summary = body
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default();
    Json(json!([memory_json("01HZRSTORE", summary)])).into_response()
}

fn authorized(headers: &HeaderMap, expected: &str) -> bool {
    headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == format!("Bearer {expected}"))
}

fn memory_json(id: &str, summary: &str) -> Value {
    json!({
        "id": id,
        "created_at": "2026-07-31T00:00:00Z",
        "updated_at": "2026-07-31T00:00:00Z",
        "last_accessed": "2026-07-31T00:00:00Z",
        "access_count": 0,
        "weight": 1.0,
        "topic": "hzr-test",
        "summary": summary,
        "raw_excerpt": null,
        "keywords": ["hzr"],
        "importance": "medium",
        "source": {"type":"manual"},
        "related_ids": [],
        "scope": "user"
    })
}

fn config(temp: &TempDir, executable: &str, address: SocketAddr) -> IcmConfig {
    let mut config = IcmConfig::from_data_root(executable, temp.path());
    config.bind_addr = address;
    config.request_timeout = Duration::from_millis(250);
    config.startup_timeout = Duration::from_secs(2);
    // Process creation can be delayed when Cargo runs this integration binary beside the
    // workspace suite. Keep the fixture below production's 30-second budget without making
    // an immediate fake `--version` response depend on a two-second scheduler window.
    config.cli_timeout = Duration::from_secs(10);
    config.shutdown_timeout = Duration::from_secs(1);
    config.circuit_failure_threshold = 1;
    config.circuit_reset_timeout = Duration::from_secs(1);
    config.transport = IcmTransport::Http;
    config
}

fn read_token(data_root: &std::path::Path) -> anyhow::Result<String> {
    let layout = IcmLayout::prepare(data_root)?;
    Ok(std::fs::read_to_string(layout.token_file)?
        .trim()
        .to_owned())
}

#[tokio::test]
async fn test_http_client_uses_authenticated_json_contracts() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let (listener, address) = bind_loopback().await?;
    let mut config = config(&temp, "icm", address);
    config.cli_fallback = false;
    let client = IcmClient::from_config(config)?;
    assert_eq!(
        client.database_path(),
        std::fs::canonicalize(temp.path())?
            .join("memory")
            .join("icm")
            .join("memories.db")
    );
    let server = spawn_fake_server(listener, read_token(temp.path())?);

    let health = client.readiness().await?;
    assert_eq!(health.status, "ok");
    assert!(health.has_embedder);

    let records = client.recall(&RecallRequest::new("exact query")).await?;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].summary, "remembered exactly");

    let receipt = client
        .store(&StoreRequest::new("hzr-test", "content stays exact"))
        .await?;
    assert_eq!(receipt.transport, MemoryTransport::Http);
    assert_eq!(
        receipt
            .memory
            .as_ref()
            .map(|memory| memory.summary.as_str()),
        Some("content stays exact")
    );
    drop(server);
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn test_http_transport_does_not_silently_invoke_cli_when_fallback_is_disabled()
-> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let (executable, invocation_marker) = fake_cli_marker_script(&temp)?;
    let (listener, address) = bind_loopback().await?;
    drop(listener);
    let mut config = config(
        &temp,
        executable
            .to_str()
            .context("non-UTF-8 fake executable path")?,
        address,
    );
    config.cli_fallback = false;
    let client = IcmClient::from_config(config)?;

    assert!(
        client
            .recall(&RecallRequest::new("must stay HTTP"))
            .await
            .is_err()
    );
    assert!(
        !invocation_marker.exists(),
        "disabled fallback invoked the ICM CLI"
    );
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn test_cli_fallback_parses_json_recall_but_not_human_store_output() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let executable = fake_icm_script(&temp, "0.10.61")?;
    let (listener, address) = bind_loopback().await?;
    drop(listener);

    let recall_client = IcmClient::from_config(config(
        &temp,
        executable
            .to_str()
            .context("non-UTF-8 fake executable path")?,
        address,
    ))?;
    let records = recall_client
        .recall(&RecallRequest::new("fallback"))
        .await?;
    assert_eq!(records[0].id, "01HZRCLI");

    let store_temp = TempDir::new()?;
    let store_executable = fake_icm_script(&store_temp, "0.10.61")?;
    let (listener, store_address) = bind_loopback().await?;
    drop(listener);
    let store_client = IcmClient::from_config(config(
        &store_temp,
        store_executable
            .to_str()
            .context("non-UTF-8 fake executable path")?,
        store_address,
    ))?;
    let receipt = store_client
        .store(&StoreRequest::new("topic", "stored through CLI"))
        .await?;
    assert_eq!(receipt.transport, MemoryTransport::Cli);
    assert!(receipt.memory.is_none());
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn test_cli_memory_maintenance_uses_typed_list_before_mutation() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let executable = fake_icm_script(&temp, "0.10.61")?;
    let (listener, address) = bind_loopback().await?;
    drop(listener);
    let client = IcmClient::from_config(config(
        &temp,
        executable.to_str().context("fake executable path")?,
        address,
    ))?;

    let records = client.list_all().await?;
    assert_eq!(records[0].id, "01HZRCLI");
    client
        .update(
            "01HZRCLI",
            "replacement",
            Some(Importance::High),
            Some(&["decision".into()]),
        )
        .await?;
    client.forget("01HZRCLI").await?;
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn test_supervisor_owns_one_process_and_second_instance_attaches() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let executable = fake_icm_script(&temp, "0.10.61")?;
    let (listener, address) = bind_loopback().await?;
    let config = config(
        &temp,
        executable
            .to_str()
            .context("non-UTF-8 fake executable path")?,
        address,
    );
    let supervisor = IcmSupervisor::new(config.clone())?;
    let server = spawn_fake_server(listener, read_token(temp.path())?);

    let first = supervisor.start().await?;
    assert!(matches!(first, StartOutcome::Started { .. }));
    let again = supervisor.start().await?;
    assert!(matches!(again, StartOutcome::AlreadyRunning { .. }));

    let second = IcmSupervisor::new(config)?;
    let attached = second.start().await?;
    assert!(matches!(attached, StartOutcome::Attached { .. }));
    assert_eq!(second.stop().await?, StopOutcome::Detached);
    assert_eq!(supervisor.stop().await?, StopOutcome::Stopped);
    drop(server);
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn test_concurrent_supervisor_starts_create_exactly_one_owner() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let executable = fake_icm_script(&temp, "0.10.61")?;
    let (listener, address) = bind_loopback().await?;
    let supervisor = Arc::new(IcmSupervisor::new(config(
        &temp,
        executable
            .to_str()
            .context("non-UTF-8 fake executable path")?,
        address,
    ))?);
    let server = spawn_fake_server(listener, read_token(temp.path())?);
    let mut tasks = Vec::new();
    for _ in 0..16 {
        let supervisor = Arc::clone(&supervisor);
        tasks.push(tokio::spawn(async move { supervisor.start().await }));
    }
    let mut started = 0;
    for task in tasks {
        if matches!(task.await??, StartOutcome::Started { .. }) {
            started += 1;
        }
    }

    assert_eq!(started, 1);
    assert_eq!(supervisor.stop().await?, StopOutcome::Stopped);
    drop(server);
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn test_supervisor_restarts_owned_process_after_post_ready_exit() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let (executable, starts) = fake_short_lived_icm_script(&temp)?;
    let (listener, address) = bind_loopback().await?;
    let supervisor = IcmSupervisor::new(config(
        &temp,
        executable
            .to_str()
            .context("non-UTF-8 fake executable path")?,
        address,
    ))?;
    let server = spawn_fake_server(listener, read_token(temp.path())?);

    assert!(matches!(
        supervisor.start().await?,
        StartOutcome::Started { .. }
    ));
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if matches!(
                supervisor.status().await,
                ServiceStatus::Exited { code: Some(0) }
            ) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .context("short-lived ICM did not exit before the deadline")?;
    assert!(matches!(
        supervisor.start().await?,
        StartOutcome::Started { .. }
    ));
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if std::fs::read_to_string(&starts).is_ok_and(|count| count.trim() == "2") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .context("replacement ICM process did not start")?;
    assert_eq!(std::fs::read_to_string(starts)?.trim(), "2");

    assert_eq!(supervisor.stop().await?, StopOutcome::Stopped);
    drop(server);
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn test_cancelled_start_is_fenced_by_final_stop() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let (executable, version_probe, release, starts) = fake_delayed_icm_script(&temp)?;
    let (listener, address) = bind_loopback().await?;
    let supervisor = Arc::new(IcmSupervisor::new(config(
        &temp,
        executable.to_str().context("fake executable path")?,
        address,
    ))?);
    let server = spawn_fake_server(listener, read_token(temp.path())?);
    let cancelled = Arc::new(AtomicBool::new(false));
    let starting = {
        let supervisor = Arc::clone(&supervisor);
        let cancelled = Arc::clone(&cancelled);
        tokio::spawn(async move { supervisor.start_unless_cancelled(&cancelled).await })
    };
    wait_for_path(&version_probe).await?;
    cancelled.store(true, Ordering::Release);
    let stopping = {
        let supervisor = Arc::clone(&supervisor);
        tokio::spawn(async move { supervisor.stop().await })
    };
    std::fs::write(release, b"continue")?;

    assert!(matches!(
        starting.await??,
        Some(StartOutcome::Started { .. })
    ));
    assert_eq!(stopping.await??, StopOutcome::Stopped);
    assert!(matches!(supervisor.status().await, ServiceStatus::Stopped));
    assert!(
        supervisor
            .start_unless_cancelled(&cancelled)
            .await?
            .is_none()
    );
    let starts = std::fs::read_to_string(starts)
        .map(|content| content.lines().count())
        .unwrap_or_default();
    assert!(
        starts <= 1,
        "cancelled supervision spawned {starts} processes"
    );
    drop(server);
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn test_supervisor_recovers_orphan_without_spawning_duplicate() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let executable = fake_icm_script(&temp, "0.10.61")?;
    let (listener, address) = bind_loopback().await?;
    let supervisor = IcmSupervisor::new(config(
        &temp,
        executable
            .to_str()
            .context("non-UTF-8 fake executable path")?,
        address,
    ))?;
    std::fs::write(&supervisor.layout().pid_file, "4242")?;
    let server = spawn_fake_server(listener, read_token(temp.path())?);

    let outcome = supervisor.start().await?;
    assert!(matches!(outcome, StartOutcome::Attached { .. }));
    assert!(!supervisor.layout().log_file.exists());
    assert_eq!(supervisor.stop().await?, StopOutcome::Detached);
    drop(server);
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn test_stdio_mcp_store_uses_typed_rpc_without_parsing_tool_text() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let (executable, capture) = fake_mcp_script(&temp)?;
    let (listener, address) = bind_loopback().await?;
    drop(listener);
    let mut config = config(
        &temp,
        executable
            .to_str()
            .context("non-UTF-8 fake executable path")?,
        address,
    );
    config.transport = IcmTransport::StdioMcp;
    let supervisor = IcmSupervisor::new(config)?;
    let client = supervisor.client();

    assert!(matches!(
        supervisor.start().await?,
        StartOutcome::Started { .. }
    ));
    let mut store = StoreRequest::new("decisions-hzr", "preserve typed MCP semantics");
    store.raw = Some("exact raw excerpt".into());
    let receipt = client.store(&store).await?;
    assert_eq!(receipt.transport, MemoryTransport::StdioMcp);
    assert!(receipt.memory.is_none());
    assert_eq!(
        serde_json::to_value(&receipt)?,
        json!({"transport":"stdio_mcp","memory":null})
    );
    assert_eq!(client.health().await?.status, "ok");

    let records = client.recall(&RecallRequest::new("typed recall")).await?;
    assert_eq!(records[0].id, "01HZRMCPCLI");
    let requests = std::fs::read_to_string(capture)?;
    assert!(requests.contains("icm_memory_store"));
    assert!(requests.contains("exact raw excerpt"));
    assert_eq!(supervisor.stop().await?, StopOutcome::Stopped);
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn test_supervisor_rejects_wrong_icm_version() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let executable = fake_icm_script(&temp, "0.10.60")?;
    let (_listener, address) = bind_loopback().await?;
    let supervisor = IcmSupervisor::new(config(
        &temp,
        executable
            .to_str()
            .context("non-UTF-8 fake executable path")?,
        address,
    ))?;

    let Err(error) = supervisor.start().await else {
        anyhow::bail!("supervisor accepted an incompatible ICM version");
    };
    assert!(error.to_string().contains("expected 0.10.61"));
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn test_installation_verifier_rejects_wrong_executable_checksum() -> anyhow::Result<()> {
    let temp = TempDir::new()?;
    let executable = fake_icm_script(&temp, "0.10.61")?;
    let (_listener, address) = bind_loopback().await?;
    let mut config = config(
        &temp,
        executable
            .to_str()
            .context("non-UTF-8 fake executable path")?,
        address,
    );
    config.expected_executable_sha256 = Some("0".repeat(64));

    let Err(error) = verify_installation(&config).await else {
        anyhow::bail!("installation verifier accepted the wrong executable checksum");
    };
    assert!(error.to_string().contains("checksum mismatch"));
    Ok(())
}

#[cfg(unix)]
fn fake_icm_script(temp: &TempDir, version: &str) -> anyhow::Result<std::path::PathBuf> {
    use std::os::unix::fs::PermissionsExt;

    let path = temp.path().join("fake-icm");
    let recall = memory_json("01HZRCLI", "CLI fallback").to_string();
    let script = format!(
        "#!/bin/sh\n\
         if [ \"$1\" = \"--version\" ]; then printf 'icm {version}\\n'; exit 0; fi\n\
         case \" $* \" in\n\
           *\" recall \"*) printf '%s\\n' '[{recall}]' ;;\n\
           *\" list \"*) printf '%s\\n' '[{recall}]' ;;\n\
           *\" store \"*) printf 'Stored: HUMAN-ONLY-ID\\n' ;;\n\
           *\" update \"*) printf 'Updated\\n' ;;\n\
           *\" forget \"*) printf 'Forgotten\\n' ;;\n\
           *\" serve \"*) sleep 60 ;;\n\
           *) printf 'unexpected arguments\\n' >&2; exit 2 ;;\n\
         esac\n"
    );
    std::fs::write(&path, script)?;
    let mut permissions = std::fs::metadata(&path)?.permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&path, permissions)?;
    Ok(path)
}

#[cfg(unix)]
fn fake_cli_marker_script(
    temp: &TempDir,
) -> anyhow::Result<(std::path::PathBuf, std::path::PathBuf)> {
    use std::os::unix::fs::PermissionsExt;

    let path = temp.path().join("fake-cli-marker");
    let marker = temp.path().join("cli-invoked");
    let script = format!("#!/bin/sh\ntouch '{}'\nexit 0\n", marker.display());
    std::fs::write(&path, script)?;
    let mut permissions = std::fs::metadata(&path)?.permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&path, permissions)?;
    Ok((path, marker))
}

#[cfg(unix)]
fn fake_short_lived_icm_script(
    temp: &TempDir,
) -> anyhow::Result<(std::path::PathBuf, std::path::PathBuf)> {
    use std::os::unix::fs::PermissionsExt;

    let path = temp.path().join("fake-short-lived-icm");
    let starts = temp.path().join("starts");
    let script = format!(
        "#!/bin/sh\n\
         if [ \"$1\" = \"--version\" ]; then printf 'icm 0.10.61\\n'; exit 0; fi\n\
         case \" $* \" in\n\
           *\" serve \"*)\n\
             count=0\n\
             if [ -f '{starts}' ]; then count=$(sed -n '1p' '{starts}'); fi\n\
             count=$((count + 1))\n\
             printf '%s\\n' \"$count\" > '{starts}'\n\
             sleep 0.15 ;;\n\
           *) printf 'unexpected arguments\\n' >&2; exit 2 ;;\n\
         esac\n",
        starts = starts.display()
    );
    std::fs::write(&path, script)?;
    let mut permissions = std::fs::metadata(&path)?.permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&path, permissions)?;
    Ok((path, starts))
}

#[cfg(unix)]
fn fake_delayed_icm_script(
    temp: &TempDir,
) -> anyhow::Result<(
    std::path::PathBuf,
    std::path::PathBuf,
    std::path::PathBuf,
    std::path::PathBuf,
)> {
    use std::os::unix::fs::PermissionsExt;

    let path = temp.path().join("fake-delayed-icm");
    let version_probe = temp.path().join("version-probe");
    let release = temp.path().join("release-version");
    let starts = temp.path().join("starts-delayed");
    let script = format!(
        "#!/bin/sh\n\
         if [ \"$1\" = \"--version\" ]; then\n\
           touch '{version_probe}'\n\
           while [ ! -f '{release}' ]; do sleep 0.01; done\n\
           printf 'icm 0.10.61\\n'; exit 0\n\
         fi\n\
         case \" $* \" in\n\
           *\" serve \"*) printf 'x\\n' >> '{starts}'; sleep 60 ;;\n\
           *) printf 'unexpected arguments\\n' >&2; exit 2 ;;\n\
         esac\n",
        version_probe = version_probe.display(),
        release = release.display(),
        starts = starts.display()
    );
    std::fs::write(&path, script)?;
    let mut permissions = std::fs::metadata(&path)?.permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&path, permissions)?;
    Ok((path, version_probe, release, starts))
}

#[cfg(unix)]
async fn wait_for_path(path: &std::path::Path) -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !path.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .context("timed out waiting for fake ICM gate")?;
    Ok(())
}

#[cfg(unix)]
fn fake_mcp_script(temp: &TempDir) -> anyhow::Result<(std::path::PathBuf, std::path::PathBuf)> {
    use std::os::unix::fs::PermissionsExt;

    let path = temp.path().join("fake-icm-mcp");
    let capture = temp.path().join("mcp-requests.jsonl");
    let recall = memory_json("01HZRMCPCLI", "MCP correctness-first recall").to_string();
    let script = format!(
        "#!/bin/sh\n\
         if [ \"$1\" = \"--version\" ]; then printf 'icm 0.10.61\\n'; exit 0; fi\n\
         case \" $* \" in\n\
           *\" recall \"*) printf '%s\\n' '[{recall}]'; exit 0 ;;\n\
         esac\n\
         next=1\n\
         while IFS= read -r line; do\n\
           printf '%s\\n' \"$line\" >> '{capture}'\n\
           case \"$line\" in\n\
             *'\"method\":\"initialize\"'*)\n\
               printf '{{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":{{\"protocolVersion\":\"2024-11-05\",\"serverInfo\":{{\"name\":\"icm\",\"version\":\"0.10.34\"}}}}}}\\n' \"$next\"\n\
               next=$((next + 1)) ;;\n\
             *'\"method\":\"tools/call\"'*)\n\
               printf '{{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":{{\"content\":[{{\"type\":\"text\",\"text\":\"Updated existing memory (similarity 1.00): HUMAN-TEXT-ID\"}}]}}}}\\n' \"$next\"\n\
               next=$((next + 1)) ;;\n\
             *'\"method\":\"ping\"'*)\n\
               printf '{{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":{{}}}}\\n' \"$next\"\n\
               next=$((next + 1)) ;;\n\
           esac\n\
         done\n",
        capture = capture.display()
    );
    std::fs::write(&path, script)?;
    let mut permissions = std::fs::metadata(&path)?.permissions();
    permissions.set_mode(0o700);
    std::fs::set_permissions(&path, permissions)?;
    Ok((path, capture))
}
