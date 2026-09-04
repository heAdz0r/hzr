use super::read_files;
use crate::AppState;
use axum::{Json, extract::State};
use hzr_core::Config;
use hzr_exec::{PINNED_RTK_VERSION, expected_engine_identity};
use hzr_protocol::ReadApiRequest;
use std::{fs, os::unix::fs::PermissionsExt};

async fn fixture() -> (tempfile::TempDir, AppState, ReadApiRequest) {
    let dir = tempfile::tempdir().expect("fixture");
    let engines = dir.path().join("engines");
    fs::create_dir(&engines).expect("engines");
    let identity =
        serde_json::to_string(&expected_engine_identity().expect("identity")).expect("JSON");
    let engine_identity = expected_engine_identity().expect("identity");
    let receipt = serde_json::to_string(&serde_json::json!({
        "contract_version": engine_identity.contract_version,
        "engine": engine_identity,
        "correlation_id": "CORRELATION",
        "sequence": 0, "occurred_at_unix_ms": 0,
        "baseline_tokens": 0, "delivered_tokens": 0, "execution_ms": 0,
        "measurement": "unmeasured", "route": "optimized", "host_grant_applied": false,
        "attribution": {"operation": "read", "mode": "read_full", "stage": "internal_transport"}
    }))
    .expect("fixture receipt");
    let (receipt_before, receipt_after) =
        receipt.split_once("CORRELATION").expect("correlation slot");
    let binary = engines.join("rtk");
    fs::write(&binary, format!(r#"#!/bin/sh
case "$1 $2" in
  "--version ") printf 'rtk {PINNED_RTK_VERSION}\n';;
  "contract --json") printf '%s\n' '{identity}';;
  "rewrite --help") printf 'Usage: rtk rewrite [ARGS]... Raw command to rewrite\n';;
  "proxy --help") printf 'Usage: rtk proxy [ARGS]... Execute command without filtering\n';;
  *) if test "$1" = read; then
      source_file="$2"; shift 2
      from=1; to=100000
      while test "$#" -gt 0; do case "$1" in --from) from="$2"; shift 2;; --to) to="$2"; shift 2;; --level) shift 2;; *) exit 65;; esac; done
      set -- $(LC_ALL=C awk -v first="$from" -v last="$to" 'NR<first {{ start+=length($0)+1 }} NR>=first && NR<=last {{ count+=length($0)+1 }} END {{ print start+0, count+0 }}' "$source_file")
      dd if="$source_file" bs=1 skip="$1" count="$2" 2>/dev/null || exit 66
      printf '%s%s%s\n' '{receipt_before}' "$HZR_INTERNAL_ACCOUNTING_CORRELATION" '{receipt_after}' >> "$HZR_INTERNAL_ACCOUNTING_RECEIPT_JOURNAL"
     else exit 64; fi;;
esac
"#)).expect("engine");
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).expect("permissions");
    let mut config = Config {
        data_dir: dir.path().join("data"),
        ..Config::default()
    };
    config.engines.directory = Some(engines);
    config.engines.auto_start_icm = false;
    config.engines.auto_index = false;
    let state = AppState::initialize(config).await.expect("state");
    let request = ReadApiRequest {
        context_epoch: None,
        cwd: dir.path().to_string_lossy().into_owned(),
        paths: vec!["source.rs".into()],
        from: None,
        to: None,
        max_lines: None,
        max_tokens: Some(1024),
        expected_sha256: None,
        agent: None,
        session_id: None,
    };
    (dir, state, request)
}

