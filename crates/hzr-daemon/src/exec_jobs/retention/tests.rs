use std::fs;
use std::time::Duration;

use axum::{Json, extract::State};
use hzr_exec::{ExecJobSnapshot, ExecJobState};
use hzr_protocol::{ExecOutputApiRequest, ExecOutputStream};

use super::super::tests::{fixture, poll, request};
use super::super::{ExecJobs, JobRecord, cancel, read_output, start, wait};

fn expired(jobs: &ExecJobs, id: &str) -> JobRecord {
    let mut record = jobs
        .read_record(&jobs.path(id).expect("path"))
        .expect("record");
    record.snapshot.outcome = None;
    record.snapshot.delivery = None;
    record.snapshot.revision += 1;
    record.snapshot.error = Some(
        crate::error::ApiError::service(
            "execution_result_expired",
            "fixture expired result",
            false,
        )
        .payload(),
    );
    record
}

#[tokio::test]
async fn reclaimed_command_retains_request_and_workspace_binding_after_restart() {
    let (directory, mut state) = fixture().await;
    let command = request(&directory, "printf once >> executions; printf output");
    let _ = start(State(state.clone()), Json(command.clone()))
        .await
        .expect("start");
    let completed = wait(State(state.clone()), Json(poll(&command)))
        .await
        .expect("completion")
        .0;
    assert_eq!(completed.state, ExecJobState::Completed);
    let record = expired(&state.exec_jobs, &command.operation_id);
    state.exec_jobs.compact(&[record]).expect("compact");
    state.exec_jobs = ExecJobs::new(&state.config.data_dir).expect("restart");
    let replay = start(State(state.clone()), Json(command.clone()))
        .await
        .expect("replay receipt")
        .0;
    assert_eq!(replay.state, ExecJobState::Completed);
    assert!(replay.outcome.is_none());
    assert_eq!(
        replay.error.expect("expired").code,
        "execution_result_expired"
    );
    assert_eq!(
        fs::read_to_string(directory.path().join("executions")).expect("effects"),
        "once"
    );
    let mut changed = command.clone();
    changed.request.command = "printf twice >> executions".into();
    assert!(start(State(state.clone()), Json(changed)).await.is_err());
    let foreign = tempfile::tempdir().expect("foreign workspace");
    let mut changed = poll(&command);
    changed.cwd = foreign.path().to_string_lossy().into_owned();
    assert_eq!(
        wait(State(state.clone()), Json(changed))
            .await
            .expect_err("workspace isolation")
            .payload()
            .code,
        "execution_not_found"
    );
    let output = ExecOutputApiRequest {
        operation_id: command.operation_id,
        cwd: command.request.cwd,
        stream: ExecOutputStream::Stdout,
        offset: 0,
        max_bytes: None,
        expected_sha256: None,
    };
    assert_eq!(
        read_output(State(state), Json(output))
            .await
            .expect_err("expired output")
            .payload()
            .code,
        "execution_result_expired"
    );
}

#[tokio::test]
async fn cancelled_and_interrupted_receipts_never_dispatch_after_restart() {
    let (directory, mut state) = fixture().await;
    let cancelled = request(&directory, "printf unexpected > cancelled-effect");
    let _ = cancel(State(state.clone()), Json(poll(&cancelled)))
        .await
        .expect("cancel before dispatch");
    state
        .exec_jobs
        .compact(&[expired(&state.exec_jobs, &cancelled.operation_id)])
        .expect("compact cancellation");
    let interrupted = request(&directory, "printf unexpected > interrupted-effect");
    let record = JobRecord {
        workspace: directory.path().canonicalize().expect("workspace"),
        request_hash: hzr_core::privacy_identity_hash(
            "exec_job",
            &serde_json::to_string(&interrupted.request).expect("request"),
        ),
        snapshot: ExecJobSnapshot {
            delivery: None,
            operation_id: interrupted.operation_id.clone(),
            state: ExecJobState::Running,
            revision: 1,
            outcome: None,
            error: None,
        },
    };
    state
        .exec_jobs
        .write_record(&record)
        .expect("interrupted dispatch marker");
    state.exec_jobs = ExecJobs::new(&state.config.data_dir).expect("restart running marker");
    state
        .exec_jobs
        .compact(&[expired(&state.exec_jobs, &interrupted.operation_id)])
        .expect("compact interruption");
    state.exec_jobs = ExecJobs::new(&state.config.data_dir).expect("restart compact store");
    for (request, expected) in [
        (cancelled, ExecJobState::Cancelled),
        (interrupted, ExecJobState::Interrupted),
    ] {
        assert_eq!(
            start(State(state.clone()), Json(request))
                .await
                .expect("durable terminal identity")
                .0
                .state,
            expected
        );
    }
    assert!(!directory.path().join("cancelled-effect").exists());
    assert!(!directory.path().join("interrupted-effect").exists());
}

