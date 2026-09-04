use hzr_core::Config;
use hzr_protocol::{
    AccountingChannel, AccountingOperationKind, AccountingOperationMode, AccountingRoute,
    AccountingSearchStrategy, AccountingStage, SearchFallbackCode, SearchMode,
};
use serde_json::{Value, json};

use crate::cli::McpClientArg;

use super::{
    INVALID_PARAMS, INVALID_REQUEST, LATEST_MCP_PROTOCOL_VERSION, METHOD_NOT_FOUND, PARSE_ERROR,
    SessionState, ToolKind, apply_workspace_policy, bounded_usize, cancelled_request_id,
    classify_workspace_binding, file_uri_path, fork_result, handle_line, initialize_result,
    initialize_workspace_root, lifecycle_metadata, mcp_operation_request, optional_enum,
    parse_mode, read_fork_request, registration_snippet, tool_definitions, tool_error,
    tool_success, validate_tool_arguments, write_fork_request,
};

#[test]
fn acceptance_gate_mcp_accounting_is_typed_private_and_stage_explicit() {
    let arguments = json!({
        "query": "secret search text",
        "path": "private/source",
        "mode": "auto",
        "limit": 7,
        "include_content": true
    });
    let response = json!({
        "query": "secret search text",
        "path": "private/source",
        "total_hits": 0,
        "shown_hits": 0,
        "scanned_files": 1,
        "skipped_large": 0,
        "skipped_binary": 0,
        "hits": [],
        "effective_mode": "exact",
        "strategy": "fork_rgai_builtin",
        "fallback_code": "semantic_index_unavailable",
        "fallback_reason": "private failure detail"
    });
    let request = mcp_operation_request(
        ToolKind::Search,
        "hzr_search",
        "/work",
        &arguments,
        &response,
    )
    .expect("accounting request");

    assert_eq!(request.channel, AccountingChannel::Mcp);
    assert_eq!(request.route, AccountingRoute::Optimized);
    assert_eq!(
        request.baseline_tokens_estimated, request.delivered_tokens_estimated,
        "retrieval is coverage, not claimed savings"
    );
    assert!(request.delivered_tokens_estimated > 0);
    let encoded = serde_json::to_value(&request).expect("accounting JSON");
    assert_eq!(encoded["agent"], "mcp");
    let attribution = request.attribution.expect("search attribution");
    assert_eq!(attribution.mode, AccountingOperationMode::SearchExact);
    assert_eq!(
        attribution.requested_mode,
        Some(AccountingOperationMode::SearchAuto)
    );
    assert_eq!(
        attribution.effective_mode,
        Some(AccountingOperationMode::SearchExact)
    );
    assert_eq!(attribution.stage, AccountingStage::FinalDelivery);
    assert_eq!(
        attribution.search_strategy,
        Some(AccountingSearchStrategy::ForkRgaiBuiltin)
    );
    assert_eq!(
        attribution.search_fallback_code,
        Some(SearchFallbackCode::SemanticIndexUnavailable)
    );
    assert_eq!(attribution.limit, Some(7));
    assert_eq!(attribution.path_scope_count, Some(1));
    let encoded = serde_json::to_string(&attribution).expect("attribution JSON");
    assert!(!encoded.contains("secret search text"));
    assert!(!encoded.contains("private/source"));

    let actual_backend_response = json!({
        "query": "secret search text",
        "path": "private/source",
        "total_hits": 0,
        "shown_hits": 0,
        "scanned_files": 1,
        "skipped_large": 0,
        "skipped_binary": 0,
        "hits": [],
        "effective_mode": "semantic",
        "strategy": "fork_rgai_ripgrep",
        "fallback_code": "grepai_unavailable"
    });
    let actual_backend = mcp_operation_request(
        ToolKind::Search,
        "hzr_search",
        "/work",
        &arguments,
        &actual_backend_response,
    )
    .expect("actual backend accounting request")
    .attribution
    .expect("actual backend attribution");
    assert_eq!(
        actual_backend.search_strategy,
        Some(AccountingSearchStrategy::ForkRgaiRipgrep)
    );
    assert_eq!(
        actual_backend.effective_mode,
        Some(AccountingOperationMode::SearchSemantic)
    );
    assert_eq!(
        actual_backend.search_fallback_code,
        Some(SearchFallbackCode::GrepaiUnavailable)
    );
}

