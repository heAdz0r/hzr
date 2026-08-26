#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use hzr_core::Config;
use hzr_daemon::{AppState, AuthToken, router};
use hzr_index::Workspace;
use serde_json::Value;
use tokio::time::sleep;
use tower::ServiceExt;

const TOKEN: &str = "0000000000000000000000000000000000000000000000000000000000000000";

#[tokio::test]
async fn live_and_failed_grepai_watcher_are_visible_through_health_and_dashboard() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let engines = directory.path().join("engines");
    fs::create_dir(&engines).expect("engine directory");
    write_fake_grepai(&engines);
    write_fake_rtk(&engines);

    let root = directory.path().join("workspace");
    fs::create_dir(&root).expect("workspace directory");
    fs::create_dir(root.join("src")).expect("source directory");
    fs::write(root.join("src/lib.rs"), "pub fn indexed_symbol() {}\n").expect("source fixture");

    let mut config = Config {
        data_dir: directory.path().join("data"),
        ..Config::default()
    };
    config.engines.directory = Some(engines);
    config.engines.auto_start_icm = false;
    config.engines.auto_index = true;
    let worktree_id = Workspace::discover_managed(
        &root,
        Path::new("git"),
        &config.data_dir,
        Duration::from_secs(3),
    )
    .await
    .expect("managed workspace")
    .register()
    .expect("workspace registration")
    .worktree_id;

    let state = AppState::initialize(config)
        .await
        .expect("daemon state initializes");
    let token = AuthToken::new(TOKEN.to_owned()).expect("token");
    let application = router(state.clone(), token);

    let search = application
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/search")
                .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "workspace": root,
                        "query": "indexed_symbol",
                        "limit": 5,
                        "mode": "semantic",
                        "include_content": true
                    })
                    .to_string(),
                ))
                .expect("search request"),
        )
        .await
        .expect("search response");
    let search_status = search.status();
    let search_body = to_bytes(search.into_body(), 1_048_576)
        .await
        .expect("search body");
    assert_eq!(
        search_status,
        StatusCode::OK,
        "search failed: {}",
        String::from_utf8_lossy(&search_body)
    );

    assert_grepai_health(&application, "ready", "1 active watcher(s), 0 failed").await;
    assert_dashboard_watcher(
        &application,
        &worktree_id,
        "ready",
        "HZR-owned watcher is live",
    )
    .await;

    fs::write(root.join("fake-watch-die"), "die\n").expect("failure sentinel");
    for _ in 0..30 {
        sleep(Duration::from_millis(50)).await;
        if grepai_state(&application).await == "degraded" {
            break;
        }
    }

    assert_grepai_health(&application, "degraded", "0 active watcher(s), 1 failed").await;
    assert_dashboard_watcher(
        &application,
        &worktree_id,
        "degraded",
        "Managed watcher exited unexpectedly",
    )
    .await;

    state.index_maintenance_stop.store(true, Ordering::Release);
    if let Some(task) = state.index_maintenance_task.lock().await.take() {
        task.abort();
        let _ = task.await;
    }
    state
        .context
        .shutdown()
        .await
        .expect("index coordinator shutdown");
}

async fn grepai_state(application: &axum::Router) -> String {
    let payload = authenticated_json(application, "/v1/health").await;
    payload["engines"]
        .as_array()
        .expect("engine array")
        .iter()
        .find(|engine| engine["name"] == "grepai")
        .and_then(|engine| engine["state"].as_str())
        .expect("grepai state")
        .to_owned()
}

async fn assert_grepai_health(application: &axum::Router, state: &str, detail: &str) {
    let payload = authenticated_json(application, "/v1/health").await;
    let grepai = payload["engines"]
        .as_array()
        .expect("engine array")
        .iter()
        .find(|engine| engine["name"] == "grepai")
        .expect("grepai health");
    assert_eq!(
        grepai["state"], state,
        "unexpected grepai health payload: {grepai}"
    );
    assert!(
        grepai["detail"]
            .as_str()
            .is_some_and(|value| value.contains(detail)),
        "unexpected grepai health detail: {}",
        grepai["detail"]
    );
}

