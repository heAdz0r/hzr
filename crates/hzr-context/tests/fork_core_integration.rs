#![cfg(unix)]

use std::fs;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use hzr_context::{ContextPlanner, PlanRequest, SearchRequest};
use hzr_core::Config;
use hzr_exec::{ForkCoreConfig, ForkRuntimePaths, PinnedRtkAdapter, RtkRewriteInterface};
use hzr_memory::{IcmClient, IcmConfig, IcmTransport};
use hzr_protocol::{CandidateSource, ContextWarningCode, SearchMode, SearchStrategy};

#[tokio::test]
async fn test_search_and_context_use_managed_fork_core_commands() {
    let fixture = tempfile::tempdir().expect("fixture directory");
    let workspace = fixture.path().join("workspace");
    let engines = fixture.path().join("engines");
    fs::create_dir_all(workspace.join("src")).expect("workspace source directory");
    fs::create_dir_all(&engines).expect("engine directory");
    fs::write(
        workspace.join("src/lib.rs"),
        "pub fn managed_fork_hit() {}\n",
    )
    .expect("source fixture");
    let rtk = engines.join("rtk");
    write_fake_rtk(&rtk);

    let mut config = Config {
        data_dir: fixture.path().join("data"),
        ..Config::default()
    };
    config.engines.directory = Some(engines);
    config.engines.auto_index = true;
    config.daemon.request_timeout_ms = 5_000;
    config.ensure_layout().expect("HZR data layout");

    let adapter = PinnedRtkAdapter::detect(ForkCoreConfig {
        binary: rtk,
        runtime_paths: Some(ForkRuntimePaths::from_data_root(&config.data_dir)),
        // Full-workspace tests can saturate CI executors; this fixture asserts the
        // command contract, not a five-second scheduling deadline.
        probe_timeout_ms: 20_000,
        ..ForkCoreConfig::default()
    })
    .await;
    assert!(matches!(
        adapter.capabilities().rewrite,
        RtkRewriteInterface::ForkCli
    ));
    let planner = ContextPlanner::from_config(
        &config,
        unavailable_memory(fixture.path()),
        adapter.runner(),
    );

    let search = planner
        .search(SearchRequest {
            workspace: workspace.clone(),
            query: "managed_fork_hit".into(),
            path: Some(PathBuf::from("src")),
            limit: 5,
            mode: SearchMode::Exact,
            include_content: true,
        })
        .await
        .expect("fork rgai search");
    // Exact mode must reach fork-core as a verbatim lookup. The fake engine exits 65 when
    // `--literal` is absent, so this fails loudly rather than silently ranking terms — the
    // behaviour that made `hzr search "fn handle_request" --mode exact` return every `fn`.
    assert_eq!(search.strategy, SearchStrategy::ForkRgaiBuiltin);
    assert_eq!(search.hits.len(), 1);
    // The engine reported `lib.rs` relative to the `src` scope; an agent needs the path it
    // can actually open, so HZR must rebase it onto the project root. The fake engine exits
    // 67 when `--project-root` is missing, which is what made a file-scoped exact search fail.
    assert_eq!(search.hits[0].path, "src/lib.rs");

    let leading_hyphen = planner
        .search(SearchRequest {
            workspace: workspace.clone(),
            query: "--outline".into(),
            path: Some(PathBuf::from("src")),
            limit: 5,
            mode: SearchMode::Exact,
            include_content: true,
        })
        .await
        .expect("exact query beginning with a hyphen");
    assert_eq!(leading_hyphen.query, "--outline");
    assert_eq!(leading_hyphen.hits.len(), 1);

    let context = planner
        .plan(PlanRequest {
            workspace,
            intent: "find managed fork hit".into(),
            path: Some(PathBuf::from("src")),
            topic: Some("hzr-test".into()),
            search_limit: 5,
            memory_limit: 5,
        })
        .await
        .expect("fork memory plan");
    assert_eq!(
        context
            .planner
            .as_ref()
            .and_then(|planner| planner.pipeline_version.as_deref()),
        Some("graph_first_v1")
    );
    assert_eq!(context.pack.selected.len(), 1);
    assert_eq!(context.pack.selected[0].source, CandidateSource::Context);
    assert_eq!(context.pack.selected[0].path.as_deref(), Some("src/lib.rs"));
    assert!(
        context
            .warnings
            .iter()
            .any(|warning| warning.code == ContextWarningCode::MemoryUnavailable)
    );

    let legacy_workspace = fixture.path().join("legacy-workspace");
    fs::create_dir_all(legacy_workspace.join("src")).expect("legacy workspace source directory");
    fs::create_dir(legacy_workspace.join(".grepai")).expect("legacy grepai directory");
    fs::write(
        legacy_workspace.join("src/lib.rs"),
        "pub fn managed_fork_hit() {}\n",
    )
    .expect("legacy source fixture");

    let legacy_search = planner
        .search(SearchRequest {
            workspace: legacy_workspace.clone(),
            query: "managed_fork_hit".into(),
            path: Some(PathBuf::from("src")),
            limit: 5,
            mode: SearchMode::Auto,
            include_content: true,
        })
        .await
        .expect("legacy index search falls back internally");
    assert_eq!(legacy_search.strategy, SearchStrategy::ForkRgaiBuiltin);
    assert_eq!(legacy_search.hits.len(), 1);
    assert!(
        legacy_search
            .fallback_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("requires explicit migration"))
    );

    let legacy_context = planner
        .plan(PlanRequest {
            workspace: legacy_workspace,
            intent: "find managed fork hit".into(),
            path: Some(PathBuf::from("src")),
            topic: Some("hzr-test".into()),
            search_limit: 5,
            memory_limit: 5,
        })
        .await
        .expect("legacy index planning falls back internally");
    assert!(legacy_context.warnings.iter().any(|warning| {
        warning.code == ContextWarningCode::SearchDegraded
            && warning.message.contains("must be centralized")
    }));

    planner.shutdown().await.expect("index shutdown");
}