#[test]
fn acceptance_gate_every_non_dedicated_mcp_tool_has_typed_accounting() {
    let cases = [
        (
            ToolKind::Read,
            "hzr_read",
            json!({"path": "private/source", "outline": true}),
            AccountingOperationKind::Read,
            AccountingOperationMode::ReadOutline,
            AccountingStage::FinalDelivery,
        ),
        (
            ToolKind::Write,
            "hzr_write",
            json!({}),
            AccountingOperationKind::Write,
            AccountingOperationMode::Write,
            AccountingStage::FinalDelivery,
        ),
        (
            ToolKind::ContextPlan,
            "hzr_context_plan",
            json!({}),
            AccountingOperationKind::Context,
            AccountingOperationMode::ContextPlan,
            AccountingStage::StandaloneDelivery,
        ),
        (
            ToolKind::MemoryRecall,
            "hzr_memory_recall",
            json!({}),
            AccountingOperationKind::Memory,
            AccountingOperationMode::MemoryRecall,
            AccountingStage::StandaloneDelivery,
        ),
        (
            ToolKind::MemoryStore,
            "hzr_memory_store",
            json!({}),
            AccountingOperationKind::Memory,
            AccountingOperationMode::MemoryStore,
            AccountingStage::StandaloneDelivery,
        ),
        (
            ToolKind::MemoryForget,
            "hzr_memory_forget",
            json!({}),
            AccountingOperationKind::Memory,
            AccountingOperationMode::MemoryForget,
            AccountingStage::StandaloneDelivery,
        ),
        (
            ToolKind::MemoryUpdate,
            "hzr_memory_update",
            json!({}),
            AccountingOperationKind::Memory,
            AccountingOperationMode::MemoryUpdate,
            AccountingStage::StandaloneDelivery,
        ),
        (
            ToolKind::MemoryPrune,
            "hzr_memory_prune",
            json!({}),
            AccountingOperationKind::Memory,
            AccountingOperationMode::MemoryPrune,
            AccountingStage::StandaloneDelivery,
        ),
        (
            ToolKind::Observability,
            "hzr_observability",
            json!({}),
            AccountingOperationKind::Observability,
            AccountingOperationMode::ObservabilitySnapshot,
            AccountingStage::ControlPlane,
        ),
        (
            ToolKind::Doctor,
            "hzr_doctor",
            json!({}),
            AccountingOperationKind::Doctor,
            AccountingOperationMode::DoctorCheck,
            AccountingStage::ControlPlane,
        ),
    ];
    for (kind, name, arguments, operation, mode, stage) in cases {
        let request = mcp_operation_request(kind, name, "/work", &arguments, &json!({"ok": true}))
            .expect("typed accounting request");
        let attribution = request.attribution.expect("typed attribution");
        assert_eq!(attribution.operation, operation, "{name}");
        assert_eq!(attribution.mode, mode, "{name}");
        assert_eq!(attribution.stage, stage, "{name}");
    }
}

/// Mirror of the notification rule in `handle_line`, which cannot be exercised
/// directly without a daemon: a request without `id` gets no response.
fn is_notification(request: &Value) -> bool {
    request.get("id").is_none()
}

/// A normally-bound workspace, for the tests whose subject is not the binding itself.
fn test_binding() -> super::WorkspaceBinding {
    classify_workspace_binding(std::path::Path::new("/Users/andrew/code/app"), None)
}

#[test]
fn test_cancellation_notification_identifies_only_valid_request_ids() {
    let valid = json!({
        "jsonrpc": "2.0",
        "method": "notifications/cancelled",
        "params": {"requestId": "call-7", "reason": "user stopped it"}
    });
    assert_eq!(cancelled_request_id(&valid), Some(json!("call-7")));
    assert_eq!(
        cancelled_request_id(&json!({"method": "notifications/cancelled"})),
        None
    );
}

#[test]
fn test_initialize_negotiates_latest_stable_for_unknown_revision() {
    let request = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "3000-01-01"}
    });
    let result = initialize_result(&request, &test_binding()).expect("initialize succeeds");
    assert_eq!(result["protocolVersion"], LATEST_MCP_PROTOCOL_VERSION);
    assert!(result["capabilities"]["tools"].is_object());
    assert_eq!(result["serverInfo"]["name"], "hzr");
    assert!(
        result["instructions"]
            .as_str()
            .expect("instructions")
            .contains("hzr_context_plan"),
        "clients must be told why not to call icm/grepai directly"
    );
}

