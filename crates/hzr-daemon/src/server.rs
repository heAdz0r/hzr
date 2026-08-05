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
use crate::visualizer;
use crate::{AppState, DaemonError};

pub fn router(state: AppState, token: AuthToken) -> Router {
    let timeout = std::time::Duration::from_millis(state.config.daemon.request_timeout_ms);
    let limit = state.config.daemon.request_limit_bytes;

    let authenticated = Router::new()
        .route("/v1/health", get(api::health))
        .route("/v1/engines", get(api::engines))
        .route("/v1/search", post(api::search))
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
        .route("/v1/fork/run", post(api::fork_run))
        .route("/v1/codec/compile", post(api::codec_compile))
        .route("/v1/usage", post(api::usage))
        .route("/v1/operations", post(api::operation))
        .route_layer(middleware::from_fn_with_state(token, authorize));
    let public = Router::new()
        .route("/v1/dashboard", get(api::dashboard))
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
    let memory = state.memory.clone();
    let context = state.context.clone();
    let serve_result = axum::serve(listener, router(state, token))
        .with_graceful_shutdown(shutdown)
        .await
        .map_err(DaemonError::Io);
    let (memory_stop, context_stop) = tokio::join!(memory.stop(), context.shutdown());
    serve_result?;
    memory_stop.map_err(DaemonError::Memory)?;
    context_stop.map_err(DaemonError::Context)
}

#[cfg(test)]
mod tests {
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode, header};
    use hzr_core::Config;
    use serde_json::Value;
    use tempfile::TempDir;
    use tower::ServiceExt;

    use super::router;
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
        assert!(payload.get("token").is_none());
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
            "outcome": "completed"
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
        let summary = hzr_core::Ledger::open(&directory.path().join("data/ledger/hzr.sqlite"))
            .expect("ledger opens")
            .summary()
            .expect("ledger summary");
        assert_eq!(summary.actual_input_tokens, 120);
        assert_eq!(summary.actual_output_tokens, 30);
        assert_eq!(summary.estimated_input_tokens, 900);
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
}
