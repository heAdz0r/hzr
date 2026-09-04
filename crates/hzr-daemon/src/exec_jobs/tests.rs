use std::fs;

#[tokio::test]
async fn spilled_output_is_bounded_hash_bound_and_confined_to_its_job() {
    let (directory, state) = fixture().await;
    let request = request(&directory, "head -c 300000 /dev/zero | tr '\\000' x");
    let _ = start(State(state.clone()), Json(request.clone()))
        .await
        .expect("start output fixture");
    let snapshot = wait(State(state.clone()), Json(poll(&request)))
        .await
        .expect("output completion")
        .0;
    assert_eq!(snapshot.state, ExecJobState::Completed);
    let mut output_request = hzr_protocol::ExecOutputApiRequest {
        operation_id: request.operation_id.clone(),
        cwd: request.request.cwd.clone(),
        stream: hzr_protocol::ExecOutputStream::Stdout,
        offset: 0,
        max_bytes: Some(11),
        expected_sha256: None,
    };
    let page = super::read_output(State(state.clone()), Json(output_request.clone()))
        .await
        .expect("bounded spill")
        .0;
    assert_eq!(page.content, "xxxxxxxxxxx");
    assert_eq!(page.next_offset, Some(11));
    assert_eq!(page.total_bytes, 300000);
    assert!(!page.complete);
    output_request.offset = page.next_offset.expect("continuation");
    output_request.expected_sha256 = Some(page.source_sha256);
    assert_eq!(
        super::read_output(State(state.clone()), Json(output_request.clone()))
            .await
            .expect("same immutable output")
            .0
            .offset,
        11
    );
    output_request.expected_sha256 = Some("0".repeat(64));
    assert!(
        super::read_output(State(state.clone()), Json(output_request.clone()))
            .await
            .is_err()
    );
    output_request.expected_sha256 = None;
    let outside = tempfile::tempdir().expect("foreign workspace");
    output_request.cwd = outside.path().to_string_lossy().into_owned();
    assert!(
        super::read_output(State(state.clone()), Json(output_request.clone()))
            .await
            .is_err()
    );
    output_request.cwd = request.request.cwd;
    let path = state
        .exec_jobs
        .path(&request.operation_id)
        .expect("job record path");
    let mut record = state
        .exec_jobs
        .read_record(&path)
        .expect("completed record");
    let Some(ExecutionOutcome::Completed { result }) = &mut record.snapshot.outcome else {
        panic!("expected execution result");
    };
    assert!(matches!(
        result.stdout.content,
        hzr_exec::CapturedContent::Spilled { .. }
    ));
    let secret = outside.path().join("outside-output");
    fs::write(&secret, b"not job output").expect("foreign file fixture");
    result.stdout.content = hzr_exec::CapturedContent::Spilled { path: secret };
    state
        .exec_jobs
        .write_record(&record)
        .expect("tampered output manifest fixture");
    assert!(
        super::read_output(State(state), Json(output_request))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn output_pages_preserve_binary_and_utf8_boundaries() {
    let (directory, state) = fixture().await;
    let request = request(&directory, "printf '\\377\\000'; printf 'αβγ' >&2");
    let _ = start(State(state.clone()), Json(request.clone()))
        .await
        .expect("start byte fixture");
    let _ = wait(State(state.clone()), Json(poll(&request)))
        .await
        .expect("wait byte fixture");
    let mut output_request = hzr_protocol::ExecOutputApiRequest {
        operation_id: request.operation_id,
        cwd: request.request.cwd,
        stream: hzr_protocol::ExecOutputStream::Stdout,
        offset: 0,
        max_bytes: Some(3),
        expected_sha256: None,
    };
    let binary = super::read_output(State(state.clone()), Json(output_request.clone()))
        .await
        .expect("binary output")
        .0;
    assert_eq!(binary.encoding, "hex");
    assert_eq!(binary.content, "ff00");
    assert!(binary.complete);
    output_request.stream = hzr_protocol::ExecOutputStream::Stderr;
    let text = super::read_output(State(state), Json(output_request))
        .await
        .expect("UTF-8 page")
        .0;
    assert_eq!(text.encoding, "utf8");
    assert_eq!(text.content, "α");
    assert_eq!(text.next_offset, Some(2));
    assert!(!text.complete);
}

#[tokio::test]
async fn cancellation_before_dispatch_cannot_be_replayed_into_execution() {
    let (directory, state) = fixture().await;
    let request = request(&directory, "printf unexpected > should-not-exist");
    let cancelled = cancel(State(state.clone()), Json(poll(&request)))
        .await
        .expect("cancel tombstone")
        .0;
    assert_eq!(cancelled.state, ExecJobState::Cancelled);
    let attempted = start(State(state.clone()), Json(request))
        .await
        .expect("cancelled replay")
        .0;
    assert_eq!(attempted.state, ExecJobState::Cancelled);
    assert!(!directory.path().join("should-not-exist").exists());
    assert!(state.exec_jobs.active.lock().await.is_empty());
}

#[tokio::test]
async fn missing_completion_never_polls_running_forever() {
    let (directory, state) = fixture().await;
    let request = request(&directory, "printf unused");
    let record = JobRecord {
        workspace: directory
            .path()
            .canonicalize()
            .expect("valid execution fixture"),
        request_hash: "already-dispatched".into(),
        snapshot: hzr_exec::ExecJobSnapshot {
            delivery: None,
            operation_id: request.operation_id.clone(),
            state: ExecJobState::Running,
            revision: 1,
            outcome: None,
            error: None,
        },
    };
    state
        .exec_jobs
        .write_record(&record)
        .expect("valid execution fixture");
    let snapshot = wait(State(state), Json(poll(&request)))
        .await
        .expect("valid execution fixture")
        .0;
    assert_eq!(snapshot.state, ExecJobState::Interrupted);
    assert_eq!(
        snapshot.error.expect("valid execution fixture").code,
        "execution_completion_unknown"
    );
}

#[tokio::test]
async fn result_delivery_budget_and_revision_do_not_reexecute() {
    let (directory, state) = fixture().await;
    let request = request(&directory, "printf once > execution-marker; printf result");
    let _ = start(State(state.clone()), Json(request.clone()))
        .await
        .expect("valid execution fixture");
    let first = wait(State(state.clone()), Json(poll(&request)))
        .await
        .expect("valid execution fixture")
        .0;
    assert!(first.outcome.is_some());
    let mut repeated = poll(&request);
    repeated.after_revision = Some(first.revision);
    let second = wait(State(state.clone()), Json(repeated))
        .await
        .expect("valid execution fixture")
        .0;
    assert!(second.outcome.is_none());
    assert!(second.delivery.expect("valid execution fixture").unchanged);
    let required = first
        .delivery
        .as_ref()
        .expect("valid execution fixture")
        .required_bytes;
    assert!(required > 0);
    let mut invalid = poll(&request);
    invalid.max_output_bytes = Some(1);
    assert!(wait(State(state), Json(invalid)).await.is_err());
    assert_eq!(
        fs::read_to_string(directory.path().join("execution-marker"))
            .expect("valid execution fixture"),
        "once"
    );
}

#[test]
fn output_budget_omission_is_explicit_and_larger_fetch_is_exact() {
    let outcome = ExecutionOutcome::NotStarted {
        disposition: hzr_exec::NotStarted::Denied {
            requested: hzr_exec::CanonicalCommand::shell("x".repeat(2000)),
            reason: "policy".into(),
        },
    };
    let snapshot = hzr_exec::ExecJobSnapshot {
        delivery: None,
        operation_id: uuid::Uuid::new_v4().to_string(),
        state: ExecJobState::Completed,
        revision: 2,
        outcome: Some(outcome.clone()),
        error: None,
    };
    let bounded =
        super::deliver(snapshot.clone(), None, Some(1024)).expect("valid execution fixture");
    assert!(bounded.outcome.is_none());
    assert!(
        bounded
            .delivery
            .expect("valid execution fixture")
            .output_omitted
    );
    let full = super::deliver(snapshot, None, Some(8192)).expect("valid execution fixture");
    assert_eq!(full.outcome, Some(outcome));
}

#[test]
fn execution_record_read_rejects_symlink_and_store_quota_reserves_completions() {
    let directory = tempfile::tempdir().expect("valid execution fixture");
    let jobs = ExecJobs::new(directory.path()).expect("valid execution fixture");
    let id = uuid::Uuid::new_v4().to_string();
    let target = directory.path().join("foreign.json");
    fs::write(&target, "{}").expect("valid execution fixture");
    std::os::unix::fs::symlink(&target, jobs.path(&id).expect("valid execution fixture"))
        .expect("valid execution fixture");
    assert!(
        jobs.read_record(&jobs.path(&id).expect("valid execution fixture"))
            .is_err()
    );
    assert!(jobs.reserve_record_capacity(32).is_err());
}
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;

use axum::{Json, extract::State};
use hzr_core::Config;
use hzr_exec::{
    ExecJobState, ExecutionOutcome, PINNED_RTK_VERSION, TerminationCause, expected_engine_identity,
};
use hzr_protocol::{ExecApiRequest, ExecJobApiRequest, ExecStartApiRequest};
use tempfile::TempDir;

use super::{ExecJobs, JobRecord, cancel, start, wait};
use crate::AppState;

async fn fixture() -> (TempDir, AppState) {
    let directory = tempfile::tempdir().expect("temporary fixture");
    let engines = directory.path().join("engines");
    fs::create_dir(&engines).expect("engines");
    let contract =
        serde_json::to_string(&expected_engine_identity().expect("identity")).expect("JSON");
    let binary = engines.join("rtk");
    fs::write(
        &binary,
        format!(
            r#"#!/bin/sh
case "$1 $2" in
  "--version ") printf 'rtk {PINNED_RTK_VERSION}\n';;
  "contract --json") printf '%s\n' '{contract}';;
  "rewrite --help") printf 'Usage: rtk rewrite [ARGS]... Raw command to rewrite\n';;
  "proxy --help") printf 'Usage: rtk proxy [ARGS]... Execute command without filtering\n';;
  *) if test "$1" = rewrite-plan; then printf '{{"decision":"proxy"}}'; elif test "$1" = proxy; then shift; exec "$@"; else exit 64; fi;;