#[test]
fn test_initialize_can_select_one_percent_encoded_client_root() {
    let request = json!({
        "params": {
            "roots": [{"uri": "file:///Users/andrew/My%20Project", "name": "workspace"}]
        }
    });

    assert_eq!(
        initialize_workspace_root(&request).expect("valid root"),
        Some(std::path::PathBuf::from("/Users/andrew/My Project"))
    );
    assert!(
        initialize_workspace_root(&json!({"params": {"roots": []}}))
            .expect("empty roots")
            .is_none()
    );
    assert!(file_uri_path("https://example.com/repo").is_err());
    assert!(
        initialize_workspace_root(&json!({
            "params": {"roots": [{"uri": "file:///one"}, {"uri": "file:///two"}]}
        }))
        .is_err()
    );
}

#[test]
fn test_initialize_preserves_a_supported_client_revision() {
    let request = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2024-11-05"}
    });
    let result = initialize_result(&request, &test_binding()).expect("initialize succeeds");
    assert_eq!(result["protocolVersion"], "2024-11-05");
}

#[test]
fn test_lifecycle_is_client_managed_and_never_started_by_init() {
    let lifecycle = lifecycle_metadata();

    assert_eq!(lifecycle["mode"], "client_managed_stdio");
    assert_eq!(lifecycle["started_by_init"], false);
    assert_eq!(lifecycle["registered_by"], "hzr install --force");
    assert_eq!(lifecycle["launched_by"], "MCP client on connection");
}

#[test]
fn test_initialize_requires_a_protocol_version() {
    let request = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}
    });
    assert!(initialize_result(&request, &test_binding()).is_err());
}

#[test]
fn test_every_tool_has_model_guidance_and_typed_schemas() {
    let tools = tool_definitions();
    assert_eq!(tools.len(), super::tools::tool_definitions().len());
    for tool in &tools {
        let name = tool["name"].as_str().expect("tool name");
        assert!(
            name.starts_with("hzr_"),
            "tools must be namespaced to HZR: {name}"
        );
        assert!(
            !tool["description"]
                .as_str()
                .expect("description")
                .is_empty()
        );
        assert!(!tool["title"].as_str().expect("title").is_empty());
        assert_eq!(tool["inputSchema"]["type"], "object");
        assert_eq!(tool["inputSchema"]["additionalProperties"], false);
        assert!(
            tool["inputSchema"]["required"].is_array(),
            "{name} must declare required arguments"
        );
        assert_eq!(tool["outputSchema"]["type"], "object");
        if tool["outputSchema"]["additionalProperties"] != false {
            let branches = tool["outputSchema"]["anyOf"]
                .as_array()
                .expect("strict output branches");
            for branch in branches {
                assert_eq!(branch["additionalProperties"], false);
            }
        }
        assert!(tool["annotations"]["readOnlyHint"].is_boolean());
    }
    assert!(
        tools.iter().any(|tool| tool["name"] == "hzr_context_plan"),
        "the MCP surface must expose HZR graph-first planning"
    );
}

