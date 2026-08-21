use std::process::Command;

fn rtk_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_rtk"))
}

#[test]
fn ps_has_a_first_class_rewrite_and_bounded_command() {
    let rewrite = rtk_bin()
        .args(["rewrite", "ps aux"])
        .output()
        .expect("rewrite ps");
    assert!(rewrite.status.success());
    assert_eq!(String::from_utf8_lossy(&rewrite.stdout), "rtk ps aux");

    let help = rtk_bin().args(["ps", "--help"]).output().expect("ps help");
    assert!(help.status.success());
}

#[test]
fn hidden_rewrite_plan_dispatches_one_typed_json_object() {
    let output = rtk_bin()
        .args(["rewrite-plan", "ps aux"])
        .output()
        .expect("typed rewrite plan");
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).expect("plan JSON");
    assert_eq!(value["decision"], "rewrite");
    assert_eq!(value["proposed"], "rtk ps aux");
    assert_eq!(
        output.stdout.iter().filter(|byte| **byte == b'\n').count(),
        1
    );
}

#[test]
fn sqlite_route_rejects_writes_and_invalid_projection_before_execution() {
    let write = rtk_bin()
        .args(["sqlite3", "missing.db", "DELETE FROM operations"])
        .output()
        .expect("reject sqlite write");
    assert!(!write.status.success());
    assert!(String::from_utf8_lossy(&write.stderr).contains("SELECT statements only"));

    let projection = rtk_bin()
        .args([
            "sqlite3",
            "missing.db",
            "SELECT * FROM operations",
            "--columns",
            "id, secret",
        ])
        .output()
        .expect("reject unsafe projection");
    assert!(!projection.status.success());
    assert!(String::from_utf8_lossy(&projection.stderr).contains("invalid projected column"));
}

#[test]
fn tar_route_rejects_creation_and_extraction_before_execution() {
    for args in [
        ["tar", "-cf", "archive.tar", "src"],
        ["tar", "-xf", "archive.tar", "src"],
    ] {
        let output = rtk_bin().args(args).output().expect("reject tar mutation");
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("Ask/E10"));
    }
}

#[test]
fn remote_logs_route_rejects_shell_syntax_before_ssh() {
    let output = rtk_bin()
        .args(["logs", "host;id", "container"])
        .output()
        .expect("reject remote shell syntax");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("reject shell syntax"));
}

#[test]
fn w7_exact_routes_require_compatible_closed_reasons_before_execution() {
    let cases = [
        (vec!["sqlite3", ":memory:", "SELECT 1"], "complete_log"),
        (
            vec!["tar", "-tf", "definitely-missing-archive.tar"],
            "complete_log",
        ),
        (
            vec!["logs", "safe-host", "safe-container"],
            "machine_protocol",
        ),
    ];
    for (args, contradicted_reason) in cases {
        let missing = rtk_bin()
            .args(&args)
            .env("HZR_RAW_FIDELITY", "1")
            .output()
            .expect("reject missing fidelity reason");
        assert!(!missing.status.success());
        assert!(String::from_utf8_lossy(&missing.stderr).contains("closed HZR_RAW_FIDELITY_REASON"));

        let contradicted = rtk_bin()
            .args(&args)
            .env("HZR_RAW_FIDELITY", "1")
            .env("HZR_RAW_FIDELITY_REASON", contradicted_reason)
            .output()
            .expect("reject contradicted fidelity reason");
        assert!(!contradicted.status.success());
        assert!(!String::from_utf8_lossy(&contradicted.stderr).contains(contradicted_reason));
    }
}

#[test]
fn unknown_fidelity_reason_is_refused_without_echo() {
    let sentinel = "unknown-user-sentinel";
    let output = rtk_bin()
        .args(["sqlite3", ":memory:", "SELECT 1"])
        .env("HZR_RAW_FIDELITY", "1")
        .env("HZR_RAW_FIDELITY_REASON", sentinel)
        .output()
        .expect("reject unknown fidelity reason");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("exact fidelity refused"));
    assert!(!stderr.contains(sentinel));
}
