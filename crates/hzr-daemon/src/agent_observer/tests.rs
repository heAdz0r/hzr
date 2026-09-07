use std::path::Path;

use hzr_core::{AgtxProject, Config};

use super::*;

#[tokio::test]
async fn resuming_after_disable_allows_observation_again() {
    let _spawn = SPAWN_GUARD.lock().await;
    let harness = harness();
    let body = snapshot_body(&harness, "snapshot-v1-ready.json", &request_id(&harness, 0));
    stub_helper(&harness.engines, &body);
    harness.observer.stop();
    harness.observer.resume();
    let outcome = harness.observer.observe_once(&harness.project).await;
    assert!(outcome.complete, "{outcome:?}");
}

#[tokio::test]
async fn helper_output_is_capped_before_waiting_for_exit() {
    let _spawn = SPAWN_GUARD.lock().await;
    let dir = tempfile::tempdir().expect("temp");
    let path = stub_helper(dir.path(), "{}");
    std::fs::write(
        &path,
        "#!/bin/sh\nwhile :; do printf '0123456789abcdef'; done\n",
    )
    .expect("script");
    let result = bounded_helper_output(&path, &[], &[], 128, Duration::from_secs(10)).await;
    assert_eq!(
        result.expect_err("helper must fail"),
        "helper_response_too_large"
    );
}

#[tokio::test]
async fn a_nonzero_exit_cannot_supply_a_successful_snapshot() {
    let _spawn = SPAWN_GUARD.lock().await;
    let dir = tempfile::tempdir().expect("temp");
    let path = stub_helper(dir.path(), "{}");
    std::fs::write(&path, "#!/bin/sh\nprintf '{}'; exit 7\n").expect("script");
    let result = bounded_helper_output(&path, &[], &[], 128, Duration::from_secs(10)).await;
    assert_eq!(result.expect_err("helper must fail"), "helper_exit_failed");
}

#[tokio::test]
async fn an_empty_worker_does_not_probe_an_installed_component() {
    let _spawn = SPAWN_GUARD.lock().await;
    let harness = harness();
    let mut enrollment = harness.observer.enrollments().await;
    enrollment.disable_project(&harness.project);
    harness.observer.set_enrollments(enrollment).await;
    let path = stub_helper(&harness.engines, "{}");
    let marker = harness.engines.join("probe-marker");
    std::fs::write(path, format!("#!/bin/sh\ntouch '{}'\n", marker.display())).expect("script");
    let worker = tokio::spawn(harness.observer.clone().run());
    tokio::time::sleep(Duration::from_millis(50)).await;
    worker.abort();
    let _ = worker.await;
    assert!(!marker.exists());
}

#[tokio::test]
async fn a_concurrent_sync_for_the_same_project_is_refused() {
    let harness = harness();
    let lock = Arc::new(Mutex::new(()));
    harness
        .observer
        .inner
        .project_locks
        .lock()
        .await
        .insert(harness.project.clone(), lock.clone());
    let _guard = lock.lock().await;
    let result = harness.observer.observe_once(&harness.project).await;
    assert_eq!(
        result.error_code.as_deref(),
        Some("observation_in_progress")
    );
}

/// Serializes the tests that actually spawn the stub helper.
///
/// The daemon's other suites probe the pinned fork-core with a five-second
/// budget, and a burst of concurrent process spawns from here is enough to
/// starve that probe on a loaded machine. Running the spawning tests one at a
/// time costs a few seconds and removes a cross-suite flake.
static SPAWN_GUARD: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Write an executable stub that answers `--version` like the pinned observer
/// and echoes `body` for a snapshot request.
///
/// A stub rather than the real component on purpose: the real one is optional
/// and absent in CI, and what these tests are about is HZR's side of the
/// contract — spawning, bounding, classifying and refusing.
fn stub_helper(dir: &Path, body: &str) -> PathBuf {
    stub_helper_with_version(
        dir,
        body,
        &format!(
            "hzr-agtx-observer 1.0.4 schema={} patch={}",
            AGENT_SNAPSHOT_SCHEMA_VERSION, AGENT_OBSERVER_PATCH_IDENTITY
        ),
    )
}

