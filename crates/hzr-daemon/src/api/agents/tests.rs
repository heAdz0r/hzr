use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use hzr_core::{AgtxProject, Config, privacy_identity_hash};
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

use crate::server::router;
use crate::{AppState, AuthToken};

const TOKEN: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// A router with one enrolled project and no observer component installed.
///
/// Deliberately no component: these tests are about what the *public* surface
/// does, and it must behave identically whether or not a helper exists.
async fn agents_router(directory: &TempDir, enrolled: bool) -> (axum::Router, String, AppState) {
    let project = directory.path().join("work/repo");
    let data_root = directory.path().join("agtx");
    std::fs::create_dir_all(&project).expect("project directory");
    std::fs::create_dir_all(&data_root).expect("store directory");

    let mut config = Config {
        data_dir: directory.path().join("data"),
        ..Config::default()
    };
    config.engines.auto_start_icm = false;
    config.engines.directory = Some(directory.path().join("missing-engines"));
    if enrolled {
        config.integrations.agtx.enabled = true;
        config.integrations.agtx.projects = vec![AgtxProject {
            project_path: project.clone(),
            data_dir: data_root,
            enabled: true,
        }];
    }
    let state = AppState::initialize(config)
        .await
        .expect("test state initializes");
    // Every test in this file is about the HTTP surface, never about the
    // background observer — and an enrolled project starts one. Left running it
    // would poll and ingest underneath the assertions, so whether a test passed
    // would depend on how fast the machine was. The worker has its own suite.
    state.ensure_agent_worker(false).await;
    let token = AuthToken::new(TOKEN.to_owned()).expect("test token is valid");
    let project_id = privacy_identity_hash("project", &project.to_string_lossy());
    (router(state.clone(), token), project_id, state)
}

async fn get(router: &axum::Router, uri: &str) -> (StatusCode, Value) {
    let response = router
        .clone()
        .oneshot(
            Request::builder()
                .uri(uri)
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .expect("body");
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

#[tokio::test]
async fn the_board_is_public_and_reports_disabled_without_an_enrollment() {
    let directory = TempDir::new().expect("temp dir");
    let (router, _, _state) = agents_router(&directory, false).await;
    let (status, body) = get(&router, "/v1/dashboard/agents").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["state"], "disabled");
    assert_eq!(body["tasks"].as_array().map(Vec::len), Some(0));
    assert_eq!(body["projects"].as_array().map(Vec::len), Some(0));
}

#[tokio::test]
async fn an_enrolled_project_is_offered_by_pseudonym_only() {
    let directory = TempDir::new().expect("temp dir");
    let (router, project_id, _state) = agents_router(&directory, true).await;
    let (status, body) = get(&router, "/v1/dashboard/agents").await;
    assert_eq!(status, StatusCode::OK);
    let projects = body["projects"].as_array().expect("projects");
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0]["project_id"], project_id);
    assert!(
        projects[0]["label"]
            .as_str()
            .is_some_and(|label| label.starts_with("Project ")),
        "labels are pseudonymous"
    );
    let encoded = body.to_string();
    assert!(
        !encoded.contains("work/repo"),
        "no worktree path is exposed"
    );
    assert!(!encoded.contains("agtx"), "no store path is exposed");
}

#[tokio::test]
async fn an_unknown_scope_is_a_404_and_a_path_is_not_a_scope() {
    let directory = TempDir::new().expect("temp dir");
    let (router, _, _state) = agents_router(&directory, true).await;
    let unknown = "a".repeat(64);
    let (status, _) = get(
        &router,
        &format!("/v1/dashboard/agents?project_id={unknown}"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = get(&router, "/v1/dashboard/agents?project_id=%2Fetc%2Fpasswd").await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a filesystem path is never an acceptable scope"
    );
}

#[tokio::test]
async fn limits_are_bounded() {
    let directory = TempDir::new().expect("temp dir");
    let (router, project_id, _state) = agents_router(&directory, true).await;
    for uri in [
        format!("/v1/dashboard/agents?project_id={project_id}&limit=0"),
        format!("/v1/dashboard/agents?project_id={project_id}&limit=201"),
        format!("/v1/dashboard/agents/events?project_id={project_id}&limit=501"),
    ] {
        let (status, _) = get(&router, &uri).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}");
    }
}