fn representative_output(name: &str) -> Option<Value> {
    let memory = json!({
        "score": null,
        "id": "memory-1",
        "created_at": "2026-08-05T00:00:00Z",
        "updated_at": "2026-08-05T00:00:00Z",
        "last_accessed": "2026-08-05T00:00:00Z",
        "access_count": 0,
        "weight": 0.5,
        "topic": "architecture",
        "summary": "One durable fact",
        "raw_excerpt": null,
        "keywords": ["ledger"],
        "importance": "high",
        "source": {"type": "manual"},
        "related_ids": [],
        "scope": "project"
    });
    Some(match name {
        "hzr_memory_get" => {
            json!({"id": "memory-1", "topic": "architecture", "updated_at": "2026-09-04", "summary": "Fact", "raw_excerpt": null})
        }
        "hzr_memory_recall" => json!({"count": 1, "total_matches": 2, "memories": [memory]}),
        "hzr_memory_store" => json!({"transport": "stdio_mcp", "memory": memory}),
        "hzr_memory_forget" | "hzr_memory_update" | "hzr_memory_prune" => {
            json!({"affected_ids": ["memory-1"], "dry_run": false})
        }
        "hzr_search" => json!({
            "query": "Ledger",
            "path": "crates",
            "total_hits": 1,
            "shown_hits": 1,
            "scanned_files": 10,
            "skipped_large": 0,
            "skipped_binary": 0,
            "hits": [{
                "path": "crates/hzr-core/src/ledger.rs",
                "score": 1.0,
                "matched_lines": 1,
                "snippets": [{"lines": [{"line": 10, "text": "struct Ledger"}]}]
            }],
            "effective_mode": "exact",
            "strategy": "fork_rgai_builtin"
        }),
        "hzr_context_plan" => json!({
            "pack": {
                "selected": [{
                    "id": "candidate-1",
                    "source": "exact",
                    "content_ref": "sha256:abc",
                    "path": "src/lib.rs",
                    "symbol": null,
                    "symbol_unavailable_reason": "no_enclosing_symbol",
                    "line_start": 1,
                    "line_end": 2,
                    "source_rank": 1,
                    "relevance": 0.8,
                    "tokens": {"value": 4, "source": "estimate"},
                    "freshness": "generation-1",
                    "trust": "workspace:untrusted",
                    "provenance": {
                        "source": "fork-core/rgai-builtin",
                        "content_hash": "abc",
                        "generation": "generation-1",
                        "canonical_ref": "src/lib.rs#L1-L2",
                        "derived_by": null
                    }
                }],
                "rejected": [],
                "used": {"value": 4, "source": "estimate"},
                "hard_limit": 100,
                "coverage": 1.0,
                "confidence": 0.8,
                "budget_exceeded": false
            },
            "contents": {"sha256:abc": "struct Ledger"},
            "warnings": []
        }),
        "hzr_codec" => json!({
            "content": "unchanged",
            "changed": false,
            "profile": "adaptive",
            "protected_spans": [{"start": 0, "end": 4, "kind": "code"}],
            "coverage_state": "applied",
            "global_response_replacement_confirmed": false,
            "estimated_token_credit_eligible": false
        }),
        "hzr_read" => json!({
            "content": "line one\n",
            "stderr": "",
            "termination": "exited",
            "exit_code": 0,
            "signal": null,
            "duration_ms": 1,
            "stdout_sha256": "abc",
            "stderr_sha256": "def",
            "stdout_truncated": false,
            "stderr_truncated": false
        }),
        "hzr_write" => json!({
            "receipt": {"version": 1, "ok": true, "op": "patch", "applied": 1},
            "stderr": "",
            "termination": "exited",
            "exit_code": 0,
            "signal": null,
            "duration_ms": 1,
            "stdout_sha256": "abc",
            "stderr_sha256": "def",
            "stdout_truncated": false,
            "stderr_truncated": false
        }),
        "hzr_exec" => json!({"outcome": "not_started", "disposition": {}}),
        "hzr_observability" => json!({
            "protocol_version": 1,
            "hzr_version": "0.6.0",
            "state": "ready",
            "workspace_root": "/work",
            "engines": [{
                "name": "grepai", "version": null, "state": "ready", "detail": null
            }],
            "capabilities": ["search"]
        }),
        "hzr_doctor" => json!({
            "hzr_version": "0.6.0",
            "config_path": "/config.toml",
            "data_dir": "/data",
            "workspace": "/work",
            "healthy": true,
            "checks": [{"name": "binding", "status": "pass", "detail": "exact"}],
            "client_workspace_bindings": [],
            "response_codec_coverage": []
        }),
        _ => return None,
    })
}

#[test]
fn test_representative_structured_content_conforms_to_every_output_schema() {
    for tool in tool_definitions() {
        let name = tool["name"].as_str().expect("tool name");
        let output = representative_output(name);
        assert!(output.is_some(), "missing representative output for {name}");
        let result = super::tools::validate_tool_output(
            name,
            output.as_ref().expect("representative output"),
        );
        assert!(
            result.is_ok(),
            "{name}: {}",
            result.expect_err("schema error")
        );
    }
}

