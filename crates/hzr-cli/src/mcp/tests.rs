use hzr_core::Config;
use hzr_protocol::SearchMode;
use serde_json::{Value, json};

use super::{
    INVALID_REQUEST, LATEST_MCP_PROTOCOL_VERSION, METHOD_NOT_FOUND, PARSE_ERROR, SessionState,
    bounded_usize, handle_line, initialize_result, lifecycle_metadata, optional_enum, parse_mode,
    reject_unknown, tool_definitions, tool_error, tool_success,
};

/// Mirror of the notification rule in `handle_line`, which cannot be exercised
/// directly without a daemon: a request without `id` gets no response.
fn is_notification(request: &Value) -> bool {
    request.get("id").is_none()
}

#[test]
fn test_initialize_negotiates_latest_stable_for_unknown_revision() {
    let request = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "3000-01-01"}
    });
    let result = initialize_result(&request).expect("initialize succeeds");
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
    let result = initialize_result(&request).expect("initialize succeeds");
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
    assert!(initialize_result(&request).is_err());
}

#[test]
fn test_every_tool_has_model_guidance_and_typed_schemas() {
    let tools = tool_definitions();
    assert_eq!(tools.len(), 4);
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
        assert!(tool["annotations"]["readOnlyHint"].is_boolean());
    }
    assert!(
        tools.iter().any(|tool| tool["name"] == "hzr_context_plan"),
        "the MCP surface must expose HZR graph-first planning"
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
        "/repo",
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
