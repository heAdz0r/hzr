use std::process::Command;
use std::sync::{Arc, Barrier};
use std::thread;

use hzr_core::{Config, Ledger, StatsQuery};
use rusqlite::Connection;
use tempfile::tempdir;

#[test]
fn concurrent_stats_readers_and_writer_share_a_read_only_snapshot_without_lock_errors() {
    let directory = tempdir().expect("temp directory");
    let config_path = directory.path().join("config.toml");
    let config = Config {
        data_dir: directory.path().join("data"),
        ..Default::default()
    };
    config.ensure_layout().expect("data layout");
    config.write(&config_path).expect("config");
    let ledger_path = config.data_dir.join("ledger/hzr.sqlite");
    Ledger::open(&ledger_path).expect("seed ledger");
    let schema_before: u64 = Connection::open(&ledger_path)
        .expect("schema reader")
        .query_row("PRAGMA schema_version", [], |row| row.get(0))
        .expect("schema version");

    const READERS: usize = 32;
    const ROUNDS: usize = 10;
    let barrier = Arc::new(Barrier::new(READERS + 1));
    let writer_barrier = Arc::clone(&barrier);
    let writer_path = ledger_path.clone();
    let writer = thread::spawn(move || {
        let ledger = Ledger::open(&writer_path).expect("writer ledger");
        writer_barrier.wait();
        for round in 0..ROUNDS {
            ledger
                .record_operation("writer", "writer", 1, 1, round as u64, "/work")
                .expect("concurrent operation write");
        }
    });

    let handles = (0..READERS)
        .map(|_| {
            let barrier = Arc::clone(&barrier);
            let ledger_path = ledger_path.clone();
            thread::spawn(move || {
                barrier.wait();
                for _ in 0..ROUNDS {
                    Ledger::stats_collection_read_only(&ledger_path, StatsQuery::default())
                        .expect("read-only stats snapshot");
                }
            })
        })
        .collect::<Vec<_>>();

    for handle in handles {
        handle.join().expect("stats reader");
    }
    writer.join().expect("stats writer");

    let schema_after: u64 = Connection::open(&ledger_path)
        .expect("schema reader")
        .query_row("PRAGMA schema_version", [], |row| row.get(0))
        .expect("schema version");
    assert_eq!(
        schema_after, schema_before,
        "stats readers must not migrate schema"
    );

    let output = Command::new(env!("CARGO_BIN_EXE_hzr"))
        .args(["--config"])
        .arg(&config_path)
        .args(["--json", "stats"])
        .output()
        .expect("run public stats command");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect("stats JSON");
    assert_eq!(report["direct_savings"]["operations"], ROUNDS);
}

#[test]
fn read_only_stats_refuses_an_old_schema_without_migrating_it() {
    let directory = tempdir().expect("temp directory");
    let ledger_path = directory.path().join("legacy.sqlite");
    let connection = Connection::open(&ledger_path).expect("legacy database");
    connection
        .execute_batch(
            "CREATE TABLE commands (
                id INTEGER PRIMARY KEY,
                timestamp TEXT NOT NULL,
                original_cmd TEXT NOT NULL,
                rtk_cmd TEXT NOT NULL,
                input_tokens INTEGER NOT NULL,
                output_tokens INTEGER NOT NULL,
                saved_tokens INTEGER NOT NULL,
                savings_pct REAL NOT NULL
            );",
        )
        .expect("legacy schema");
    let schema_before: u64 = connection
        .query_row("PRAGMA schema_version", [], |row| row.get(0))
        .expect("schema version");
    let columns_before: Vec<String> = connection
        .prepare("PRAGMA table_info(commands)")
        .expect("table info")
        .query_map([], |row| row.get(1))
        .expect("columns")
        .collect::<Result<_, _>>()
        .expect("column names");
    drop(connection);

    Ledger::stats_collection_read_only(&ledger_path, StatsQuery::default())
        .expect_err("old schemas must be refused, not silently migrated by a reader");

    let connection = Connection::open(&ledger_path).expect("legacy database after stats");
    let schema_after: u64 = connection
        .query_row("PRAGMA schema_version", [], |row| row.get(0))
        .expect("schema version");
    let columns_after: Vec<String> = connection
        .prepare("PRAGMA table_info(commands)")
        .expect("table info")
        .query_map([], |row| row.get(1))
        .expect("columns")
        .collect::<Result<_, _>>()
        .expect("column names");
    assert_eq!(schema_after, schema_before);
    assert_eq!(columns_after, columns_before);
}