esac
"#
        ),
    )
    .expect("fake engine");
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).expect("permissions");
    let mut config = Config {
        data_dir: directory.path().join("data"),
        ..Config::default()
    };
    config.engines.directory = Some(engines);
    config.engines.auto_start_icm = false;
    config.engines.auto_index = false;
    let state = AppState::initialize(config).await.expect("state");
    (directory, state)
}

fn request(directory: &TempDir, command: &str) -> ExecStartApiRequest {
    ExecStartApiRequest {
        operation_id: uuid::Uuid::new_v4().to_string(),
        request: ExecApiRequest {
            channel: None,
            cwd: directory.path().to_string_lossy().into_owned(),
            command: command.into(),
            fidelity_requested: false,
            fidelity_reason: None,
            timeout_ms: Some(120_000),
            caller_path: std::env::var("PATH").ok(),
            agent: Some("test".into()),
            session_id: Some("job-fixture".into()),
            host_execution_grant: None,
        },
    }
}

fn poll(request: &ExecStartApiRequest) -> ExecJobApiRequest {
    ExecJobApiRequest {
        operation_id: request.operation_id.clone(),
        cwd: request.request.cwd.clone(),
        wait_ms: Some(10_000),
        after_revision: None,
        max_output_bytes: None,
    }
}