fn stub_helper_with_version(dir: &Path, body: &str, version_line: &str) -> PathBuf {
    let path = dir.join(OBSERVER_BINARY);
    // The body carries `__REQUEST_ID__`, which the stub replaces with the id it
    // was actually handed — otherwise a second page would look like somebody
    // else's response and be refused for the wrong reason.
    let script = format!(
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo '{version_line}'; exit 0; fi\nREQ=$(cat)\nID=$(printf '%s' \"$REQ\" | sed -n 's/.*\"request_id\":\"\\([^\"]*\\)\".*/\\1/p')\ncat <<'HZREOF' | sed \"s/__REQUEST_ID__/$ID/\"\n{body}\nHZREOF\n"
    );
    std::fs::write(&path, script).expect("stub written");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("fixture step succeeds");
    }
    path
}

fn fixture(name: &str) -> serde_json::Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../integrations/agtx/fixtures")
        .join(name);
    serde_json::from_str(&std::fs::read_to_string(&path).expect("fixture readable"))
        .expect("fixture parses")
}

struct Harness {
    _dir: tempfile::TempDir,
    observer: AgentObserver,
    project: PathBuf,
    data_root: PathBuf,
    engines: PathBuf,
}

fn harness() -> Harness {
    let dir = tempfile::tempdir().expect("temp dir");
    let engines = dir.path().join("engines");
    let project = dir.path().join("work/repo");
    let data_root = dir.path().join("agtx");
    std::fs::create_dir_all(&engines).expect("fixture step succeeds");
    std::fs::create_dir_all(&project).expect("fixture step succeeds");
    std::fs::create_dir_all(&data_root).expect("fixture step succeeds");

    let mut config = Config {
        data_dir: dir.path().join("hzr"),
        ..Config::default()
    };
    config.engines.directory = Some(engines.clone());
    config.integrations.agtx.enabled = true;
    config.integrations.agtx.projects = vec![AgtxProject {
        project_path: project.clone(),
        data_dir: data_root.clone(),
        enabled: true,
    }];
    let ledger = LedgerWriter::open(&config.data_dir.join("ledger.sqlite")).expect("ledger");
    let observer = AgentObserver::new(Arc::new(config), ledger);
    Harness {
        _dir: dir,
        observer,
        project,
        data_root,
        engines,
    }
}

/// The recorded fixture, retargeted at this harness's request id.
fn snapshot_body(harness: &Harness, name: &str, request_id: &str) -> String {
    let mut value = fixture(name);
    value["request_id"] = serde_json::Value::String(request_id.to_string());
    let _ = &harness.data_root;
    serde_json::to_string(&value).expect("fixture step succeeds")
}

/// A body whose request id the stub fills in from the actual request.
fn echoing_body(name: &str) -> String {
    let mut value = fixture(name);
    value["request_id"] = serde_json::Value::String("__REQUEST_ID__".into());
    serde_json::to_string(&value).expect("fixture step succeeds")
}

fn request_id(harness: &Harness, page: usize) -> String {
    let project_hash =
        hzr_core::privacy_identity_hash("project", &harness.project.to_string_lossy());
    format!("{project_hash}:1:{page}")
}

#[tokio::test]
async fn an_absent_component_reports_missing_and_spawns_nothing() {
    let _spawn = SPAWN_GUARD.lock().await;
    let harness = harness();
    harness.observer.refresh_component().await;
    let outcome = harness.observer.observe_once(&harness.project).await;
    assert_eq!(outcome.state, AgentIntegrationState::MissingComponent);
    assert_eq!(outcome.error_code.as_deref(), Some("missing_component"));
    assert_eq!(outcome.pages, 0);
}

#[tokio::test]
async fn a_disabled_integration_never_runs_the_helper() {
    let _spawn = SPAWN_GUARD.lock().await;
    let harness = harness();
    stub_helper(&harness.engines, "{}");
    harness.observer.refresh_component().await;

    let mut disabled = harness.observer.enrollments().await;
    disabled.disable_project(&harness.project);
    harness.observer.set_enrollments(disabled).await;

    let outcome = harness.observer.observe_once(&harness.project).await;
    assert_eq!(outcome.state, AgentIntegrationState::Disabled);
    assert_eq!(outcome.pages, 0);
    assert_eq!(
        harness.observer.state_for(&harness.project).await,
        AgentIntegrationState::Disabled
    );
}

#[tokio::test]
async fn a_foreign_producer_is_refused() {
    let _spawn = SPAWN_GUARD.lock().await;
    let harness = harness();
    stub_helper_with_version(
        &harness.engines,
        "{}",
        "hzr-agtx-observer 1.0.4 schema=1 patch=some-other-observer",
    );
    harness.observer.refresh_component().await;
    assert!(!harness.observer.component().await.usable());
    let outcome = harness.observer.observe_once(&harness.project).await;
    assert_eq!(outcome.state, AgentIntegrationState::Incompatible);
}