/// The density codec was reachable only through `hzr codec compile`, so no agent ever
/// used it: nothing in the hook path, the planner or the MCP surface called it. A
/// capability the control plane advertises but never exposes is dead weight.
#[test]
fn test_the_density_codec_is_reachable_over_mcp() {
    let tools = tool_definitions();
    let codec = tools
        .iter()
        .find(|tool| tool["name"] == "hzr_codec")
        .expect("the MCP surface must expose the density codec");

    assert_eq!(
        codec["inputSchema"]["properties"]["content"]["type"],
        "string"
    );
    assert_eq!(codec["inputSchema"]["required"][0], "content");
    assert!(
        codec["inputSchema"]["properties"]["profile"]["enum"]
            .as_array()
            .is_some_and(|profiles| profiles.iter().any(|profile| profile == "shadow")),
        "shadow profile must be selectable so a counterfactual can be measured without changing output"
    );
    assert_eq!(codec["annotations"]["readOnlyHint"], true);
    assert!(
        codec["outputSchema"]["properties"]["counterfactual"].is_object(),
        "the codec must report what it would have saved, otherwise it earns no ledger credit"
    );
    assert!(
        codec["outputSchema"]["properties"]["coverage_state"].is_object(),
        "clients must distinguish applied tool output from instructed global response coverage"
    );
    assert!(
        codec["description"]
            .as_str()
            .is_some_and(|description| description.contains("never earns provider-billed credit")),
        "the tool must not imply that a returned transform replaced the final assistant response"
    );
}

#[test]
fn test_tools_expose_no_direct_engine_access() {
    let encoded = serde_json::to_string(&tool_definitions()).expect("serialize");
    for forbidden in ["icm serve", "grepai watch", "rtk proxy"] {
        assert!(
            !encoded.contains(forbidden),
            "the MCP surface must not offer direct engine control: {forbidden}"
        );
    }
}

#[test]
fn test_notifications_are_never_answered() {
    assert!(is_notification(
        &json!({"jsonrpc": "2.0", "method": "notifications/initialized"})
    ));
    assert!(!is_notification(
        &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"})
    ));
}

#[tokio::test]
async fn test_tools_are_rejected_before_initialization() {
    let mut session = SessionState::default();
    let mut binding = classify_workspace_binding(std::path::Path::new("/repo"), None);
    let response = handle_line(
        &Config::default(),
        &mut binding,
        true,
        &mut session,
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
    )
    .await
    .expect("request receives a response");
    assert_eq!(response["error"]["code"], INVALID_REQUEST);
}

#[tokio::test]
async fn pinned_workspace_conflict_rejects_initialization_before_tools_are_exposed() {
    let mut session = SessionState::default();
    let mut binding = classify_workspace_binding(std::path::Path::new("/configured-project"), None);
    let response = handle_line(
        &Config::default(),
        &mut binding,
        true,
        &mut session,
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","roots":[{"uri":"file:///current-project"}]}}"#,
    )
    .await
    .expect("request receives a response");

    assert_eq!(response["error"]["code"], INVALID_PARAMS);
    assert!(
        response["error"]["message"]
            .as_str()
            .is_some_and(|message| message.contains("conflicts with client root"))
    );
    assert!(!session.initialized);
}

#[test]
fn test_invalid_tool_arguments_are_not_silently_defaulted() {
    assert!(
        optional_enum(
            &json!({"mode": "typo"}),
            "mode",
            SearchMode::Auto,
            parse_mode,
            "auto, semantic, exact",
        )
        .is_err()
    );
    assert!(bounded_usize(&json!({"limit": 51}), "limit", 10, 50).is_err());
    assert!(
        validate_tool_arguments("hzr_search", &json!({"query": "x", "workspace": "/other"}))
            .is_err()
    );
}