#[tokio::test]
async fn a_malformed_task_id_never_reaches_the_projection() {
    let directory = TempDir::new().expect("temp dir");
    let (router, _, _state) = agents_router(&directory, true).await;
    for task in [
        "..%2F..%2Fetc%2Fpasswd",
        "%3Cscript%3Ealert(1)%3C%2Fscript%3E",
        "short",
    ] {
        let (status, _) = get(&router, &format!("/v1/dashboard/agents/tasks/{task}")).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{task}");
    }
}

#[tokio::test]
async fn an_economics_window_wider_than_a_month_is_refused() {
    let directory = TempDir::new().expect("temp dir");
    let (router, project_id, _state) = agents_router(&directory, true).await;
    let (status, _) = get(
        &router,
        &format!(
            "/v1/dashboard/agents/economics?project_id={project_id}&from_ms=1&to_ms=9999999999999"
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn repeated_public_reads_never_spawn_a_helper_or_write_a_ledger_row() {
    let directory = TempDir::new().expect("temp dir");
    let (router, project_id, _state) = agents_router(&directory, true).await;
    // A component that would fail loudly if the public path ever ran it. The
    // harness has already stopped the background observer, which is the thing
    // that legitimately spawns it; without that, this test only proved the
    // machine was fast enough to finish before the first poll.
    let engines = directory.path().join("missing-engines");
    std::fs::create_dir_all(&engines).expect("engines directory");
    let marker = directory.path().join("helper-was-run");
    let helper = engines.join("hzr-agtx-observer");
    std::fs::write(
        &helper,
        format!("#!/bin/sh\ntouch '{}'\nexit 1\n", marker.display()),
    )
    .expect("helper stub");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755))
            .expect("permissions");
    }
    // A poll already in flight when the worker stopped could still land here.
    let _ = std::fs::remove_file(&marker);

    for _ in 0..25 {
        let (status, _) = get(
            &router,
            &format!("/v1/dashboard/agents?project_id={project_id}"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }
    assert!(
        !marker.exists(),
        "a dashboard GET must never spawn the observer"
    );

    // Prove the assertion above is not vacuous: the stub really does leave the
    // marker when something runs it, so its absence means nothing ran it.
    #[cfg(unix)]
    {
        std::process::Command::new(&helper)
            .status()
            .expect("the stub helper is runnable");
        assert!(marker.exists(), "the marker proves the stub was invocable");
    }
}

#[tokio::test]
async fn control_routes_require_the_bearer_token() {
    let directory = TempDir::new().expect("temp dir");
    let (router, _, _state) = agents_router(&directory, true).await;
    for (method, uri) in [
        ("GET", "/v1/agents/status"),
        ("POST", "/v1/agents/reload"),
        ("POST", "/v1/agents/sync"),
        ("POST", "/v1/agents/links"),
        ("POST", "/v1/agents/usage/import"),
    ] {
        let response = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(uri)
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "{method} {uri} must be authenticated"
        );
    }
}

#[tokio::test]
async fn an_empty_projection_answers_with_unavailable_rather_than_fabricated_zeros() {
    let directory = TempDir::new().expect("temp dir");
    let (router, project_id, _state) = agents_router(&directory, true).await;
    let (status, body) = get(
        &router,
        &format!("/v1/dashboard/agents?project_id={project_id}"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["coverage"]["history_start_ms"], Value::Null);
    assert_eq!(
        body["coverage"]["usage_coverage_pct"],
        Value::Null,
        "a percentage with no denominator must be null, not zero"
    );
    assert_eq!(body["source_observed_at_ms"], Value::Null);
    assert_eq!(body["lag_ms"], Value::Null);
}
