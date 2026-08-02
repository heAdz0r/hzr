//! The density codec must be able to earn its place in the ledger.
//!
//! `hzr-codec` was reachable only through `hzr codec compile` and recorded nothing, so the
//! `codec` subsystem could never appear in `hzr stats` no matter how much it saved. A
//! capability that cannot be measured cannot be justified, and one that is never called is
//! indistinguishable from one that does not work.

use hzr_core::{Ledger, OperationContext, OperationSubsystem, classify_operation};
use tempfile::tempdir;

#[test]
fn test_a_recorded_operation_reaches_the_efficiency_summary() {
    let directory = tempdir().expect("temp directory");
    let ledger = Ledger::open(&directory.path().join("hzr.sqlite")).expect("ledger open");

    ledger
        .record_operation("hzr codec compile", "hzr codec adaptive", 1_200, 400, 3, "")
        .expect("record operation");
    let summary = ledger.efficiency_summary().expect("efficiency summary");

    assert_eq!(summary.operations, 1);
    assert_eq!(summary.baseline_tokens_estimated, 1_200);
    assert_eq!(summary.delivered_tokens_estimated, 400);
    assert_eq!(summary.net_avoided_tokens_estimated, 800);
    let command = summary
        .by_command
        .first()
        .expect("the operation is grouped by command");
    assert_eq!(command.command, "hzr codec adaptive");
    assert_eq!(
        classify_operation(&command.command).subsystem,
        OperationSubsystem::Codec,
        "a recorded codec operation must land in the codec subsystem"
    );
}

/// A transform that grew the text is a regression, and the ledger has to be able to say so
/// rather than clamping it to zero.
#[test]
fn test_an_operation_that_grew_the_output_is_recorded_as_a_regression() {
    let directory = tempdir().expect("temp directory");
    let ledger = Ledger::open(&directory.path().join("hzr.sqlite")).expect("ledger open");

    ledger
        .record_operation("hzr codec compile", "hzr codec compact", 100, 140, 1, "")
        .expect("record operation");
    let summary = ledger.efficiency_summary().expect("efficiency summary");

    assert_eq!(summary.regression_tokens_estimated, 40);
    assert_eq!(summary.gross_avoided_tokens_estimated, 0);
    assert_eq!(summary.net_avoided_tokens_estimated, -40);
}

#[test]
fn test_a_recorded_operation_is_scoped_to_its_project() {
    let directory = tempdir().expect("temp directory");
    let path = directory.path().join("hzr.sqlite");
    let ledger = Ledger::open(&path).expect("ledger open");

    ledger
        .record_operation(
            "hzr codec compile",
            "hzr codec adaptive",
            500,
            100,
            2,
            "/work/project",
        )
        .expect("record operation");
    let activity = ledger
        .project_activity("/work/project")
        .expect("project activity");

    assert_eq!(activity.operations, 1);
    assert_eq!(activity.optimized_operations, 1);
    assert_eq!(activity.raw_operations, 0);
}

#[test]
fn test_recent_activity_preserves_inspectable_request_context() {
    let directory = tempdir().expect("temp directory");
    let path = directory.path().join("hzr.sqlite");
    let ledger = Ledger::open(&path).expect("ledger open");

    ledger
        .record_operation_with_context(
            "rg --files visualizer/src",
            "hzr rtk -- raw rg --files visualizer/src",
            120,
            120,
            7,
            OperationContext {
                project_path: "/work/project/visualizer",
                agent: Some("codex"),
                session_id: Some("thread-123"),
            },
        )
        .expect("record attributed operation");

    let activity = ledger
        .project_activity("/work/project")
        .expect("project activity");
    let operation = activity
        .recent_operations
        .first()
        .expect("recent operation");

    assert!(operation.ledger_id > 0);
    assert_eq!(operation.original_command, "rg --files visualizer/src");
    assert_eq!(
        operation.recorded_command,
        "hzr rtk -- raw rg --files visualizer/src"
    );
    assert_eq!(operation.working_directory, "/work/project/visualizer");
    assert_eq!(operation.agent.as_deref(), Some("codex"));
    assert_eq!(operation.session_id.as_deref(), Some("thread-123"));
}

#[test]
fn test_raw_routes_receive_zero_credit_even_when_recorded_counts_differ() {
    let directory = tempdir().expect("temp directory");
    let ledger = Ledger::open(&directory.path().join("hzr.sqlite")).expect("ledger open");

    ledger
        .record_operation("cat useful", "read useful", 100, 20, 2, "/work/project")
        .expect("optimized operation");
    ledger
        .record_operation(
            "rg noisy",
            "hzr rtk -- raw rg noisy",
            100,
            1,
            3,
            "/work/project",
        )
        .expect("raw operation");

    let summary = ledger.efficiency_summary().expect("efficiency summary");
    assert_eq!(summary.baseline_tokens_estimated, 101);
    assert_eq!(summary.delivered_tokens_estimated, 21);
    assert_eq!(summary.gross_avoided_tokens_estimated, 80);
    assert_eq!(summary.regression_tokens_estimated, 0);
    assert_eq!(summary.net_avoided_tokens_estimated, 80);

    let raw = summary
        .by_command
        .iter()
        .find(|command| command.command.contains("-- raw"))
        .expect("raw command summary");
    assert_eq!(raw.baseline_tokens_estimated, 1);
    assert_eq!(raw.delivered_tokens_estimated, 1);
    assert_eq!(raw.gross_avoided_tokens_estimated, 0);
    assert_eq!(raw.regression_tokens_estimated, 0);
    assert_eq!(raw.net_avoided_tokens_estimated, 0);
}
