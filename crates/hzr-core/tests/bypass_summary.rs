//! Bypassed operations must be countable on their own.
//!
//! The lifetime reduction ratio stays healthy even when half of the delivered tokens
//! never reached the optimizer, because a bypassed row contributes equally to the
//! baseline and to the delivered total. Only an explicit bypass query makes that visible.

use hzr_core::Ledger;
use rusqlite::Connection;
use tempfile::tempdir;

fn seed(path: &std::path::Path, rows: &[(&str, &str, u64, u64, &str)]) {
    Ledger::open(path).expect("ledger open");
    let connection = Connection::open(path).expect("open for seeding");
    for (timestamp, command, input, output, project) in rows {
        connection
            .execute(
                "INSERT INTO commands (
                    timestamp, original_cmd, rtk_cmd, input_tokens, output_tokens,
                    saved_tokens, savings_pct, exec_time_ms, project_path,
                    producer_version, accounting_policy_version
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                           'test', 'privacy_typed_v1')",
                rusqlite::params![
                    timestamp,
                    command,
                    command,
                    input,
                    output,
                    input.saturating_sub(*output),
                    0.0_f64,
                    5_i64,
                    project,
                ],
            )
            .expect("seed row");
    }
}

#[test]
fn test_bypass_summary_separates_bypassed_tokens_from_optimized_tokens() {
    let directory = tempdir().expect("temp directory");
    let path = directory.path().join("hzr.sqlite");
    seed(
        &path,
        &[
            // Optimized: filtering removed 900 of 1000 tokens.
            ("2026-08-01 09:00:00", "rtk read src/lib.rs", 1_000, 100, ""),
            // Bypassed: delivered == baseline, zero savings, and today it is invisible.
            (
                "2026-08-01 09:05:00",
                "rtk proxy sed -n 1,400p src/lib.rs",
                800,
                800,
                "",
            ),
            (
                "2026-08-01 09:06:00",
                "rtk proxy rg -n needle crates",
                200,
                200,
                "",
            ),
        ],
    );
    let ledger = Ledger::open(&path).expect("ledger open");

    let summary = ledger.bypass_summary().expect("bypass summary");

    assert_eq!(summary.lifetime.operations, 2);
    assert_eq!(summary.lifetime.total_operations, 3);
    assert_eq!(summary.lifetime.delivered_tokens_estimated, 1_000);
    assert_eq!(summary.lifetime.total_delivered_tokens_estimated, 1_100);
}

#[test]
fn test_bypass_summary_groups_by_tool_and_carries_the_replacement() {
    let directory = tempdir().expect("temp directory");
    let path = directory.path().join("hzr.sqlite");
    seed(
        &path,
        &[
            (
                "2026-08-01 09:00:00",
                "rtk proxy sed -n 10,20p a.rs",
                100,
                100,
                "",
            ),
            (
                "2026-08-01 09:01:00",
                "rtk proxy sed -n 30,40p b.rs",
                300,
                300,
                "",
            ),
            (
                "2026-08-01 09:02:00",
                "rtk proxy cargo clippy --workspace",
                50,
                50,
                "",
            ),
        ],
    );
    let ledger = Ledger::open(&path).expect("ledger open");

    let summary = ledger.bypass_summary().expect("bypass summary");
    let sed = summary
        .by_tool
        .iter()
        .find(|tool| tool.tool == "sed")
        .expect("sed bucket");

    assert_eq!(sed.executions, 2);
    assert_eq!(sed.delivered_tokens_estimated, 400);
    assert_eq!(sed.example_command, "rtk raw sed");
    assert_eq!(
        sed.replacement, None,
        "privacy-safe aggregates never retain path-bearing suggestions"
    );

    let cargo = summary
        .by_tool
        .iter()
        .find(|tool| tool.tool == "cargo")
        .expect("cargo bucket");
    assert_eq!(
        cargo.replacement, None,
        "a bypass without an equivalent must not invent one"
    );

    assert!(
        summary
            .by_tool
            .first()
            .is_some_and(|tool| tool.tool == "sed"),
        "buckets are ranked by delivered tokens so the costliest bypass leads"
    );
}

#[test]
fn test_bypass_summary_on_a_clean_ledger_is_empty_rather_than_an_error() {
    let directory = tempdir().expect("temp directory");
    let ledger = Ledger::open(&directory.path().join("empty.sqlite")).expect("ledger open");

    let summary = ledger.bypass_summary().expect("bypass summary");

    assert_eq!(summary.lifetime.operations, 0);
    assert_eq!(summary.lifetime.total_operations, 0);
    assert!(summary.by_tool.is_empty());
}