#[tokio::test]
async fn an_unknown_protocol_major_is_refused() {
    let _spawn = SPAWN_GUARD.lock().await;
    let harness = harness();
    stub_helper_with_version(
        &harness.engines,
        "{}",
        &format!(
            "hzr-agtx-observer 9.9.9 schema=2 patch={}",
            AGENT_OBSERVER_PATCH_IDENTITY
        ),
    );
    harness.observer.refresh_component().await;
    let outcome = harness.observer.observe_once(&harness.project).await;
    assert_eq!(outcome.state, AgentIntegrationState::Incompatible);
}

#[tokio::test]
async fn a_ready_fixture_is_ingested_once_and_replays_clean() {
    let _spawn = SPAWN_GUARD.lock().await;
    let harness = harness();
    let body = snapshot_body(&harness, "snapshot-v1-ready.json", &request_id(&harness, 0));
    stub_helper(&harness.engines, &body);
    harness.observer.refresh_component().await;

    let first = harness.observer.observe_once(&harness.project).await;
    assert_eq!(first.state, AgentIntegrationState::Ready);
    assert_eq!(first.pages, 1);
    assert_eq!(first.tasks_observed, 3);
    assert!(first.complete);
    assert!(first.events_recorded >= 3);

    let second = harness.observer.observe_once(&harness.project).await;
    assert_eq!(second.tasks_observed, 3);
    assert_eq!(
        second.events_recorded, 0,
        "an identical page adds no events on replay"
    );
    assert_eq!(second.tombstoned, 0);
}

#[tokio::test]
async fn invalid_json_becomes_a_typed_error_not_a_panic() {
    let _spawn = SPAWN_GUARD.lock().await;
    let harness = harness();
    stub_helper(&harness.engines, "this is not json");
    harness.observer.refresh_component().await;
    let outcome = harness.observer.observe_once(&harness.project).await;
    assert_eq!(outcome.state, AgentIntegrationState::Error);
    assert_eq!(
        outcome.error_code.as_deref(),
        Some("helper_invalid_response")
    );
}

#[tokio::test]
async fn a_helper_refusal_keeps_its_stable_code() {
    let _spawn = SPAWN_GUARD.lock().await;
    let harness = harness();
    stub_helper(
        &harness.engines,
        r#"{"schema_version":1,"request_id":"x","error":"snapshot_failed","detail":"project store is not readable"}"#,
    );
    harness.observer.refresh_component().await;
    let outcome = harness.observer.observe_once(&harness.project).await;
    assert_eq!(
        outcome.error_code.as_deref(),
        Some("helper_refused:snapshot_failed")
    );
}

#[tokio::test]
async fn a_mismatched_request_id_is_refused() {
    let _spawn = SPAWN_GUARD.lock().await;
    let harness = harness();
    let body = snapshot_body(&harness, "snapshot-v1-ready.json", "someone-elses-request");
    stub_helper(&harness.engines, &body);
    harness.observer.refresh_component().await;
    let outcome = harness.observer.observe_once(&harness.project).await;
    assert_eq!(outcome.error_code.as_deref(), Some("request_id_mismatch"));
}

#[tokio::test]
async fn a_wire_schema_two_payload_is_refused() {
    let _spawn = SPAWN_GUARD.lock().await;
    let harness = harness();
    let mut value = fixture("snapshot-v1-incompatible.json");
    value["request_id"] = serde_json::Value::String(request_id(&harness, 0));
    stub_helper(
        &harness.engines,
        &serde_json::to_string(&value).expect("fixture step succeeds"),
    );
    harness.observer.refresh_component().await;
    let outcome = harness.observer.observe_once(&harness.project).await;
    assert_eq!(
        outcome.error_code.as_deref(),
        Some("incompatible_schema_version")
    );
}

#[tokio::test]
async fn a_partial_page_reports_partial_and_never_tombstones() {
    let _spawn = SPAWN_GUARD.lock().await;
    let harness = harness();
    stub_helper(&harness.engines, &echoing_body("snapshot-v1-partial.json"));
    harness.observer.refresh_component().await;

    let outcome = harness.observer.observe_once(&harness.project).await;
    // The stub answers every page identically, so the cursor never advances and
    // the cycle stops at its page cap without ever completing a traversal.
    assert_eq!(outcome.state, AgentIntegrationState::Partial);
    assert!(!outcome.complete);
    assert_eq!(outcome.tombstoned, 0);
}

