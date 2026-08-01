#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use hzr_agent::{
    BearerToken, CAVEMAN_CODE_NPM_VERSION, HzrApi, IntegrationLayout, ManagedAgent,
    ManagedAgentConfig, ResponseFormat,
};
use tempfile::TempDir;

const BUNDLED_BRIDGE: &[u8] = include_bytes!("../../../integrations/caveman-code/bridge.mjs");
const BUNDLED_PACKAGE_LOCK: &[u8] =
    include_bytes!("../../../integrations/caveman-code/package-lock.json");

fn process_fixture_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

#[tokio::test]
async fn test_managed_agent_runs_pinned_local_bridge_and_captures_jsonl() {
    let _fixture = process_fixture_lock().lock().await;
    let temp = TempDir::new().expect("temporary directory");
    let (integration, workspace) = prepare_integration(&temp);
    let fake_node = write_fake_node(
        &temp,
        r#"IFS= read -r request
request_id=$(printf '%s' "$request" | sed -n 's/.*"request_id":"\([^"]*\)".*/\1/p')
printf '{"seq":0,"request_id":"%s","kind":"ready","data":{}}\n' "$request_id"
printf '{"seq":1,"request_id":"%s","kind":"result","data":{"text":"ok"}}\n' "$request_id"
"#,
    );
    let mut config = managed_config(&temp, fake_node, integration, workspace);
    config.timeout = Duration::from_secs(5);
    config.max_capture_bytes = 64 * 1024;

    let run = ManagedAgent::new(config)
        .run("return ok", ResponseFormat::Text, 1)
        .await
        .expect("fake bridge run succeeds");

    assert_eq!(run.text, "ok");
    assert_eq!(run.events.len(), 2);
    assert!(run.json.is_none());
    assert!(!run.request_id.is_empty());
}

#[tokio::test]
async fn test_managed_agent_timeout_covers_stdin_and_terminates_descendants() {
    let _fixture = process_fixture_lock().lock().await;
    let temp = TempDir::new().expect("temporary directory");
    let (integration, workspace) = prepare_integration(&temp);
    let fake_node = write_fake_node(
        &temp,
        r#"trap '' TERM
(
  trap 'printf terminated > "$HZR_AGENT_DIR/descendant-terminated"; exit 0' TERM
  printf ready > "$HZR_AGENT_DIR/descendant-ready"
  while :; do sleep 1; done
) &
while [ ! -f "$HZR_AGENT_DIR/descendant-ready" ]; do sleep 0.01; done
sleep 30
"#,
    );
    let mut config = managed_config(&temp, fake_node, integration, workspace);
    // Leave enough scheduling headroom for a loaded workspace-wide test run while
    // remaining far below the fixture's 30-second sleep.
    config.timeout = Duration::from_millis(500);
    let prompt = "x".repeat(2 * 1024 * 1024);

    let error = ManagedAgent::new(config)
        .run(&prompt, ResponseFormat::Text, 1)
        .await
        .expect_err("bridge must time out");

    assert!(
        matches!(error, hzr_agent::RunError::Timeout),
        "expected timeout, received {error:?}"
    );
    assert_eq!(
        fs::read_to_string(temp.path().join("agent-data/descendant-terminated"))
            .expect("descendant handled group termination"),
        "terminated"
    );
}

#[tokio::test]
async fn test_dropping_managed_agent_future_terminates_descendant_processes() {
    let _fixture = process_fixture_lock().lock().await;
    let temp = TempDir::new().expect("temporary directory");
    let (integration, workspace) = prepare_integration(&temp);
    let fake_node = write_fake_node(
        &temp,
        r#"(
  while :; do
    printf x >> "$HZR_AGENT_DIR/heartbeat"
    sleep 0.05
  done
) </dev/null >/dev/null 2>&1 &
printf '%s' "$!" > "$HZR_AGENT_DIR/descendant-pid"
while :; do sleep 30; done
"#,
    );
    let config = managed_config(&temp, fake_node, integration, workspace);
    let agent_data = temp.path().join("agent-data");
    let run = tokio::spawn(async move {
        ManagedAgent::new(config)
            .run("wait", ResponseFormat::Text, 1)
            .await
    });

    wait_for_file(&agent_data.join("descendant-pid")).await;
    wait_for_file(&agent_data.join("heartbeat")).await;
    run.abort();
    let join = run.await.expect_err("run task must be cancelled");
    assert!(join.is_cancelled());

    tokio::time::sleep(Duration::from_millis(200)).await;
    let first = fs::metadata(agent_data.join("heartbeat"))
        .expect("heartbeat metadata")
        .len();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let second = fs::metadata(agent_data.join("heartbeat"))
        .expect("heartbeat metadata")
        .len();

    let pid = fs::read_to_string(agent_data.join("descendant-pid"))
        .expect("descendant pid")
        .parse::<i32>()
        .expect("numeric descendant pid");
    let _ = nix::sys::signal::kill(
        nix::unistd::Pid::from_raw(pid),
        nix::sys::signal::Signal::SIGKILL,
    );
    assert_eq!(first, second, "descendant continued after run cancellation");
}

#[test]
fn test_bundled_layout_points_at_versioned_integration() {
    let root = IntegrationLayout::bundled().root().to_path_buf();
    assert!(root.ends_with(PathBuf::from("integrations/caveman-code")));
}

fn prepare_integration(temp: &TempDir) -> (IntegrationLayout, PathBuf) {
    let integration = temp.path().join("integration");
    let package_dir = integration.join("node_modules/@juliusbrussee/caveman-code");
    let workspace = temp.path().join("workspace");
    fs::create_dir_all(&package_dir).expect("package directory");
    fs::create_dir_all(&workspace).expect("workspace directory");
    fs::write(integration.join("bridge.mjs"), BUNDLED_BRIDGE).expect("bridge fixture");
    fs::write(integration.join("package-lock.json"), BUNDLED_PACKAGE_LOCK).expect("lock fixture");
    fs::write(
        package_dir.join("package.json"),
        format!(r#"{{"version":"{CAVEMAN_CODE_NPM_VERSION}"}}"#),
    )
    .expect("manifest fixture");
    (IntegrationLayout::new(integration), workspace)
}

fn write_fake_node(temp: &TempDir, body: &str) -> PathBuf {
    let fake_node = temp.path().join(format!("node-{}", uuid::Uuid::new_v4()));
    let script = format!(
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then\n  printf 'v20.18.1\\n'\n  exit 0\nfi\n{body}"
    );
    fs::write(&fake_node, script).expect("fake Node fixture");
    fs::set_permissions(&fake_node, fs::Permissions::from_mode(0o700)).expect("executable fixture");
    fake_node
}

fn managed_config(
    temp: &TempDir,
    node: PathBuf,
    integration: IntegrationLayout,
    workspace: PathBuf,
) -> ManagedAgentConfig {
    let token = BearerToken::new("a".repeat(64)).expect("valid token");
    let api = HzrApi::new("http://127.0.0.1:47391".into(), token).expect("loopback API");
    ManagedAgentConfig::new(
        node,
        integration,
        workspace,
        temp.path().join("agent-data"),
        api,
    )
}

async fn wait_for_file(path: &std::path::Path) {
    tokio::time::timeout(Duration::from_secs(15), async {
        while !path.is_file() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("fixture process did not create marker");
}