#[tokio::test]
async fn replay_does_not_execute_again_and_workspace_scope_is_enforced() {
    let (directory, state) = fixture().await;
    let request = request(
        &directory,
        "printf 'one\\n' >> executions; sleep 0.1; printf done",
    );
    let first = start(State(state.clone()), Json(request.clone()))
        .await
        .expect("start")
        .0;
    assert_eq!(first.state, ExecJobState::Running);
    let second = start(State(state.clone()), Json(request.clone()))
        .await
        .expect("same dispatch")
        .0;
    assert_eq!(first.operation_id, second.operation_id);
    let result = wait(State(state.clone()), Json(poll(&request)))
        .await
        .expect("wait")
        .0;
    assert_eq!(result.state, ExecJobState::Completed, "{result:?}");
    assert!(matches!(
        result.outcome,
        Some(ExecutionOutcome::Completed { .. })
    ));
    assert_eq!(
        fs::read_to_string(directory.path().join("executions")).expect("side effect"),
        "one\n"
    );
    let replay = start(State(state.clone()), Json(request.clone()))
        .await
        .expect("completed replay")
        .0;
    assert_eq!(replay.state, ExecJobState::Completed);
    let mut changed = request.clone();
    changed.request.command = "printf different".into();
    assert!(start(State(state.clone()), Json(changed)).await.is_err());
    let foreign = tempfile::tempdir().expect("other workspace");
    let mut invalid = poll(&request);
    invalid.cwd = foreign.path().to_string_lossy().into_owned();
    assert!(wait(State(state), Json(invalid)).await.is_err());
}

