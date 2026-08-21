use hzr_core::Config;
use hzr_protocol::{
    AccountingChannel, AccountingOperationMode, AccountingRoute, AccountingSearchStrategy,
    AccountingStage, SearchFallbackCode, SearchMode,
};
use serde_json::{Value, json};

use crate::cli::McpClientArg;

use super::{
    INVALID_REQUEST, LATEST_MCP_PROTOCOL_VERSION, METHOD_NOT_FOUND, PARSE_ERROR, SessionState,
    apply_workspace_policy, bounded_usize, cancelled_request_id, classify_workspace_binding,
    handle_line, initialize_result, lifecycle_metadata, mcp_operation_request, optional_enum,
    parse_mode, registration_snippet, reject_unknown, tool_definitions, tool_error, tool_success,
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
    let request = mcp_operation_request("hzr_search", "/work", &arguments, &response)
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
    let actual_backend =
        mcp_operation_request("hzr_search", "/work", &arguments, &actual_backend_response)
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
    assert_eq!(tools.len(), 8);
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
        assert_eq!(tool["outputSchema"]["additionalProperties"], false);
        assert!(tool["annotations"]["readOnlyHint"].is_boolean());
    }
    assert!(
        tools.iter().any(|tool| tool["name"] == "hzr_context_plan"),
        "the MCP surface must expose HZR graph-first planning"
    );
}

fn schema_accepts(schema: &Value, value: &Value, path: &str) -> Result<(), String> {
    if let Some(options) = schema.get("anyOf").and_then(Value::as_array) {
        if options
            .iter()
            .any(|option| schema_accepts(option, value, path).is_ok())
        {
            return Ok(());
        }
        return Err(format!("{path} matched no anyOf branch"));
    }
    if let Some(expected) = schema.get("const") {
        if expected != value {
            return Err(format!("{path} expected constant {expected}, got {value}"));
        }
    }
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        if !values.contains(value) {
            return Err(format!("{path} is outside enum: {value}"));
        }
    }
    if let Some(kind) = schema.get("type") {
        let kinds = kind
            .as_array()
            .cloned()
            .unwrap_or_else(|| vec![kind.clone()]);
        let matches = kinds.iter().any(|kind| match kind.as_str() {
            Some("object") => value.is_object(),
            Some("array") => value.is_array(),
            Some("string") => value.is_string(),
            Some("integer") => value.as_i64().is_some() || value.as_u64().is_some(),
            Some("number") => value.is_number(),
            Some("boolean") => value.is_boolean(),
            Some("null") => value.is_null(),
            _ => false,
        });
        if !matches {
            return Err(format!("{path} has {value}, expected type {kind}"));
        }
    }
    if let Some(object) = value.as_object() {
        let properties = schema.get("properties").and_then(Value::as_object);
        if let Some(required) = schema.get("required").and_then(Value::as_array) {
            for key in required.iter().filter_map(Value::as_str) {
                if !object.contains_key(key) {
                    return Err(format!("{path} is missing required property {key}"));
                }
            }
        }
        for (key, child) in object {
            if let Some(child_schema) = properties.and_then(|items| items.get(key)) {
                schema_accepts(child_schema, child, &format!("{path}.{key}"))?;
            } else if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
                return Err(format!("{path} emitted undeclared property {key}"));
            } else if let Some(additional) = schema
                .get("additionalProperties")
                .filter(|value| value.is_object())
            {
                schema_accepts(additional, child, &format!("{path}.{key}"))?;
            }
        }
    }
    if let (Some(items), Some(values)) = (schema.get("items"), value.as_array()) {
        for (index, item) in values.iter().enumerate() {
            schema_accepts(items, item, &format!("{path}[{index}]"))?;
        }
    }
    Ok(())
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
            "protected_spans": [{"start": 0, "end": 4, "kind": "code"}]
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
        let result = schema_accepts(
            &tool["outputSchema"],
            output.as_ref().expect("representative output"),
            name,
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
    let response = handle_line(
        &Config::default(),
        &classify_workspace_binding(std::path::Path::new("/repo"), None),
        &mut session,
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
    )
    .await
    .expect("request receives a response");
    assert_eq!(response["error"]["code"], INVALID_REQUEST);
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
    assert!(reject_unknown(&json!({"query": "x", "workspace": "/other"}), &["query"]).is_err());
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

/// The registration snippet is what a user actually pastes, so it is the only place that
/// can prevent the bad binding rather than diagnose it afterwards. Claude Desktop launches
/// from `/` and can never bind by cwd, so its snippet must carry `--workspace`.
#[test]
fn test_the_registration_snippet_pins_the_workspace() {
    let binary = std::path::Path::new("/Users/andrew/.local/bin/hzr");
    let project = std::path::Path::new("/Users/andrew/code/app");

    for client in [McpClientArg::Codex, McpClientArg::ClaudeDesktop] {
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