#[test]
fn test_unavailable_backend_reports_an_error_not_a_fake_success() {
    let payload = tool_error("HZR daemon is unavailable; nothing was written.");
    assert_eq!(payload["isError"], true);
    assert!(
        payload["content"][0]["text"]
            .as_str()
            .expect("text")
            .contains("nothing was written"),
        "a dead backend must never look like a successful store"
    );
    let result = tool_success(&json!({"ok": true}));
    assert_eq!(result["isError"], false);
    assert_eq!(result["structuredContent"]["ok"], true);
    assert_eq!(result["content"][0]["text"], r#"{"ok":true}"#);
}

fn fork_response(
    stdout: &str,
    stderr: &str,
    termination: hzr_protocol::CommandTermination,
    exit_code: Option<i32>,
) -> hzr_protocol::ForkRunApiResponse {
    hzr_protocol::ForkRunApiResponse {
        stdout: stdout.to_owned(),
        stderr: stderr.to_owned(),
        termination,
        exit_code,
        signal: None,
        duration_ms: 1,
        stdout_sha256: "a".repeat(64),
        stderr_sha256: "b".repeat(64),
        stdout_truncated: false,
        stderr_truncated: false,
    }
}

#[test]
fn mcp_fork_result_never_turns_failed_read_or_write_into_success() {
    let missing = fork_response(
        "",
        "missing file",
        hzr_protocol::CommandTermination::Exited,
        Some(2),
    );
    let error = fork_result(missing, "content", false).expect_err("missing read must fail");
    assert!(error.to_string().contains("exit code Some(2)"));
    assert!(error.to_string().contains("missing file"));

    let timed_out = fork_response(
        "{\"ok\":true}",
        "",
        hzr_protocol::CommandTermination::TimedOut,
        None,
    );
    assert!(fork_result(timed_out, "receipt", true).is_err());

    let rejected = fork_response(
        "{\"ok\":false,\"error\":\"CAS conflict\"}",
        "",
        hzr_protocol::CommandTermination::Exited,
        Some(0),
    );
    assert!(fork_result(rejected, "receipt", true).is_err());

    let success = fork_response(
        "{\"ok\":true,\"op\":\"patch\"}",
        "",
        hzr_protocol::CommandTermination::Exited,
        Some(0),
    );
    let receipt = fork_result(success, "receipt", true).expect("successful write receipt");
    assert_eq!(receipt["receipt"]["ok"], true);
}

#[test]
fn mcp_read_and_write_build_only_confined_typed_fork_requests() {
    let read = read_fork_request("/work", &json!({"path": "src/lib.rs", "from": 2, "to": 4}))
        .expect("read request");
    assert_eq!(read.cwd, "/work");
    assert_eq!(
        read.args,
        ["read", "src/lib.rs", "--from", "2", "--to", "4"]
    );
    assert!(read.managed_write.is_none());
    assert!(read_fork_request("/work", &json!({"path": "x", "from": 5, "to": 4})).is_err());

    let old = "private old block";
    let new = "private new block";
    let patch = write_fork_request(
        "/work",
        &json!({
            "operation": "patch", "path": "src/lib.rs", "old": old, "new": new, "cas": true
        }),
    )
    .expect("patch request");
    assert!(patch.args.is_empty());
    assert!(patch.stdin.is_none());
    assert_eq!(
        patch.managed_write,
        Some(hzr_protocol::ForkManagedWrite::Patch {
            path: "src/lib.rs".into(),
            old: old.into(),
            new: new.into(),
        })
    );
    assert!(
        write_fork_request(
            "/work",
            &json!({"operation": "patch", "path": "x", "old": "a", "new": "b", "cas": false})
        )
        .is_err()
    );
    assert!(
        write_fork_request(
            "/work",
            &json!({"operation": "create", "path": "x", "old": "a", "content": "b"})
        )
        .is_err()
    );
}

#[test]
fn mcp_exact_content_accepts_deletion_empty_files_and_whitespace() {
    for content in ["", "  ", "\\n", "λ"] {
        let create = json!({"operation": "create", "path": "file", "content": content});
        assert!(super::validate_tool_input("hzr_write", &create).is_ok());
        assert!(write_fork_request("/work", &create).is_ok());
        let patch = json!({"operation": "patch", "path": "file", "old": " ", "new": content});
        assert!(super::validate_tool_input("hzr_write", &patch).is_ok());
        assert!(write_fork_request("/work", &patch).is_ok());
    }
    let empty_old = json!({"operation": "patch", "path": "file", "old": "", "new": "x"});
    assert!(super::validate_tool_input("hzr_write", &empty_old).is_err());
    assert!(write_fork_request("/work", &empty_old).is_err());
    let unicode = json!({"operation": "create", "path": "file", "content": "λ".repeat(super::MCP_CREATE_CONTENT_MAX_BYTES / 2 + 1)});
    assert!(super::validate_tool_input("hzr_write", &unicode).is_err());
    assert!(write_fork_request("/work", &unicode).is_err());
    for value in [0, 100_001] {
        let read = json!({"path": "file", "max_lines": value});
        assert!(super::validate_tool_input("hzr_read", &read).is_err());
        assert!(read_fork_request("/work", &read).is_err());
    }
}

#[test]
fn test_error_codes_are_standard_json_rpc() {
    assert_eq!(METHOD_NOT_FOUND, -32601);
    assert_eq!(PARSE_ERROR, -32700);
}

/// A client that launches `hzr mcp serve` from its own working directory decides the
/// project namespace by accident. Claude Desktop launches from `/`, so every memory it
/// stored landed in the namespace of the filesystem root instead of the repository the
/// user was discussing — and a CLI recall from inside that repository could never see it.
/// The binding must be classified before it is used, so the unusable cases are named
/// rather than silently hashed into a namespace nobody reads.
#[test]
fn test_a_client_launch_directory_that_cannot_be_a_project_is_refused() {
    let home = std::path::Path::new("/Users/andrew");

    for unusable in [
        std::path::Path::new("/"),
        home,
        std::path::Path::new("/Users"),
    ] {
        let binding = classify_workspace_binding(unusable, Some(home));
        assert!(
            binding.project_root().is_none(),
            "{} must never own a project namespace",
            unusable.display()
        );
        let reason = binding.refusal().expect("a refusal explains itself");
        assert!(
            reason.contains("--workspace"),
            "the refusal must name the flag that fixes it, got: {reason}"
        );
    }
}

/// An agent cannot reason about which project its memory belongs to unless it is told.
/// The handshake is the only place every client reads, so the resolved binding — and a
/// refusal — must be stated there rather than inferred from a namespace hash.
#[test]
fn test_initialize_states_the_resolved_workspace_binding() {
    let request = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2025-11-25"}
    });

    let bound = classify_workspace_binding(std::path::Path::new("/Users/andrew/code/app"), None);
    let result = initialize_result(&request, &bound).expect("initialize succeeds");
    assert_eq!(
        result["serverInfo"]["workspace"]["project"],
        "/Users/andrew/code/app"
    );
    assert_eq!(result["serverInfo"]["workspace"]["bound"], true);

    let refused = classify_workspace_binding(
        std::path::Path::new("/"),
        Some(std::path::Path::new("/Users/andrew")),
    );
    let result = initialize_result(&request, &refused).expect("initialize still succeeds");
    assert_eq!(result["serverInfo"]["workspace"]["bound"], false);
    assert!(
        result["instructions"]
            .as_str()
            .expect("instructions")
            .contains("--workspace"),
        "a refused session must say how to fix itself in the text every client reads"
    );
}

