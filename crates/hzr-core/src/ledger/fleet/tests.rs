use super::*;
use crate::privacy_identity_hash;

fn insert(
    connection: &Connection,
    timestamp: i64,
    project: &str,
    tokens: (i64, i64),
    route: &str,
    stage: &str,
    policy: &str,
) {
    let (input, output) = tokens;
    connection.execute(
        "INSERT INTO commands (timestamp,original_cmd,rtk_cmd,input_tokens,output_tokens,saved_tokens,savings_pct,project_hash,agent,operation_family,route,accounting_stage,measurement,accounting_policy_version,command_hash,session_hash)
         VALUES (datetime(?1,'unixepoch'),'SECRET argv content','rtk cargo',?2,?3,?2-?3,0,?4,'codex:SECRET-agent','cargo',?5,?6,'estimated',?7,'command-hash','session-hash')",
        params![timestamp,input,output,project,route,stage,policy],
    ).expect("fixture row");
}

fn query() -> FleetStatsQuery<'static> {
    FleetStatsQuery {
        since_unix_seconds: 100,
        until_unix_seconds: 200,
        project_id: None,
        include_legacy_versions: false,
    }
}

#[test]
fn fleet_window_is_exact_private_and_reconciles_all_dimensions() {
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join("hzr.sqlite");
    let ledger = Ledger::open(&path).expect("ledger");
    let first = privacy_identity_hash("project", "/deleted/SECRET-project");
    let second = privacy_identity_hash("project", "/active/project");
    let policy = super::super::CURRENT_ACCOUNTING_POLICY_VERSION;
    for timestamp in [99, 200] {
        insert(
            &ledger.connection,
            timestamp,
            &first,
            (999, 1),
            "optimized",
            "internal_transport",
            policy,
        );
    }
    insert(
        &ledger.connection,
        100,
        &first,
        (100, 20),
        "optimized",
        "internal_transport",
        policy,
    );
    insert(
        &ledger.connection,
        101,
        &first,
        (100, 60),
        "optimized",
        "internal_transport",
        policy,
    );
    insert(
        &ledger.connection,
        150,
        &second,
        (80, 100),
        "optimized",
        "internal_transport",
        policy,
    );
    insert(
        &ledger.connection,
        151,
        &second,
        (0, 999),
        "optimized",
        "final_delivery",
        policy,
    );
    insert(
        &ledger.connection,
        160,
        &second,
        (100, 100),
        "bypassed",
        "internal_transport",
        policy,
    );
    insert(
        &ledger.connection,
        170,
        &second,
        (0, 40),
        "native_unaccounted",
        "internal_transport",
        policy,
    );
    insert(
        &ledger.connection,
        171,
        &second,
        (80, 10),
        "optimized",
        "internal_transport",
        "obsolete",
    );
    let mut report = Ledger::fleet_stats_read_only(&path, query()).expect("snapshot");
    assert_eq!(report.totals.recorded_operations, 6);
    assert_eq!(report.totals.measured_operations, 4);
    assert_eq!(report.totals.net_avoided_tokens_estimated, 100);
    assert_eq!(report.totals.stage_excluded_operations, 1);
    assert_eq!(report.totals.native_unaccounted_operations, 1);
    assert_eq!(report.totals.excluded_legacy_operations, 1);
    assert_eq!(report.totals.repeated_after_filter_operations, 1);
    assert_eq!(report.totals.repeated_after_filter_tokens_estimated, 60);
    for metrics in [
        report
            .by_project
            .iter()
            .map(|row| &row.metrics)
            .collect::<Vec<_>>(),
        report.by_host.iter().map(|row| &row.metrics).collect(),
        report.by_family.iter().map(|row| &row.metrics).collect(),
        report.groups.iter().map(|row| &row.metrics).collect(),
    ] {
        let mut sum = FleetMetrics::default();
        for row in metrics {
            sum.add(row).expect("sum");
        }
        assert_eq!(sum, report.totals);
    }
    report.include_registered_project(first.clone(), false);
    report.include_registered_project(privacy_identity_hash("project", "/zero"), true);
    assert_eq!(report.by_project.len(), 3);
    assert_eq!(
        report
            .by_project
            .iter()
            .find(|row| row.project_id == first)
            .expect("deleted")
            .workspace_exists,
        Some(false)
    );
    let encoded = serde_json::to_string(&report).expect("JSON");
    assert!(!encoded.contains("SECRET"));
    assert!(!encoded.contains("/deleted"));
    assert!(!report.economic_claim_ready);
    assert!(report.host_coverage.starts_with("unknown"));
    let scoped = Ledger::fleet_stats_read_only(
        &path,
        FleetStatsQuery {
            project_id: Some(&first),
            ..query()
        },
    )
    .expect("historical ID");
    assert_eq!(scoped.totals.recorded_operations, 2);
    assert_eq!(scoped.totals.net_avoided_tokens_estimated, 120);
    assert_eq!(scoped.provider_tasks, None);
    let exported = directory.path().join("fleet.json");
    report.export_atomic(&exported).expect("export");
    let reopened: FleetStatsSnapshot =
        serde_json::from_slice(&std::fs::read(&exported).expect("read export")).expect("decode");
    assert_eq!(report, reopened);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(exported)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}

#[test]
fn fleet_read_transaction_excludes_concurrent_writer_and_missing_ledger_stays_absent() {
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join("hzr.sqlite");
    let empty = Ledger::fleet_stats_read_only(&path, query()).expect("absent snapshot");
    assert!(!empty.ledger_present);
    assert!(!path.exists());
    let ledger = Ledger::open(&path).expect("ledger");
    let id = privacy_identity_hash("project", "/project");
    let policy = super::super::CURRENT_ACCOUNTING_POLICY_VERSION;
    insert(
        &ledger.connection,
        100,
        &id,
        (100, 20),
        "optimized",
        "internal_transport",
        policy,
    );
    let reader =
        Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).expect("reader");
    let transaction = reader.unchecked_transaction().expect("transaction");
    let _: u64 = transaction
        .query_row("SELECT COUNT(*) FROM commands", [], |row| row.get(0))
        .expect("pin snapshot");
    insert(
        &ledger.connection,
        101,
        &id,
        (100, 20),
        "optimized",
        "internal_transport",
        policy,
    );
    let snapshot = fleet_snapshot(&transaction, query()).expect("same snapshot");
    assert_eq!(snapshot.totals.recorded_operations, 1);
    transaction.commit().expect("commit");
    assert_eq!(
        Ledger::fleet_stats_read_only(&path, query())
            .expect("later snapshot")
            .totals
            .recorded_operations,
        2
    );
}

#[test]
fn fleet_rejects_invalid_windows_and_project_ids_before_opening() {
    let path = Path::new("/definitely/absent/ledger.sqlite");
    assert!(
        Ledger::fleet_stats_read_only(
            path,
            FleetStatsQuery {
                until_unix_seconds: 100,
                ..query()
            }
        )
        .is_err()
    );
    assert!(
        Ledger::fleet_stats_read_only(
            path,
            FleetStatsQuery {
                since_unix_seconds: -1,
                ..query()
            }
        )
        .is_err()
    );
    assert!(
        Ledger::fleet_stats_read_only(
            path,
            FleetStatsQuery {
                project_id: Some("/private/path"),
                ..query()
            }
        )
        .is_err()
    );
}