#[tokio::test]
async fn full_read_and_revision_bound_expansion_are_exact() {
    let (dir, state, mut request) = fixture().await;
    fs::write(dir.path().join("source.rs"), "first\nsecond\nthird\n").expect("source");
    let Json(full) = read_files(State(state.clone()), Json(request.clone()))
        .await
        .expect("full");
    assert!(full.files[0].complete);
    assert_eq!(full.files[0].content, "first\nsecond\nthird\n");
    assert_eq!(full.files[0].next_line, None);
    request.max_lines = Some(1);
    let Json(part) = read_files(State(state.clone()), Json(request.clone()))
        .await
        .expect("part");
    assert!(!part.files[0].complete);
    assert_eq!(part.files[0].next_line, Some(2));
    request.from = Some(2);
    request.expected_sha256 = Some(part.files[0].source_sha256.clone());
    let Json(next) = read_files(State(state.clone()), Json(request.clone()))
        .await
        .expect("expansion");
    assert_eq!(next.files[0].content, "second\n");
    fs::write(dir.path().join("source.rs"), "changed\n").expect("change");
    assert!(
        read_files(State(state.clone()), Json(request))
            .await
            .is_err()
    );
    state
        .index_maintenance_stop
        .store(true, std::sync::atomic::Ordering::Release);
    state.context.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn batch_budget_and_confinement_are_enforced() {
    let (dir, state, mut request) = fixture().await;
    fs::write(
        dir.path().join("source.rs"),
        "quote \\\" unicode λ\n".repeat(1000),
    )
    .expect("large source");
    fs::write(dir.path().join("empty.rs"), "").expect("empty source");
    request.paths.push("empty.rs".into());
    let Json(result) = read_files(State(state.clone()), Json(request.clone()))
        .await
        .expect("batch");
    assert!(serde_json::to_vec(&result).expect("wire").len() <= 4096);
    assert!(result.files[0].next_line.is_some());
    assert!(!result.files[0].complete);
    request.paths = vec!["../outside".into()];
    assert!(
        read_files(State(state.clone()), Json(request))
            .await
            .is_err()
    );
    state
        .index_maintenance_stop
        .store(true, std::sync::atomic::Ordering::Release);
    state.context.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn read_episode_advice_preserves_explicit_content_and_resets_after_compaction() {
    let (dir, state, mut request) = fixture().await;
    fs::write(dir.path().join("source.rs"), "first\nsecond\nthird\n").expect("source");
    request.context_epoch = Some("before-compaction".into());
    request.session_id = Some("test-session".into());
    request.max_lines = Some(1);
    let Json(first) = read_files(State(state.clone()), Json(request.clone()))
        .await
        .expect("first");
    let Json(repeated) = read_files(State(state.clone()), Json(request.clone()))
        .await
        .expect("repeat");
    assert_eq!(first.files[0].content, repeated.files[0].content);
    let advice = repeated.files[0].cost_advice.as_ref().expect("cost advice");
    assert_eq!(advice.requests, 2);
    assert!(advice.repeated_source_tokens_estimated > 0);
    assert_eq!(advice.next_action, "read_remaining");
    assert_eq!(advice.next_missing_from, Some(2));
    assert_eq!(advice.next_missing_to, Some(3));
    assert!(serde_json::to_vec(&repeated).expect("wire").len() <= 4096);
    request.context_epoch = Some("after-compaction".into());
    let Json(reset) = read_files(State(state.clone()), Json(request.clone()))
        .await
        .expect("reset");
    assert_eq!(
        reset.files[0]
            .cost_advice
            .as_ref()
            .expect("new episode")
            .requests,
        1
    );
    request.max_lines = None;
    let Json(full) = read_files(State(state.clone()), Json(request))
        .await
        .expect("explicit full");
    assert_eq!(full.files[0].content, "first\nsecond\nthird\n");
    assert!(full.files[0].complete);
    state
        .index_maintenance_stop
        .store(true, std::sync::atomic::Ordering::Release);
    state.context.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn exact_read_preserves_crlf_and_unterminated_eof() {
    let (dir, state, mut request) = fixture().await;
    for source in ["first\r\nsecond\r\n", "first\nsecond", "last", ""] {
        fs::write(dir.path().join("source.rs"), source).expect("source");
        request.from = None;
        let Json(full) = read_files(State(state.clone()), Json(request.clone()))
            .await
            .expect("exact full read");
        assert_eq!(full.files[0].content, source);
        assert!(full.files[0].complete);
        if source.contains('\n') {
            request.from = Some(2);
            let Json(range) = read_files(State(state.clone()), Json(request.clone()))
                .await
                .expect("exact range");
            assert_eq!(
                range.files[0].content,
                source.split_once('\n').expect("line boundary").1
            );
        }
    }
    state
        .index_maintenance_stop
        .store(true, std::sync::atomic::Ordering::Release);
    state.context.shutdown().await.expect("shutdown");
}
