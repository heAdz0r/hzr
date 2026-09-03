use std::future::Future;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use hzr_core::Config;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::timeout::TimeoutLayer;

use crate::api;
use crate::auth::{AuthToken, authorize, load_or_create_token};
use crate::lock::DaemonLock;
use crate::state::{stop_index_maintenance, stop_memory_supervision};
use crate::visualizer;
use crate::{AppState, DaemonError};

pub fn router(state: AppState, token: AuthToken) -> Router {
    let timeout = std::time::Duration::from_millis(state.config.daemon.request_timeout_ms);
    let limit = state.config.daemon.request_limit_bytes;

    let authenticated = Router::new()
        .route("/v1/health", get(api::health))
        .route("/v1/engines", get(api::engines))
        .route("/v1/search", post(api::search))
        .route("/v1/search/readiness", post(api::semantic_readiness))
        .route("/v1/context/plan", post(api::context_plan))
        .route("/v1/memory/recall", post(api::memory_recall))
        .route("/v1/memory/store", post(api::memory_store))
        .route("/v1/memory/forget", post(api::memory_forget))
        .route("/v1/memory/update", post(api::memory_update))
        .route("/v1/memory/prune", post(api::memory_prune))
        .route(
            "/v1/memory/topics/{topic_id}",
            get(api::memory_topic_details),
        )
        .route("/v1/exec/rewrite", post(api::exec_rewrite))
        .route("/v1/exec/run", post(api::exec_run))
        .route("/v1/exec/approval", post(api::exec_approval))
        .route("/v1/fidelity/reconcile", post(api::fidelity_reconcile))
        .route("/v1/fork/run", post(api::fork_run))
        .route("/v1/codec/compile", post(api::codec_compile))
        .route("/v1/usage", post(api::usage))
        .route("/v1/billing/receipts", post(api::provider_receipt))
        .route("/v1/operations", post(api::operation))
        .route("/v1/policy/events", post(api::policy_event))
        .route_layer(middleware::from_fn_with_state(token, authorize));
    let public = Router::new()
        .route("/v1/dashboard", get(api::dashboard))
        .route("/v1/dashboard/projects", get(api::dashboard_projects))
        .route(
            "/v1/dashboard/observability",
            get(api::dashboard_observability),
        )
        .route(
            "/v1/dashboard/memory/topics/{topic_id}",
            get(api::dashboard_memory_topic),
        );
    let router = public.merge(authenticated);
    let router = if let Some(directory) = visualizer::assets_directory() {
        router.fallback_service(ServeDir::new(directory).append_index_html_on_directories(true))
    } else {
        router.route("/", get(visualizer_unavailable))
    };

    router
        .layer(DefaultBodyLimit::max(limit))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            timeout,
        ))
        .layer(CatchPanicLayer::new())
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(
                "default-src 'self'; connect-src 'self'; img-src 'self' data:; \
                 style-src 'self'; script-src 'self'; font-src 'self'; object-src 'none'; \
                 base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
            ),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_FRAME_OPTIONS,
            HeaderValue::from_static("DENY"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            HeaderValue::from_static("no-store"),
        ))
        .with_state(state)
}

async fn visualizer_unavailable() -> impl IntoResponse {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        "HZR visualizer assets are unavailable; build them with `bun run build` in `visualizer/`",
    )
}

pub async fn serve<F>(config: Config, shutdown: F) -> Result<(), DaemonError>
where
    F: Future<Output = ()> + Send + 'static,
{
    config
        .validate()
        .map_err(|error| DaemonError::Config(error.to_string()))?;
    config
        .ensure_layout()
        .map_err(|error| DaemonError::Config(error.to_string()))?;
    let _daemon_lock = DaemonLock::acquire(&config.data_dir)?;
    let (token, _) = load_or_create_token(&config.data_dir).map_err(DaemonError::Io)?;
    let listener = tokio::net::TcpListener::bind(config.daemon.bind)
        .await
        .map_err(DaemonError::Io)?;
    let address = listener.local_addr().map_err(DaemonError::Io)?;
    eprintln!("hzrd listening on {address}; index coordinators start on demand");
    let state = AppState::initialize(config).await?;
    let accounting_task = tokio::spawn(crate::accounting_sweeper::run(state.clone()));
    let shutdown_state = state.clone();
    let serve_result = axum::serve(listener, router(state, token))
        .with_graceful_shutdown(shutdown)
        .await
        .map_err(DaemonError::Io);
    let (memory_stop, context_stop) = tokio::join!(
        stop_memory_supervision(&shutdown_state),
        stop_index_maintenance(&shutdown_state)
    );
    accounting_task.abort();
    let _ = accounting_task.await;
    serve_result?;
    memory_stop.map_err(DaemonError::Memory)?;
    context_stop.map_err(DaemonError::Context)
}