#[tokio::test]
async fn compaction_interruption_retains_full_results_and_recovers_owned_output() {
    let (directory, mut state) = fixture().await;
    let command = request(&directory, "printf once >> executions");
    let _ = start(State(state.clone()), Json(command.clone()))
        .await
        .expect("start");
    let _ = wait(State(state.clone()), Json(poll(&command)))
        .await
        .expect("complete");
    let mut receipt = expired(&state.exec_jobs, &command.operation_id);
    state
        .exec_jobs
        .receipts
        .store(&[receipt])
        .expect("durable receipt before unlink");
    state.exec_jobs = ExecJobs::new(&state.config.data_dir).expect("restart before unlink");
    let replay = start(State(state.clone()), Json(command.clone()))
        .await
        .expect("full result retained")
        .0;
    assert!(replay.outcome.is_some());
    receipt = expired(&state.exec_jobs, &command.operation_id);
    receipt.snapshot.error.as_mut().expect("expired").message = "new release wording".into();
    state
        .exec_jobs
        .receipts
        .store(&[receipt])
        .expect("semantic receipt identity is stable across wording changes");
    fs::remove_file(state.exec_jobs.path(&command.operation_id).expect("path"))
        .expect("simulate crash after payload unlink");
    let output = state
        .exec_jobs
        .directory
        .join(format!("{}.output", command.operation_id));
    fs::create_dir(&output).expect("leftover output");
    fs::write(output.join("stdout"), "old captured output").expect("output bytes");
    state.exec_jobs = ExecJobs::new(&state.config.data_dir).expect("restart after unlink");
    assert!(!output.exists());
    assert!(
        start(State(state), Json(command))
            .await
            .expect("no replay")
            .0
            .outcome
            .is_none()
    );
    assert_eq!(
        fs::read_to_string(directory.path().join("executions")).expect("effects"),
        "once"
    );
}

#[tokio::test]
async fn failed_receipt_commit_preserves_full_record_and_output() {
    let (directory, state) = fixture().await;
    let command = request(&directory, "printf once");
    let _ = start(State(state.clone()), Json(command.clone()))
        .await
        .expect("start");
    let _ = wait(State(state.clone()), Json(poll(&command)))
        .await
        .expect("complete");
    let output = state
        .exec_jobs
        .directory
        .join(format!("{}.output", command.operation_id));
    fs::create_dir(&output).expect("output directory");
    fs::write(output.join("stdout"), "preserved").expect("output");
    let database = rusqlite::Connection::open(state.exec_jobs.directory.join("replay.sqlite"))
        .expect("test database");
    database.execute_batch("CREATE TRIGGER fail_receipt BEFORE INSERT ON receipts BEGIN SELECT RAISE(ABORT, 'injected storage failure'); END;").expect("failure injection");
    assert!(
        state
            .exec_jobs
            .compact(&[expired(&state.exec_jobs, &command.operation_id)])
            .is_err()
    );
    assert!(
        state
            .exec_jobs
            .path(&command.operation_id)
            .expect("record path")
            .exists()
    );
    assert_eq!(
        fs::read_to_string(output.join("stdout")).expect("output preserved"),
        "preserved"
    );
    assert!(
        state
            .exec_jobs
            .receipts
            .read(&command.operation_id)
            .expect("receipt store")
            .is_none()
    );
}

#[test]
fn required_receipt_database_cannot_be_reinitialized_or_redirected() {
    for defect in [
        "missing",
        "empty",
        "missing-table",
        "version",
        "application",
        "symlink",
        "journal-symlink",
    ] {
        let directory = tempfile::tempdir().expect("store fixture");
        let jobs = ExecJobs::new(directory.path()).expect("initial store");
        let database_path = jobs.directory.join("replay.sqlite");
        match defect {
            "missing" => fs::remove_file(&database_path).expect("remove database"),
            "empty" => fs::write(&database_path, []).expect("truncate database"),
            "missing-table" | "version" | "application" => {
                let connection =
                    rusqlite::Connection::open(&database_path).expect("fixture connection");
                connection
                    .execute_batch(match defect {
                        "missing-table" => "DROP TABLE receipts",
                        "version" => "PRAGMA user_version = 2",
                        _ => "PRAGMA application_id = 0",
                    })
                    .expect("alter fixture schema");
            }
            "symlink" => {
                fs::remove_file(&database_path).expect("remove database");
                std::os::unix::fs::symlink(directory.path().join("foreign"), &database_path)
                    .expect("foreign database symlink");
            }
            _ => std::os::unix::fs::symlink(
                directory.path().join("foreign"),
                jobs.directory.join("replay.sqlite-journal"),
            )
            .expect("foreign journal symlink"),
        }
        assert!(
            ExecJobs::new(directory.path()).is_err(),
            "must fail closed for {defect}"
        );
    }
}

