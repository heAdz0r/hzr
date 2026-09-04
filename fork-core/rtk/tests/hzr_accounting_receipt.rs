use std::fs;

#[test]
fn stdin_pipe_emits_only_an_internal_estimated_receipt() {
    use hzr_engine_contract::AccountingStage;
    use std::io::Write;
    use std::process::Stdio;

    let directory = tempfile::tempdir().expect("temporary directory");
    let receipts = directory.path().join("receipts.jsonl");
    let history = directory.path().join("must-not-exist.sqlite");
    let raw = format!("running 180 tests\n{}\ntest result: ok. 180 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n",
        (0..180).map(|n| format!("test module::case_{n} ... ok")).collect::<Vec<_>>().join("\n"));
    let mut child = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["pipe", "--filter", "cargo-test"])
        .env("RTK_DB_PATH", &history)
        .env("RTK_TRACKING_DISABLED", "0")
        .env("HZR_INTERNAL_ACCOUNTING_RECEIPT_JOURNAL", &receipts)
        .env(
            "HZR_INTERNAL_ACCOUNTING_CORRELATION",
            "0123456789abcdef0123456789abcdef",
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("pipe");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(raw.as_bytes())
        .expect("source");
    let output = child.wait_with_output().expect("wait");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.len() < raw.len());
    let journal = fs::read_to_string(receipts).expect("receipt");
    let receipt: EngineAccountingReceipt =
        serde_json::from_str(journal.trim()).expect("typed receipt");
    assert_eq!(
        receipt.attribution.stage,
        AccountingStage::InternalTransport
    );
    assert_eq!(receipt.measurement, AccountingMeasurement::Estimated);
    assert_eq!(receipt.route, AccountingRoute::Optimized);
    assert!(receipt.baseline_tokens > receipt.delivered_tokens);
    assert!(!journal.contains("module::case_"));
    assert!(!history.exists());
}

use std::process::Command;

use hzr_engine_contract::{
    AccountingMeasurement, AccountingOperationKind, AccountingRoute, EngineAccountingReceipt,
};

#[test]
fn hzr_receipt_mode_never_opens_the_rtk_history_database() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let source = directory.path().join("private-source.txt");
    let history = directory.path().join("must-not-exist.sqlite");
    let receipts = directory.path().join("accounting-receipts.jsonl");
    let failures = directory.path().join("accounting-failures.jsonl");
    let correlation = "0123456789abcdef0123456789abcdef";
    fs::write(&source, "private sentinel text\nsecond line\n").expect("source fixture");

    let output = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args([
            "read",
            source.to_str().expect("utf-8 fixture path"),
            "--level",
            "minimal",
        ])
        .env("RTK_DB_PATH", &history)
        .env("RTK_TRACKING_DISABLED", "0")
        .env("HZR_INTERNAL_ACCOUNTING_RECEIPT_JOURNAL", &receipts)
        .env("HZR_INTERNAL_ACCOUNTING_FAILURE_JOURNAL", &failures)
        .env("HZR_INTERNAL_ACCOUNTING_CORRELATION", correlation)
        .output()
        .expect("run managed read");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!history.exists(), "fork wrote the HZR-owned SQLite path");
    assert!(!directory.path().join("must-not-exist.sqlite-wal").exists());
    assert!(!directory.path().join("must-not-exist.sqlite-shm").exists());
    assert!(!failures.exists(), "receipt write unexpectedly failed");

    let journal = fs::read_to_string(&receipts).expect("receipt journal");
    assert!(!journal.contains("private sentinel text"));
    assert!(!journal.contains(source.to_str().expect("utf-8 fixture path")));
    let receipt: EngineAccountingReceipt =
        serde_json::from_str(journal.trim()).expect("typed receipt");
    assert_eq!(receipt.correlation_id, correlation);
    assert_eq!(receipt.sequence, 0);
    assert_eq!(receipt.measurement, AccountingMeasurement::Estimated);
    assert_eq!(receipt.route, AccountingRoute::Optimized);
    assert_eq!(receipt.attribution.operation, AccountingOperationKind::Read);
}