#[tokio::test]
async fn cancellation_reaps_the_execution_before_terminal_receipt() {
    let (directory, state) = fixture().await;
    let request = request(&directory, "printf started > started; sleep 30");
    let _ = start(State(state.clone()), Json(request.clone()))
        .await
        .expect("start");
    let started = tokio::time::timeout(Duration::from_secs(3), async {
        while !directory.path().join("started").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(
        started.is_ok(),
        "process not started: {:?}",
        wait(
            State(state.clone()),
            Json(ExecJobApiRequest {
                wait_ms: Some(0),
                ..poll(&request)
            })
        )
        .await
    );
    let result = cancel(State(state.clone()), Json(poll(&request)))
        .await
        .expect("cancel")
        .0;
    assert_eq!(result.state, ExecJobState::Cancelled);
    let result = result
        .outcome
        .and_then(|outcome| match outcome {
            ExecutionOutcome::Completed { result } => Some(result),
            _ => None,
        })
        .expect("missing execution completion");
    assert_eq!(result.termination.cause, TerminationCause::Cancelled);
    assert!(state.exec_jobs.active.lock().await.is_empty());
}

#[test]
fn restart_marks_unfinished_execution_unknown_without_replaying() {
    let directory = tempfile::tempdir().expect("temporary");
    let jobs = ExecJobs::new(directory.path()).expect("store");
    let operation_id = uuid::Uuid::new_v4().to_string();
    let record = JobRecord {
        workspace: directory.path().canonicalize().expect("canonical"),
        request_hash: "private-command-hash".into(),
        snapshot: hzr_exec::ExecJobSnapshot {
            delivery: None,
            operation_id: operation_id.clone(),
            state: ExecJobState::Running,
            revision: 1,
            outcome: None,
            error: None,
        },
    };
    jobs.write_record(&record).expect("running marker");
    let restarted = ExecJobs::new(directory.path()).expect("restart");
    let recovered = restarted
        .read_record(&restarted.path(&operation_id).expect("path"))
        .expect("record");
    assert_eq!(recovered.snapshot.state, ExecJobState::Interrupted);
    assert_eq!(recovered.snapshot.revision, 2);
    assert!(restarted.path("../elsewhere").is_err());
}

#[tokio::test]
#[ignore = "90-second real-process probe; run explicitly when execution lifecycle changes"]
async fn ninety_second_job_survives_short_request_budget() {
    let (directory, mut state) = fixture().await;
    std::sync::Arc::make_mut(&mut state.config)
        .daemon
        .request_timeout_ms = 1000;
    let request = request(&directory, "sleep 90; printf finished");
    let _ = start(State(state.clone()), Json(request.clone()))
        .await
        .expect("start");
    let result = tokio::time::timeout(Duration::from_secs(115), async {
        loop {
            let snapshot = wait(State(state.clone()), Json(poll(&request)))
                .await
                .expect("wait")
                .0;
            if snapshot.state != ExecJobState::Running {
                break snapshot;
            }
        }
    })
    .await
    .expect("long command completed");
    assert_eq!(result.state, ExecJobState::Completed);
    let result = result
        .outcome
        .and_then(|outcome| match outcome {
            ExecutionOutcome::Completed { result } => Some(result),
            _ => None,
        })
        .expect("missing completed command");
    assert_eq!(result.termination.exit_code, Some(0));
}