#[tokio::test]
async fn test_initialize_rebinds_an_unpinned_server_to_the_client_root() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let project = directory.path().join("project");
    std::fs::create_dir(&project).expect("project directory");
    let config = Config {
        data_dir: directory.path().join("data"),
        ..Config::default()
    };
    config.ensure_layout().expect("data layout");
    let workspace = crate::activation::discover(&config, &project)
        .await
        .expect("workspace identity");
    workspace
        .ensure_managed_location()
        .expect("managed workspace");

    let mut binding = classify_workspace_binding(std::path::Path::new("/"), None);
    let mut session = SessionState::default();
    let request = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-11-25",
            "roots": [{"uri": format!("file://{}", project.display())}]
        }
    });
    let response = handle_line(
        &config,
        &mut binding,
        false,
        &mut session,
        &request.to_string(),
    )
    .await
    .expect("initialize response");

    assert_eq!(response["result"]["serverInfo"]["workspace"]["bound"], true);
    let canonical_project = std::fs::canonicalize(&project).expect("canonical project");
    assert_eq!(
        response["result"]["serverInfo"]["workspace"]["project"],
        canonical_project.to_string_lossy().as_ref()
    );
}

/// The registration snippet is what a user actually pastes, so it is the only place that
/// can prevent the bad binding rather than diagnose it afterwards. Claude Desktop launches
/// from `/` and can never bind by cwd, so its snippet must carry `--workspace`.
#[test]
fn test_the_registration_snippet_pins_the_workspace() {
    let binary = std::path::Path::new("/Users/andrew/.local/bin/hzr");
    let project = std::path::Path::new("/Users/andrew/code/app");

    for client in [
        McpClientArg::Codex,
        McpClientArg::ClaudeDesktop,
        McpClientArg::ClaudeCode,
    ] {
        let pinned = registration_snippet(client, binary, Some(project));
        assert!(
            pinned.contains("--workspace") && pinned.contains("/Users/andrew/code/app"),
            "a pinned snippet must carry the workspace: {pinned}"
        );
    }

    // Without an explicit workspace the snippet must still warn, because a Claude Desktop
    // registration that relies on cwd silently binds the filesystem root.
    let unpinned = registration_snippet(McpClientArg::ClaudeDesktop, binary, None);
    assert!(
        unpinned.contains("--workspace"),
        "an unpinned desktop snippet must still name the flag it needs: {unpinned}"
    );
    let claude_code = registration_snippet(McpClientArg::ClaudeCode, binary, Some(project));
    assert!(claude_code.contains(".mcp.json"));
    assert!(claude_code.contains("-s project"));
}