#[test]
fn live_foreign_receipt_database_never_becomes_an_unknown_operation() {
    for defect in ["empty", "missing-table", "version", "application"] {
        let directory = tempfile::tempdir().expect("store fixture");
        let jobs = ExecJobs::new(directory.path()).expect("store");
        let path = jobs.directory.join("replay.sqlite");
        if defect == "empty" {
            fs::write(&path, []).expect("replace live database");
        } else {
            let database = rusqlite::Connection::open(path).expect("database fixture");
            database
                .execute_batch(match defect {
                    "missing-table" => "DROP TABLE receipts",
                    "version" => "PRAGMA user_version = 2",
                    _ => "PRAGMA application_id = 0",
                })
                .expect("foreign live schema");
        }
        let id = uuid::Uuid::new_v4().to_string();
        assert!(
            jobs.receipts.read(&id).is_err(),
            "must fail closed for {defect}"
        );
        assert!(
            jobs.receipts.store(&[]).is_err(),
            "cannot commit into foreign {defect}"
        );
        assert_ne!(
            jobs.read_record(&jobs.path(&id).expect("operation path"))
                .err()
                .expect("do not dispatch")
                .kind(),
            std::io::ErrorKind::NotFound
        );
    }
}

#[tokio::test]
async fn spilled_output_reads_hold_the_same_lease_as_reclamation() {
    let (directory, state) = fixture().await;
    let command = request(&directory, "printf leased-output");
    let _ = start(State(state.clone()), Json(command.clone()))
        .await
        .expect("start");
    let _ = wait(State(state.clone()), Json(poll(&command)))
        .await
        .expect("complete");
    let jobs = state.exec_jobs.clone();
    let lease = jobs.active.lock().await;
    let mut reader = tokio::spawn(read_output(
        State(state),
        Json(ExecOutputApiRequest {
            operation_id: command.operation_id,
            cwd: command.request.cwd,
            stream: ExecOutputStream::Stdout,
            offset: 0,
            max_bytes: None,
            expected_sha256: None,
        }),
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(30), &mut reader)
            .await
            .is_err()
    );
    drop(lease);
    assert_eq!(
        reader
            .await
            .expect("reader task")
            .expect("output read")
            .0
            .content,
        "leased-output"
    );
}

#[test]
fn byte_pressure_reclaims_old_spills_even_below_the_record_count_limit() {
    let directory = tempfile::tempdir().expect("store fixture");
    let jobs = ExecJobs::new(directory.path()).expect("store");
    for index in 1..=16 {
        let id = uuid::Uuid::from_u128(index).to_string();
        let record = JobRecord {
            workspace: directory.path().canonicalize().expect("workspace"),
            request_hash: "fixture".into(),
            snapshot: ExecJobSnapshot {
                delivery: None,
                operation_id: id.clone(),
                state: ExecJobState::Completed,
                revision: 2,
                outcome: None,
                error: None,
            },
        };
        jobs.write_record(&record).expect("terminal record");
        fs::File::options()
            .write(true)
            .open(jobs.path(&id).expect("path"))
            .expect("record")
            .set_modified(std::time::SystemTime::now() - Duration::from_secs(7200))
            .expect("old result");
        let output = jobs.directory.join(format!("{id}.output"));
        fs::create_dir(&output).expect("output directory");
        for stream in ["stdout", "stderr"] {
            fs::File::create(output.join(stream))
                .expect("sparse output")
                .set_len(64 * 1024 * 1024)
                .expect("bounded output stream");
        }
    }
    jobs.reserve_record_capacity(0, &crate::approval::ApprovalStore::default())
        .expect("old output reclaimed under byte pressure");
    assert!(
        jobs.receipts
            .read(&uuid::Uuid::from_u128(1).to_string())
            .expect("receipt")
            .is_some()
    );
}

#[test]
fn active_completion_reservations_and_private_registry_quota_are_preserved() {
    use std::os::unix::fs::PermissionsExt;
    let directory = tempfile::tempdir().expect("store fixture");
    let jobs = ExecJobs::new(directory.path()).expect("store");
    let approvals = crate::approval::ApprovalStore::default();
    jobs.reserve_record_capacity(14, &approvals)
        .expect("fifteenth reservation fits");
    assert!(
        jobs.reserve_record_capacity(15, &approvals).is_err(),
        "do not weaken existing 2GiB completion reservation"
    );
    for path in [
        jobs.directory.join("replay.sqlite"),
        jobs.directory.join("replay-required"),
    ] {
        assert_eq!(
            fs::metadata(path)
                .expect("private store")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    let connection =
        rusqlite::Connection::open(jobs.directory.join("replay.sqlite")).expect("fixture database");
    let count: i64 = connection
        .query_row("SELECT count(*) FROM receipts", [], |row| row.get(0))
        .expect("receipt table");
    assert_eq!(count, 0);
}