#[tokio::test]
async fn a_slow_helper_times_out_within_its_budget() {
    let _spawn = SPAWN_GUARD.lock().await;
    let harness = harness();
    let path = harness.engines.join(OBSERVER_BINARY);
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'hzr-agtx-observer 1.0.4 schema={} patch={}'; exit 0; fi\nsleep 8\n",
            AGENT_SNAPSHOT_SCHEMA_VERSION, AGENT_OBSERVER_PATCH_IDENTITY
        ),
    )
    .expect("fixture step succeeds");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("fixture step succeeds");
    }
    harness.observer.refresh_component().await;

    let started = std::time::Instant::now();
    let outcome = harness.observer.observe_once(&harness.project).await;
    assert_eq!(outcome.error_code.as_deref(), Some("helper_timeout"));
    assert!(
        started.elapsed() < HELPER_TIMEOUT + Duration::from_secs(2),
        "the timeout must bound the call, not merely be declared"
    );
}

#[tokio::test]
async fn revoking_an_enrollment_discards_a_result_already_in_flight() {
    let _spawn = SPAWN_GUARD.lock().await;
    let harness = harness();
    let body = snapshot_body(&harness, "snapshot-v1-ready.json", &request_id(&harness, 0));
    let path = harness.engines.join(OBSERVER_BINARY);
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'hzr-agtx-observer 1.0.4 schema={} patch={}'; exit 0; fi\ncat >/dev/null\nsleep 1\ncat <<'HZREOF'\n{body}\nHZREOF\n",
            AGENT_SNAPSHOT_SCHEMA_VERSION, AGENT_OBSERVER_PATCH_IDENTITY
        ),
    )
    .expect("fixture step succeeds");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("fixture step succeeds");
    }
    harness.observer.refresh_component().await;

    let observer = harness.observer.clone();
    let project = harness.project.clone();
    let inflight = tokio::spawn(async move { observer.observe_once(&project).await });
    tokio::time::sleep(Duration::from_millis(200)).await;

    let mut revoked = harness.observer.enrollments().await;
    revoked.disable_project(&harness.project);
    harness.observer.set_enrollments(revoked).await;

    let outcome = tokio::time::timeout(REAP_TIMEOUT, inflight)
        .await
        .expect("the in-flight observation is reaped within the budget")
        .expect("task joins");
    assert_eq!(outcome.error_code.as_deref(), Some("enrollment_revoked"));
    assert_eq!(outcome.state, AgentIntegrationState::Disabled);
}

#[tokio::test]
async fn a_source_reset_does_not_merge_with_the_previous_generation() {
    let _spawn = SPAWN_GUARD.lock().await;
    let harness = harness();
    let ready = snapshot_body(&harness, "snapshot-v1-ready.json", &request_id(&harness, 0));
    stub_helper(&harness.engines, &ready);
    harness.observer.refresh_component().await;
    harness.observer.observe_once(&harness.project).await;

    let reset = snapshot_body(
        &harness,
        "snapshot-v1-source-reset.json",
        &request_id(&harness, 0),
    );
    stub_helper(&harness.engines, &reset);
    let outcome = harness.observer.observe_once(&harness.project).await;
    assert_eq!(outcome.state, AgentIntegrationState::Ready);
    assert_eq!(
        outcome.tombstoned, 0,
        "a new generation starts fresh instead of tombstoning the old one"
    );
}

#[test]
fn backoff_grows_and_stops_growing() {
    let first = backoff_ms(1);
    let second = backoff_ms(2);
    let last = backoff_ms(50);
    assert!((5_000..5_500).contains(&first));
    assert!(second > first);
    assert!((60_000..60_500).contains(&last));
    assert_eq!(last, backoff_ms(5), "the ladder has a ceiling");
}

#[test]
fn the_component_is_never_resolved_from_path() {
    let mut config = Config::default();
    config.engines.directory = Some(PathBuf::from("/opt/hzr/engines"));
    assert_eq!(
        component_path(&config),
        PathBuf::from("/opt/hzr/engines").join(OBSERVER_BINARY)
    );
    config.engines.directory = None;
    config.data_dir = PathBuf::from("/var/hzr");
    assert_eq!(
        component_path(&config),
        PathBuf::from("/var/hzr/components").join(OBSERVER_BINARY)
    );
}