async fn assert_dashboard_watcher(
    application: &axum::Router,
    worktree_id: &str,
    state: &str,
    detail: &str,
) {
    let response = application
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/v1/dashboard?project={worktree_id}"))
                .body(Body::empty())
                .expect("dashboard request"),
        )
        .await
        .expect("dashboard response");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 1_048_576)
        .await
        .expect("dashboard body");
    let payload: Value = serde_json::from_slice(&bytes).expect("dashboard JSON");
    assert_eq!(payload["index_observatory"]["watcher"]["state"], state);
    assert_eq!(payload["index_observatory"]["watcher"]["detail"], detail);
}

async fn authenticated_json(application: &axum::Router, uri: &str) -> Value {
    let response = application
        .clone()
        .oneshot(
            Request::builder()
                .uri(uri)
                .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .expect("authenticated request"),
        )
        .await
        .expect("authenticated response");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 1_048_576)
        .await
        .expect("response body");
    serde_json::from_slice(&bytes).expect("response JSON")
}

fn write_fake_grepai(engines: &Path) -> PathBuf {
    let path = engines.join("grepai");
    let script = format!(
        r#"#!/bin/sh
set -eu
command_name="${{1:-}}"
case "$command_name" in
  version)
    printf 'grepai version {}\n'
    ;;
  init)
    mkdir -p .grepai
    printf 'version: 1\nrpg:\n    enabled: false\n' > .grepai/config.yaml
    : > .grepai/index.gob
    : > .grepai/symbols.gob
    ;;
  search)
    printf '[{{"file_path":"src/lib.rs","start_line":1,"end_line":1,"score":0.91,"content":"pub fn indexed_symbol() {{}}\\n","feature_path":"feature/index","symbol_name":"indexed_symbol"}}]\n'
    ;;
  watch)
    if [ "${{2:-}}" = "--help" ]; then
      printf 'Usage: grepai watch [flags]\n      --no-worktree-discovery\n'
      exit 0
    fi
    shift
    stop=0
    log_dir=""
    while [ "$#" -gt 0 ]; do
      case "$1" in
        --stop) stop=1 ;;
        --log-dir) shift; log_dir="$1" ;;
      esac
      shift
    done
    if [ "$stop" -eq 1 ]; then
      if [ -f "$log_dir/fake.pid" ]; then
        watcher_pid=$(cat "$log_dir/fake.pid")
        kill -TERM "$watcher_pid" 2>/dev/null || true
      fi
      exit 0
    fi
    mkdir -p "$log_dir"
    printf '%s\n' "$$" > "$log_dir/fake.pid"
    printf 'ready\n%s\n' "$$" > "$log_dir/fake.ready"
    cleanup() {{ rm -f "$log_dir/fake.pid" "$log_dir/fake.ready"; exit 0; }}
    trap cleanup INT TERM
    while [ ! -f fake-watch-die ]; do sleep 1; done
    exit 17
    ;;
  *)
    printf 'unsupported fake command: %s\n' "$command_name" >&2
    exit 2
    ;;
esac
"#,
        hzr_index::SUPPORTED_GREPAI_VERSION
    );
    fs::write(&path, script).expect("fake grepai");
    let mut permissions = fs::metadata(&path).expect("fake metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).expect("fake executable");
    path
}

fn write_fake_rtk(engines: &Path) {
    let path = engines.join("rtk");
    let script = r#"#!/bin/sh
case "$1" in
  --version)
    printf '%s\n' 'rtk 0.44.1-fork.1'
    ;;
  rewrite)
    if [ "${2:-}" = "--help" ]; then
      printf '%s\n' 'rtk rewrite - Raw command to rewrite'
    else
      exit 64
    fi
    ;;
  proxy)
    if [ "${2:-}" = "--help" ]; then
      printf '%s\n' 'rtk proxy - execute without filtering'
    else
      exit 64
    fi
    ;;
  config)
    printf '%s\n' '{"schema_version":2,"config_path":"/tmp/hzr-daemon-test-no-rtk-config","config_exists":false,"config_sha256":null,"config":{"grepai":{"enabled":true,"auto_init":true,"binary_path":null}}}'
    ;;
  rgai)
    printf '%s\n' '{"query":"indexed_symbol","path":".","total_hits":1,"shown_hits":1,"scanned_files":1,"skipped_large":0,"skipped_binary":0,"hits":[{"path":"src/lib.rs","score":9.5,"matched_lines":1,"snippets":[{"lines":[{"line":1,"text":"pub fn indexed_symbol() {}"}],"matched_terms":["indexed_symbol"]}]}]}'
    ;;
  *)
    exit 67
    ;;
esac
"#;
    fs::write(&path, script).expect("fake rtk");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("fake rtk executable");
}