fn unavailable_memory(root: &Path) -> IcmClient {
    let mut config = IcmConfig::from_data_root(root.join("missing-icm"), root.join("icm"));
    config.bind_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 65_534);
    config.request_timeout = Duration::from_millis(50);
    config.cli_fallback = false;
    config.transport = IcmTransport::Http;
    IcmClient::from_config(config).expect("ICM client fixture")
}

fn write_fake_rtk(path: &Path) {
    let script = r#"#!/bin/sh
case "$1" in
  --version)
    printf '%s\n' 'rtk 0.44.1-fork.1'
    ;;
  rewrite)
    if [ "$2" = "--help" ]; then
      printf '%s\n' 'rtk rewrite - Raw command to rewrite'
    else
      exit 64
    fi
    ;;
  proxy)
    if [ "$2" = "--help" ]; then
      printf '%s\n' 'rtk proxy - execute without filtering'
    else
      exit 64
    fi
    ;;
  rgai)
    literal_found=false
    separator_found=false
    query=""
    for argument in "$@"; do
      if [ "$argument" = "--literal" ]; then
        literal_found=true
      elif [ "$argument" = "--" ]; then
        separator_found=true
      elif [ "$separator_found" = "true" ]; then
        query="$argument"
        separator_found=false
      fi
    done
    if [ "$literal_found" != "true" ] || [ -z "$query" ]; then
      exit 65
    fi
    root_found=false
    for argument in "$@"; do
      if [ "$argument" = "--project-root" ]; then
        root_found=true
      fi
    done
    if [ "$root_found" != "true" ]; then
      exit 67
    fi
    # Hit paths are relative to `--path`, not to the project root: searching `src` reports
    # `lib.rs`. This fixture used to report `src/lib.rs`, which the real engine never returns,
    # so it hid the rebasing HZR has to do to give an agent a path it can open.
    printf '%s\n' "{\"query\":\"$query\",\"path\":\"src\",\"total_hits\":1,\"shown_hits\":1,\"scanned_files\":1,\"skipped_large\":0,\"skipped_binary\":0,\"hits\":[{\"path\":\"lib.rs\",\"score\":9.5,\"matched_lines\":1,\"snippets\":[{\"lines\":[{\"line\":1,\"text\":\"pub fn managed_fork_hit() {}\"}],\"matched_terms\":[\"$query\"]}]}]}"
    ;;
  memory)
    if [ "$2" != "plan" ]; then
      exit 66
    fi
    if [ "$4" != "src" ]; then
      exit 68
    fi
    printf '%s\n' '{"selected":[{"rel_path":"src/lib.rs","features":{},"score":0.9,"sources":["tier_a","call_graph"],"estimated_tokens":200}],"dropped":[],"budget_report":{"token_budget":12000,"estimated_used":200,"candidates_total":1,"candidates_selected":1,"efficiency_score":0.0167},"decision_trace":[],"pipeline_version":"graph_first_v1","semantic_backend_used":"rg-files","graph_candidate_count":1,"semantic_hit_count":1}'
    ;;
  *)
    exit 67
    ;;
esac
"#;
    fs::write(path, script).expect("fake rtk script");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("fake rtk permissions");
}