#[test]
fn acceptance_gate_registration_snippets_escape_hostile_paths() {
    let binary_text = "/tmp/hzr\"\\\n[mcp_servers.injected]";
    let workspace_text = "/tmp/project\"\\\ncommand = 'injected'";
    let binary = std::path::Path::new(binary_text);
    let workspace = std::path::Path::new(workspace_text);

    let codex = registration_snippet(McpClientArg::Codex, binary, Some(workspace));
    let document = codex
        .parse::<toml_edit::DocumentMut>()
        .expect("hostile paths remain valid TOML");
    let registration = &document["mcp_servers"]["hzr"];
    assert_eq!(registration["command"].as_str(), Some(binary_text));
    let args = registration["args"].as_array().expect("TOML args");
    assert_eq!(args.len(), 4);
    assert_eq!(
        args.get(3).and_then(toml_edit::Value::as_str),
        Some(workspace_text)
    );
    assert!(document["mcp_servers"].get("injected").is_none());

    for client in [McpClientArg::ClaudeDesktop, McpClientArg::ClaudeCode] {
        let snippet = registration_snippet(client, binary, Some(workspace));
        let json_start = snippet.find('{').expect("JSON object");
        let document: Value =
            serde_json::from_str(&snippet[json_start..]).expect("hostile paths remain valid JSON");
        let registration = &document["mcpServers"]["hzr"];
        assert_eq!(registration["command"], binary_text);
        assert_eq!(registration["args"][3], workspace_text);
        assert!(document["mcpServers"].get("injected").is_none());
    }
}

/// The converse: a real repository path must still bind, including one that has not been
/// `git init`-ed yet, because HZR supports those projects everywhere else.
#[test]
fn test_a_real_project_directory_still_binds() {
    let home = std::path::Path::new("/Users/andrew");
    let binding =
        classify_workspace_binding(std::path::Path::new("/Users/andrew/code/app"), Some(home));
    assert_eq!(
        binding.project_root(),
        Some(std::path::Path::new("/Users/andrew/code/app"))
    );
    assert!(binding.refusal().is_none());
}

#[tokio::test]
async fn test_mcp_refuses_an_uninitialized_or_unselected_workspace() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let project = directory.path().join("project");
    std::fs::create_dir_all(&project).expect("project directory");
    let mut config = Config {
        data_dir: directory.path().join("data"),
        ..Config::default()
    };
    config.ensure_layout().expect("data layout");

    let binding = apply_workspace_policy(&config, classify_workspace_binding(&project, None)).await;
    assert!(
        binding
            .refusal()
            .expect("uninitialized workspace refusal")
            .contains("hzr init")
    );

    let workspace = crate::activation::discover(&config, &project)
        .await
        .expect("workspace identity");
    workspace
        .ensure_managed_location()
        .expect("managed workspace");
    config.activation.mode = hzr_core::ActivationMode::Selected;

    let binding = apply_workspace_policy(&config, classify_workspace_binding(&project, None)).await;
    assert!(
        binding
            .refusal()
            .expect("unselected workspace refusal")
            .contains("hzr enable")
    );
}