#[cfg(test)]
mod tests {
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode, header};
    use hzr_core::{
        Config, Ledger, OperationAttribution, OperationChannel, OperationMeasurement,
        OperationRoute,
    };
    use hzr_exec::{PINNED_RTK_VERSION, expected_engine_identity};
    use hzr_index::{WORKSPACE_REGISTRATION_SCHEMA_VERSION, Workspace, WorkspaceRegistration};
    use hzr_protocol::{DashboardLifecycleKind, DashboardTraceStage, DashboardTraceState};
    use rusqlite::{Connection, params};
    use serde_json::Value;
    use tempfile::TempDir;
    use tower::ServiceExt;

    use super::router;
    use crate::observability::TraceSpanInput;
    use crate::{AppState, AuthToken};

    const TOKEN: &str = "0000000000000000000000000000000000000000000000000000000000000000";

    async fn test_router(directory: &TempDir) -> axum::Router {
        let mut config = Config {
            data_dir: directory.path().join("data"),
            ..Config::default()
        };
        config.engines.auto_start_icm = false;
        config.engines.directory = Some(directory.path().join("missing-engines"));
        let state = AppState::initialize(config)
            .await
            .expect("test state initializes");
        let token = AuthToken::new(TOKEN.to_owned()).expect("test token is valid");
        router(state, token)
    }

    async fn test_router_with_workspace(directory: &TempDir) -> (axum::Router, String) {
        let workspace_root = directory.path().join("workspace");
        let mut config = Config {
            data_dir: directory.path().join("data"),
            ..Config::default()
        };
        config.engines.auto_start_icm = false;
        config.engines.directory = Some(directory.path().join("missing-engines"));
        let worktree_id = register_test_workspace(&config, &workspace_root).await;
        let state = AppState::initialize(config)
            .await
            .expect("test state initializes");
        let token = AuthToken::new(TOKEN.to_owned()).expect("test token is valid");
        (router(state, token), worktree_id)
    }

    #[cfg(unix)]
    async fn test_router_with_workspace_and_fake_rtk(
        directory: &TempDir,
    ) -> (axum::Router, String) {
        use std::os::unix::fs::PermissionsExt;

        let workspace_root = directory.path().join("workspace");
        let engines = directory.path().join("engines");
        std::fs::create_dir_all(&engines).expect("engine directory");
        let binary = engines.join("rtk");
        let contract =
            serde_json::to_string(&expected_engine_identity().expect("current engine identity"))
                .expect("contract JSON");
        let script = format!(
            r#"#!/bin/sh
if test "${{1:-}}" = --version; then
  printf 'rtk %s\n' '{PINNED_RTK_VERSION}'
  exit 0
fi
if test "${{1:-}}" = contract && test "${{2:-}}" = --json; then
  printf '%s\n' '{contract}'
  exit 0
fi
if test "${{1:-}}" = rewrite && test "${{2:-}}" = --help; then
  printf 'Usage: rtk rewrite [ARGS]... Raw command to rewrite\n'
  exit 0
fi
if test "${{1:-}}" = proxy && test "${{2:-}}" = --help; then
  printf 'Usage: rtk proxy [ARGS]... Execute command without filtering\n'
  exit 0
fi
if test "${{1:-}}" = rewrite-plan; then
  printf '{{"decision":"proxy"}}'
  exit 0
fi
exit 64
"#,
            contract = contract,
        );
        std::fs::write(&binary, script).expect("fake rtk");
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700))
            .expect("fake rtk permissions");
        let mut config = Config {
            data_dir: directory.path().join("data"),
            ..Config::default()
        };
        config.engines.auto_start_icm = false;
        config.engines.directory = Some(engines);
        let worktree_id = register_test_workspace(&config, &workspace_root).await;
        let state = AppState::initialize(config)
            .await
            .expect("test state initializes");
        let token = AuthToken::new(TOKEN.to_owned()).expect("test token is valid");
        (router(state, token), worktree_id)
    }

    async fn test_router_with_fidelity_failure(directory: &TempDir) -> (axum::Router, String) {
        let workspace_root = directory.path().join("workspace");
        let mut config = Config {
            data_dir: directory.path().join("data"),
            ..Config::default()
        };
        config.engines.auto_start_icm = false;
        config.engines.directory = Some(directory.path().join("missing-engines"));
        let worktree_id = register_test_workspace(&config, &workspace_root).await;
        let state = AppState::initialize(config)
            .await
            .expect("test state initializes");
        state.ledger.inject_fidelity_failure(true);
        let token = AuthToken::new(TOKEN.to_owned()).expect("test token is valid");
        (router(state, token), worktree_id)
    }

    async fn register_test_workspace(config: &Config, root: &std::path::Path) -> String {
        std::fs::create_dir(root).expect("workspace directory");
        Workspace::discover_managed(
            root,
            std::path::Path::new("git"),
            &config.data_dir,
            std::time::Duration::from_secs(3),
        )
        .await
        .expect("managed workspace")
        .register()
        .expect("workspace registration")
        .worktree_id
    }

    #[tokio::test]
    async fn test_all_routes_require_bearer_authentication() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let response = test_router(&directory)
            .await
            .oneshot(
                Request::builder()
                    .uri("/v1/health")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_dashboard_snapshot_is_public_but_read_only() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let response = test_router(&directory)
            .await
            .oneshot(
                Request::builder()
                    .uri("/v1/dashboard")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::X_CONTENT_TYPE_OPTIONS),
            Some(&axum::http::HeaderValue::from_static("nosniff"))
        );
        let bytes = to_bytes(response.into_body(), 1_048_576)
            .await
            .expect("response body");
        let payload: Value = serde_json::from_slice(&bytes).expect("valid JSON response");
        assert_eq!(
            payload.get("protocol_version").and_then(Value::as_u64),
            Some(1)
        );
        let services = payload
            .get("services")
            .and_then(Value::as_array)
            .expect("dashboard services");
        for required in ["hzrd", "rtk", "icm", "grepai"] {
            assert!(
                services
                    .iter()
                    .any(|service| service.get("id").and_then(Value::as_str) == Some(required)),
                "missing dashboard service {required}"
            );
        }
        assert!(payload.get("observed_usage").is_some());
        assert!(payload.get("estimated_efficiency").is_some());
        assert!(payload.get("memory_observatory").is_some());
        assert!(payload.get("index_observatory").is_some());
        assert!(payload["memory_observatory"]["observed_at_ms"].is_number());
        assert!(payload["memory_observatory"]["latency_ms"].is_number());
        assert_eq!(
            payload["memory_observatory"]["source"],
            "canonical_icm_store"
        );
        assert!(payload["index_observatory"]["observed_at_ms"].is_number());
        assert!(payload["index_observatory"]["artifacts"]["size_bytes"].is_number());
        assert_eq!(
            payload["index_observatory"]["search_activity"]["state"],
            "standby"
        );
        assert!(payload["index_observatory"]["search_activity"]["command"].is_null());
        assert!(payload.get("local_activity").is_some());
        assert!(payload.get("provider_receipts").is_some());
        assert!(payload.get("session_roi").is_some());
        assert!(payload["selected_worktree_id"].is_null());
        assert!(payload.get("token").is_none());
    }

    #[tokio::test]
    async fn test_dashboard_selects_only_a_valid_stable_project_id() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let (router, worktree_id) = test_router_with_workspace(&directory).await;
        let response = router
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/dashboard?project={worktree_id}"))
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), 1_048_576)
            .await
            .expect("response body");
        let payload: Value = serde_json::from_slice(&bytes).expect("valid JSON response");
        assert_eq!(payload["selected_worktree_id"], worktree_id);
        assert_eq!(payload["projects"].as_array().map(Vec::len), Some(1));
    }

    #[tokio::test]
    async fn test_dashboard_rejects_malformed_and_unknown_project_ids() {
        let directory = tempfile::tempdir().expect("temporary directory");
        for (project, expected) in [
            ("malformed".to_owned(), StatusCode::BAD_REQUEST),
            ("f".repeat(64), StatusCode::NOT_FOUND),
        ] {
            let response = test_router(&directory)
                .await
                .oneshot(
                    Request::builder()
                        .uri(format!("/v1/dashboard?project={project}"))
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("response");
            assert_eq!(response.status(), expected);
        }
    }

    #[tokio::test]
    async fn test_dashboard_project_selection_isolates_observatories_and_ledger_activity() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let mut config = Config {
            data_dir: directory.path().join("data"),
            ..Config::default()
        };
        config.engines.auto_start_icm = false;
        config.engines.directory = Some(directory.path().join("missing-engines"));
        config.billing.pricing_file = Some(directory.path().join("missing-pricing.json"));
        let first_root = directory.path().join("first-workspace");
        let second_root = directory.path().join("second-workspace");
        register_test_workspace(&config, &first_root).await;
        let second_id = register_test_workspace(&config, &second_root).await;
        let first_root = first_root
            .canonicalize()
            .expect("canonical first workspace");
        let second_root = second_root
            .canonicalize()
            .expect("canonical second workspace");
        let ledger = Ledger::open(&config.data_dir.join("ledger/hzr.sqlite")).expect("ledger");
        for root in [&first_root, &first_root, &second_root] {
            ledger
                .record_operation_attributed(
                    "cargo test",
                    "rtk cargo test",
                    10,
                    2,
                    1,
                    OperationAttribution {
                        project_path: root.to_str().expect("UTF-8 workspace"),
                        agent: Some("test-agent"),
                        session_id: Some("test-session"),
                        channel: OperationChannel::HookCli,
                        measurement: OperationMeasurement::Estimated,
                        route: OperationRoute::Optimized,
                    },
                )
                .expect("operation");
        }
        let state = AppState::initialize(config)
            .await
            .expect("test state initializes");
        let project_hash = state
            .observability
            .project_hash(second_root.to_str().expect("UTF-8 workspace"));
        let receipt_directory = second_root.join("nested");
        std::fs::create_dir_all(&receipt_directory).expect("nested receipt directory");
        let observed_at_ms = u64::try_from(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time")
                .as_millis(),
        )
        .expect("timestamp fits u64");
        let token = AuthToken::new(TOKEN.to_owned()).expect("test token is valid");
        let application = router(state, token);
        let receipt_response = application
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/billing/receipts")
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "receipt_id": "dashboard-import-1",
                            "source": "spoofed_provider_api",
                            "observed_at_ms": observed_at_ms,
                            "harness": "codex",
                            "provider": "openai",
                            "model": "gpt-5.6-sol",
                            "method": "standard_short_context_lte_272k",
                            "currency": "USD",
                            "request_input_tokens": 100000,
                            "session_id": "test-session",
                            "project_path": receipt_directory,
                            "baseline": {"input_tokens": 10, "output_tokens": 2},
                            "delivered": {"input_tokens": 5, "output_tokens": 1},
                            "actual_baseline_cost_microunits": 20,
                            "actual_delivered_cost_microunits": 10,
                            "enable_public_estimate": false
                        })
                        .to_string(),
                    ))
                    .expect("receipt request"),
            )
            .await
            .expect("receipt response");
        assert_eq!(receipt_response.status(), StatusCode::OK);
        let receipt_body = to_bytes(receipt_response.into_body(), 1_048_576)
            .await
            .expect("receipt body");
        let receipt_payload: Value = serde_json::from_slice(&receipt_body).expect("receipt JSON");
        assert_eq!(receipt_payload["provenance"], "user_supplied");
        assert_eq!(receipt_payload["externally_verified"], false);
        assert_eq!(receipt_payload["reported_actual"]["savings_microunits"], 10);
        assert!(receipt_payload["public_estimate"].is_null());

        let response = application
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/dashboard?project={second_id}"))
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), 1_048_576)
            .await
            .expect("response body");
        let payload: Value = serde_json::from_slice(&bytes).expect("valid JSON response");
        assert_eq!(payload["selected_worktree_id"], second_id);
        assert_eq!(payload["local_activity"]["operations"], 1);
        assert_eq!(payload["local_activity"]["project"], project_hash);
        assert_eq!(payload["index_observatory"]["project"], project_hash);
        assert_eq!(payload["memory_observatory"]["project"], project_hash);
        assert_eq!(payload["session_roi"]["operations"], 1);
        assert_eq!(payload["session_roi"]["imported_claim_records"], 1);
        assert_eq!(
            payload["session_roi"]["receipt_provenance"],
            "user_supplied"
        );
        assert_eq!(payload["session_roi"]["receipt_externally_verified"], false);
        assert_eq!(
            payload["session_roi"]["reported_actual"]["savings_microunits"],
            10
        );
        assert!(payload["session_roi"]["raw_public_estimate"].is_null());
        assert!(
            payload["session_roi"]["raw_public_estimate_unavailable_reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("opt-in"))
        );
        assert_eq!(
            payload["session_roi"]["top_commands"][0]["command_family"],
            "cargo"
        );
        let projects = payload["projects"].as_array().expect("project list");
        let names = projects
            .iter()
            .map(|project| project["name"].as_str().expect("safe project label"))
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(names.len(), 2);
        assert!(names.iter().all(|name| name.starts_with("Project ")));
        assert!(projects.iter().all(|project| {
            project["root"]
                .as_str()
                .is_some_and(|root| root.starts_with("hmac-sha256:"))
        }));
    }

    #[tokio::test]
    async fn public_observability_is_bounded_scoped_and_payload_free() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let mut config = Config {
            data_dir: directory.path().join("data"),
            ..Config::default()
        };
        config.engines.auto_start_icm = false;
        config.engines.directory = Some(directory.path().join("missing-engines"));
        let workspace_root = directory.path().join("workspace-with-secret-name");
        let worktree_id = register_test_workspace(&config, &workspace_root).await;
        let state = AppState::initialize(config)
            .await
            .expect("test state initializes");
        let trace = state.observability.begin_trace(
            workspace_root
                .canonicalize()
                .expect("canonical workspace")
                .to_str()
                .expect("UTF-8 workspace"),
            Some("secret-provider-session"),
        );
        state.observability.record_span(
            &trace,
            TraceSpanInput {
                stage: DashboardTraceStage::Engine,
                state: DashboardTraceState::Completed,
                engine: "grepai",
                duration_ms: 3,
                route: Some("search"),
                error_code: None,
                generation: Some("generation-1"),
            },
        );
        state.observability.record_lifecycle(
            "icm",
            DashboardLifecycleKind::RestartScheduled,
            Some(
                workspace_root
                    .canonicalize()
                    .expect("canonical workspace")
                    .to_str()
                    .expect("UTF-8 workspace"),
            ),
            "bounded_backoff",
            None,
        );
        let token = AuthToken::new(TOKEN.to_owned()).expect("test token is valid");
        let application = router(state, token);
        let response = application
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/v1/dashboard/observability?project={worktree_id}&limit=10"
                    ))
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), 1_048_576)
            .await
            .expect("response body");
        let body = String::from_utf8(bytes.to_vec()).expect("UTF-8 JSON");
        assert!(!body.contains("workspace-with-secret-name"));
        assert!(!body.contains("secret-provider-session"));
        let payload: Value = serde_json::from_str(&body).expect("valid JSON");
        assert_eq!(payload["trace_spans"].as_array().map(Vec::len), Some(1));
        let lifecycle = payload["lifecycle_events"]
            .as_array()
            .expect("lifecycle events");
        assert!(
            lifecycle.len() >= 3,
            "global daemon lifecycle is intentionally included"
        );
        assert!(lifecycle.iter().any(|event| {
            event["engine"] == "icm" && event["detail_code"] == "bounded_backoff"
        }));
        assert!(payload["next_cursor"].is_number());

        let response = application
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/dashboard?project={worktree_id}"))
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("dashboard response");
        let bytes = to_bytes(response.into_body(), 1_048_576)
            .await
            .expect("dashboard body");
        let body = String::from_utf8(bytes.to_vec()).expect("UTF-8 dashboard JSON");
        assert!(!body.contains("workspace-with-secret-name"));
        assert!(!body.contains("secret-provider-session"));
    }

    #[tokio::test]
    async fn project_registry_page_is_bounded_at_thousand_workspace_scale() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let mut config = Config {
            data_dir: directory.path().join("data"),
            ..Config::default()
        };
        config.engines.auto_start_icm = false;
        config.engines.directory = Some(directory.path().join("missing-engines"));
        std::fs::create_dir_all(&config.data_dir).expect("data directory");
        config.data_dir = config
            .data_dir
            .canonicalize()
            .expect("canonical data directory");
        for index in 0..1_000_u64 {
            let repository_id = format!("{index:064x}");
            let worktree_id = format!("{:064x}", index + 1_000);
            let root = directory.path().join(format!("workspace-{index}"));
            std::fs::create_dir(&root).expect("workspace root");
            let registration_directory = config
                .data_dir
                .join("workspaces")
                .join(&repository_id)
                .join(&worktree_id);
            let index_directory = registration_directory.join("index/grepai");
            std::fs::create_dir_all(&registration_directory).expect("registration directory");
            let registration = WorkspaceRegistration {
                schema_version: WORKSPACE_REGISTRATION_SCHEMA_VERSION,
                root: root.canonicalize().expect("canonical root"),
                repository_id,
                worktree_id,
                git_backed: false,
                linked_worktree: false,
                index_directory,
                registered_at_ms: index,
                last_seen_at_ms: index,
            };
            std::fs::write(
                registration_directory.join("workspace.json"),
                serde_json::to_vec(&registration).expect("registration JSON"),
            )
            .expect("registration write");
        }
        let state = AppState::initialize(config)
            .await
            .expect("test state initializes");
        let token = AuthToken::new(TOKEN.to_owned()).expect("test token is valid");
        let response = router(state, token)
            .oneshot(
                Request::builder()
                    .uri("/v1/dashboard/projects?offset=100&limit=100")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), 1_048_576)
            .await
            .expect("response body");
        let payload: Value = serde_json::from_slice(&bytes).expect("valid JSON");
        assert_eq!(payload["total"], 1_000);
        assert_eq!(payload["projects"].as_array().map(Vec::len), Some(100));
        assert_eq!(payload["next_offset"], 200);
    }

    #[tokio::test]
    async fn test_dashboard_memory_topic_rejects_non_opaque_identifiers() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let response = test_router(&directory)
            .await
            .oneshot(
                Request::builder()
                    .uri("/v1/dashboard/memory/topics/release-project-token")
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let bytes = to_bytes(response.into_body(), 1_048_576)
            .await
            .expect("response body");
        let payload: Value = serde_json::from_slice(&bytes).expect("valid JSON response");
        assert_eq!(payload["code"], "invalid_request");
    }

    #[tokio::test]
    async fn dashboard_memory_topic_identity_cannot_cross_projects() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let mut config = Config {
            data_dir: directory.path().join("data"),
            ..Config::default()
        };
        config.engines.auto_start_icm = false;
        config.engines.directory = Some(directory.path().join("missing-engines"));
        let alpha_root = directory.path().join("alpha");
        let beta_root = directory.path().join("beta");
        let alpha_worktree = register_test_workspace(&config, &alpha_root).await;
        let beta_worktree = register_test_workspace(&config, &beta_root).await;
        let registrations = hzr_index::registered_workspaces(&config.data_dir).registrations;
        let alpha_repository = registrations
            .iter()
            .find(|registration| registration.worktree_id == alpha_worktree)
            .expect("alpha registration")
            .repository_id
            .clone();
        let alpha_canonical = alpha_root
            .canonicalize()
            .expect("canonical alpha workspace");
        let ledger = Ledger::open(&config.data_dir.join("ledger/hzr.sqlite")).expect("ledger");
        ledger
            .record_operation_attributed(
                "secret-dashboard-command --query private-needle",
                "rtk secret-dashboard-command",
                30,
                5,
                1,
                OperationAttribution {
                    project_path: alpha_canonical.to_str().expect("UTF-8 alpha workspace"),
                    agent: Some("secret-provider-agent"),
                    session_id: Some("secret-provider-session"),
                    channel: OperationChannel::HookCli,
                    measurement: OperationMeasurement::Estimated,
                    route: OperationRoute::Optimized,
                },
            )
            .expect("private operation fixture");
        drop(ledger);

        let state = AppState::initialize(config)
            .await
            .expect("test state initializes");
        let database = state.memory.layout().database.clone();
        std::fs::create_dir_all(database.parent().expect("memory database parent"))
            .expect("memory directory");
        let connection = Connection::open(&database).expect("memory fixture database");
        connection
            .execute_batch(
                "CREATE TABLE memories (
                    id TEXT PRIMARY KEY, created_at TEXT NOT NULL, updated_at TEXT NOT NULL,
                    last_accessed TEXT, access_count INTEGER NOT NULL, weight REAL NOT NULL,
                    topic TEXT NOT NULL, summary TEXT NOT NULL, raw_excerpt TEXT,
                    keywords TEXT NOT NULL, importance TEXT NOT NULL, source_type TEXT,
                    source_data TEXT, related_ids TEXT NOT NULL, summary_hash TEXT, embedding BLOB
                 );",
            )
            .expect("memory fixture schema");
        connection
            .execute(
                "INSERT INTO memories (
                    id, created_at, updated_at, last_accessed, access_count, weight, topic,
                    summary, raw_excerpt, keywords, importance, source_type, source_data,
                    related_ids, summary_hash, embedding
                 ) VALUES ('alpha-memory', '2026-08-25T00:00:00Z', '2026-08-25T00:00:00Z',
                           NULL, 1, 0.8, ?1, 'private alpha data', NULL, '[]', 'medium',
                           NULL, NULL, '[]', NULL, NULL)",
                params![format!("decisions-{alpha_repository}")],
            )
            .expect("alpha memory fixture");
        drop(connection);
        let snapshot = hzr_memory::read_project_snapshot(&database, &alpha_repository)
            .expect("alpha snapshot");
        let raw_topic_id = snapshot.topics.first().expect("alpha topic").id.clone();
        let public_topic_id = state.observability.topic_hash(&raw_topic_id);
        let token = AuthToken::new(TOKEN.to_owned()).expect("test token is valid");
        let application = router(state, token);

        let alpha = application
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/v1/dashboard/memory/topics/{public_topic_id}?project={alpha_worktree}"
                    ))
                    .body(Body::empty())
                    .expect("alpha request"),
            )
            .await
            .expect("alpha response");
        assert_eq!(alpha.status(), StatusCode::OK);
        let alpha_body = to_bytes(alpha.into_body(), 1_048_576)
            .await
            .expect("alpha body");
        let alpha_body = String::from_utf8(alpha_body.to_vec()).expect("UTF-8 alpha body");
        for secret in [
            alpha_repository.as_str(),
            "decisions-",
            "private alpha data",
            "alpha-memory",
        ] {
            assert!(!alpha_body.contains(secret), "public topic leaked {secret}");
        }

        let beta = application
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/v1/dashboard/memory/topics/{public_topic_id}?project={beta_worktree}"
                    ))
                    .body(Body::empty())
                    .expect("beta request"),
            )
            .await
            .expect("beta response");
        assert_eq!(beta.status(), StatusCode::NOT_FOUND);

        let authenticated_without_project = application
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/memory/topics/{raw_topic_id}"))
                    .header("authorization", format!("Bearer {TOKEN}"))
                    .body(Body::empty())
                    .expect("authenticated request without project"),
            )
            .await
            .expect("authenticated response without project");
        assert_eq!(
            authenticated_without_project.status(),
            StatusCode::BAD_REQUEST
        );

        let authenticated_alpha = application
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/v1/memory/topics/{raw_topic_id}?project={alpha_worktree}"
                    ))
                    .header("authorization", format!("Bearer {TOKEN}"))
                    .body(Body::empty())
                    .expect("authenticated alpha request"),
            )
            .await
            .expect("authenticated alpha response");
        assert_eq!(authenticated_alpha.status(), StatusCode::OK);

        let authenticated_beta = application
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/v1/memory/topics/{raw_topic_id}?project={beta_worktree}"
                    ))
                    .header("authorization", format!("Bearer {TOKEN}"))
                    .body(Body::empty())
                    .expect("authenticated beta request"),
            )
            .await
            .expect("authenticated beta response");
        assert_eq!(authenticated_beta.status(), StatusCode::NOT_FOUND);

        let dashboard = application
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/dashboard?project={alpha_worktree}"))
                    .body(Body::empty())
                    .expect("dashboard request"),
            )
            .await
            .expect("dashboard response");
        let dashboard_body = to_bytes(dashboard.into_body(), 1_048_576)
            .await
            .expect("dashboard body");
        let dashboard_body =
            String::from_utf8(dashboard_body.to_vec()).expect("UTF-8 dashboard body");
        for secret in [
            alpha_canonical.to_str().expect("UTF-8 alpha workspace"),
            alpha_repository.as_str(),
            "secret-dashboard-command",
            "private-needle",
            "secret-provider-agent",
            "secret-provider-session",
            "decisions-",
            "private alpha data",
            "alpha-memory",
        ] {
            assert!(
                !dashboard_body.contains(secret),
                "dashboard leaked {secret}"
            );
        }
    }

    #[tokio::test]
    async fn test_full_memory_topic_details_require_bearer_authentication() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let response = test_router(&directory)
            .await
            .oneshot(
                Request::builder()
                    .uri(format!("/v1/memory/topics/{}", "a".repeat(64)))
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_codec_route_preserves_exact_content() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let content = "run `cargo test` in ./crates/hzr-core";
        let body = serde_json::json!({
            "content": content,
            "fidelity": "exact",
            "risk": "low",
            "profile": "compact"
        });
        let response = test_router(&directory)
            .await
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/codec/compile")
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), 1_048_576)
            .await
            .expect("response body");
        let payload: Value = serde_json::from_slice(&bytes).expect("valid JSON response");
        assert_eq!(
            payload.get("content").and_then(Value::as_str),
            Some(content)
        );
        assert_eq!(payload.get("changed").and_then(Value::as_bool), Some(false));
    }

    #[tokio::test]
    async fn test_memory_routes_require_workspace_scope() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let response = test_router(&directory)
            .await
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/memory/recall")
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(r#"{"query":"architecture"}"#))
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn test_memory_route_rejects_user_supplied_project_override() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let body = serde_json::json!({
            "workspace": directory.path(),
            "query": "architecture",
            "project": "foreign-project"
        });
        let response = test_router(&directory)
            .await
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/memory/recall")
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn test_context_plan_reports_both_managed_services_when_degraded() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let body = serde_json::json!({
            "workspace": directory.path(),
            "intent": "unique_context_needle",
            "search_limit": 5,
            "memory_limit": 5
        });
        let response = test_router(&directory)
            .await
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/context/plan")
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), 1_048_576)
            .await
            .expect("response body");
        let payload: Value = serde_json::from_slice(&bytes).expect("valid JSON response");
        let pack = payload.get("pack").expect("context pack");
        let used = pack
            .get("used")
            .and_then(|value| value.get("value"))
            .and_then(Value::as_u64)
            .expect("used token estimate");
        let hard_limit = pack
            .get("hard_limit")
            .and_then(Value::as_u64)
            .expect("hard token limit");
        assert!(used <= hard_limit);
        assert!(
            pack.get("selected")
                .and_then(Value::as_array)
                .is_some_and(Vec::is_empty)
        );
        let warning_codes = payload
            .get("warnings")
            .and_then(Value::as_array)
            .expect("warnings")
            .iter()
            .filter_map(|warning| warning.get("code").and_then(Value::as_str))
            .collect::<Vec<_>>();
        assert!(warning_codes.contains(&"planner_unavailable"));
        assert!(warning_codes.contains(&"memory_unavailable"));
        assert!(std::fs::symlink_metadata(directory.path().join(".grepai")).is_err());
    }

    #[tokio::test]
    async fn test_invalid_search_is_rejected_before_engine_access() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let body = serde_json::json!({
            "workspace": directory.path(),
            "query": "",
            "limit": 10,
            "mode": "auto",
            "include_content": false
        });
        let response = test_router(&directory)
            .await
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/search")
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_empty_fork_invocation_is_rejected_before_engine_access() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let body = serde_json::json!({
            "cwd": directory.path(),
            "args": []
        });
        let response = test_router(&directory)
            .await
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/fork/run")
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_usage_route_records_provider_tokens_separately_from_estimates() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let workspace = directory.path().join("project");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let body = serde_json::json!({
            "trace_id": "019fb8bb-c468-7eb0-b133-b0b1239288c6",
            "provider": "test-provider",
            "model": "test-model",
            "usage": {
                "actual": {
                    "input_tokens": 120,
                    "output_tokens": 30,
                    "reasoning_tokens": null,
                    "cache_write_tokens": null,
                    "cache_read_tokens": 20
                },
                "estimated": {
                    "input_tokens": 900,
                    "output_tokens": null,
                    "method": "fixture"
                }
            },
            "turns": 2,
            "retries": 0,
            "latency_ms": 25,
            "outcome": "completed",
            "project_path": workspace
        });
        let response = test_router(&directory)
            .await
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/usage")
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::OK);
        let ledger = hzr_core::Ledger::open(&directory.path().join("data/ledger/hzr.sqlite"))
            .expect("ledger opens");
        let summary = ledger.summary().expect("ledger summary");
        assert_eq!(summary.actual_input_tokens, 120);
        assert_eq!(summary.actual_output_tokens, 30);
        assert_eq!(summary.estimated_input_tokens, 900);
        let canonical = std::fs::canonicalize(&workspace).expect("canonical workspace");
        let scoped = ledger
            .summary_for_project(&canonical.to_string_lossy())
            .expect("scoped summary");
        assert_eq!(scoped.actual_input_tokens, 120);
        assert_eq!(
            ledger
                .summary_for_project("/other")
                .expect("empty scope")
                .actual_input_tokens,
            0
        );
    }

    #[tokio::test]
    async fn test_unknown_approval_is_rejected_without_execution() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let body = serde_json::json!({
            "decision_id": "019fb8bb-c468-7eb0-b133-b0b1239288c6",
            "approved": true
        });
        let response = test_router(&directory)
            .await
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/exec/approval")
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .expect("request"),
            )
            .await
            .expect("response");

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn fidelity_approval_http_flow_records_exactly_once_and_replay_is_denied() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let (application, worktree_id) = test_router_with_workspace(&directory).await;
        let run = serde_json::json!({
            "cwd": directory.path().join("workspace"),
            "command": "printf '%s' --json",
            "fidelity_requested": true,
            "fidelity_reason": "machine_protocol",
            "agent": "test",
            "session_id": "http-fidelity-session"
        });
        let response = application
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/exec/run")
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(run.to_string()))
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::OK);
        let payload: Value = serde_json::from_slice(
            &to_bytes(response.into_body(), 1_048_576)
                .await
                .expect("response body"),
        )
        .expect("run response JSON");
        assert_eq!(payload["outcome"], "not_started");
        assert_eq!(payload["disposition"]["state"], "approval_required");
        let decision_id = payload["disposition"]["decision_id"]
            .as_str()
            .expect("approval decision id");
        let approval = serde_json::json!({
            "decision_id": decision_id,
            "approved": true
        });
        let approve = || {
            Request::builder()
                .method("POST")
                .uri("/v1/exec/approval")
                .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(approval.to_string()))
                .expect("request")
        };
        let response = application
            .clone()
            .oneshot(approve())
            .await
            .expect("approval response");
        assert_eq!(response.status(), StatusCode::OK);
        let approved: Value = serde_json::from_slice(
            &to_bytes(response.into_body(), 1_048_576)
                .await
                .expect("approval body"),
        )
        .expect("approval JSON");
        assert_eq!(approved["outcome"], "completed");

        let replay = application
            .clone()
            .oneshot(approve())
            .await
            .expect("replay response");
        assert_eq!(replay.status(), StatusCode::BAD_REQUEST);

        let ledger =
            Ledger::open(&directory.path().join("data/ledger/hzr.sqlite")).expect("ledger opens");
        let evasion = ledger
            .evasion_summary(hzr_core::StatsQuery::default())
            .expect("evasion stats");
        assert_eq!(evasion.fidelity_operations, 1);
        assert!(evasion.fidelity_delivered_tokens > 0);

        let observability = application
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/v1/dashboard/observability?project={worktree_id}&limit=100"
                    ))
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("observability response");
        assert_eq!(observability.status(), StatusCode::OK);
        let observability: Value = serde_json::from_slice(
            &to_bytes(observability.into_body(), 1_048_576)
                .await
                .expect("observability body"),
        )
        .expect("observability JSON");
        assert!(
            observability["trace_spans"]
                .as_array()
                .is_some_and(|spans| {
                    spans.iter().any(|span| {
                        span["route"] == "approval_continuation"
                            && span["state"] == "completed"
                            && span["error_code"].is_null()
                    })
                })
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn fidelity_public_route_enforces_five_operations_and_token_bound_before_spawn() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let (application, _) = test_router_with_workspace_and_fake_rtk(&directory).await;
        let workspace = directory.path().join("workspace");
        std::fs::write(workspace.join("artifact.bin"), [0_u8, 1, 2, 3]).expect("binary fixture");
        std::fs::write(workspace.join("oversized.bin"), vec![0_u8; 400_001])
            .expect("oversized fixture");
        let request = |command: &str, session_id: &str| {
            let body = serde_json::json!({
                "cwd": workspace,
                "command": command,
                "fidelity_requested": true,
                "fidelity_reason": "binary",
                "agent": "test",
                "session_id": session_id
            });
            Request::builder()
                .method("POST")
                .uri("/v1/exec/run")
                .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_string()))
                .expect("request")
        };

        for operation in 1..=5 {
            let response = application
                .clone()
                .oneshot(request("cat artifact.bin", "public-fidelity-budget"))
                .await
                .expect("fidelity response");
            assert_eq!(response.status(), StatusCode::OK, "operation {operation}");
            let payload: Value = serde_json::from_slice(
                &to_bytes(response.into_body(), 1_048_576)
                    .await
                    .expect("response body"),
            )
            .expect("response JSON");
            assert_eq!(
                payload["outcome"], "completed",
                "operation {operation}: {payload}"
            );
        }

        for (command, session) in [
            ("cat artifact.bin", "public-fidelity-budget"),
            ("cat oversized.bin", "public-fidelity-oversized"),
        ] {
            let response = application
                .clone()
                .oneshot(request(command, session))
                .await
                .expect("blocked fidelity response");
            assert_eq!(response.status(), StatusCode::OK);
            let payload: Value = serde_json::from_slice(
                &to_bytes(response.into_body(), 1_048_576)
                    .await
                    .expect("blocked response body"),
            )
            .expect("blocked response JSON");
            assert_eq!(payload["outcome"], "not_started");
            assert_eq!(payload["disposition"]["state"], "approval_required");
        }

        let ledger =
            Ledger::open(&directory.path().join("data/ledger/hzr.sqlite")).expect("ledger opens");
        let evasion = ledger
            .evasion_summary(hzr_core::StatsQuery::default())
            .expect("evasion stats");
        assert_eq!(evasion.fidelity_operations, 5);
        let workspace = std::fs::canonicalize(workspace).expect("canonical workspace");
        assert_eq!(
            ledger
                .evasion_summary(hzr_core::StatsQuery {
                    project_path: Some(&workspace.to_string_lossy()),
                    ..hzr_core::StatsQuery::default()
                })
                .expect("project evasion stats")
                .fidelity_operations,
            5
        );
    }

    #[tokio::test]
    async fn fidelity_approval_reports_non_retryable_accounting_failure() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let (application, worktree_id) = test_router_with_fidelity_failure(&directory).await;
        let run = serde_json::json!({
            "cwd": directory.path().join("workspace"),
            "command": "printf '%s' --json",
            "fidelity_requested": true,
            "fidelity_reason": "machine_protocol",
            "agent": "test",
            "session_id": "http-fidelity-failure"
        });
        let response = application
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/exec/run")
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(run.to_string()))
                    .expect("request"),
            )
            .await
            .expect("run response");
        let payload: Value = serde_json::from_slice(
            &to_bytes(response.into_body(), 1_048_576)
                .await
                .expect("run body"),
        )
        .expect("run JSON");
        let decision_id = payload["disposition"]["decision_id"]
            .as_str()
            .expect("approval id");
        let approval = serde_json::json!({"decision_id": decision_id, "approved": true});
        let response = application
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/exec/approval")
                    .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(approval.to_string()))
                    .expect("approval request"),
            )
            .await
            .expect("approval response");
        assert_eq!(response.status(), StatusCode::OK);
        let outcome: Value = serde_json::from_slice(
            &to_bytes(response.into_body(), 1_048_576)
                .await
                .expect("approval body"),
        )
        .expect("approval JSON");
        assert_eq!(outcome["outcome"], "executed_accounting_incomplete");
        assert_eq!(outcome["accounting"]["retryable"], false);
        assert_eq!(outcome["accounting"]["incident_persisted"], true);

        let observability = application
            .oneshot(
                Request::builder()
                    .uri(format!(
                        "/v1/dashboard/observability?project={worktree_id}&limit=100"
                    ))
                    .body(Body::empty())
                    .expect("observability request"),
            )
            .await
            .expect("observability response");
        let observability: Value = serde_json::from_slice(
            &to_bytes(observability.into_body(), 1_048_576)
                .await
                .expect("observability body"),
        )
        .expect("observability JSON");
        assert!(
            observability["trace_spans"]
                .as_array()
                .is_some_and(|spans| {
                    spans.iter().any(|span| {
                        span["stage"] == "ledger"
                            && span["state"] == "failed"
                            && span["error_code"] == "fidelity_accounting_failed"
                    })
                })
        );
    }
}
