use hzr_memory::{MemoryError, read_project_snapshot};
use rusqlite::{Connection, params};
use tempfile::tempdir;

const PROJECT_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const PROJECT_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

#[test]
fn missing_canonical_store_is_unavailable_instead_of_ready_and_empty() {
    let directory = tempdir().expect("temporary directory");
    let error = read_project_snapshot(&directory.path().join("missing.db"), PROJECT_A)
        .expect_err("an absent canonical store cannot prove an empty project");
    assert!(matches!(error, MemoryError::SnapshotUnavailable));
}

#[test]
fn project_snapshot_is_scoped_bounded_and_content_free() {
    let directory = tempdir().expect("temporary directory");
    let database = directory.path().join("memories.db");
    let connection = Connection::open(&database).expect("fixture database");
    connection
        .execute_batch(
            "CREATE TABLE memories (
                id TEXT PRIMARY KEY,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                last_accessed TEXT,
                access_count INTEGER NOT NULL,
                weight REAL NOT NULL,
                topic TEXT NOT NULL,
                summary TEXT NOT NULL,
                raw_excerpt TEXT,
                keywords TEXT NOT NULL,
                importance TEXT NOT NULL,
                source_type TEXT,
                source_data TEXT,
                related_ids TEXT NOT NULL,
                summary_hash TEXT,
                embedding BLOB
             );",
        )
        .expect("fixture schema");
    for (id, topic, related_ids, summary) in [
        (
            "a1",
            format!("decisions-{PROJECT_A}"),
            r#"["a2"]"#,
            "private decision",
        ),
        (
            "a2",
            format!("architecture-{PROJECT_A}"),
            r#"["a1"]"#,
            "private architecture",
        ),
        (
            "b1",
            format!("decisions-{PROJECT_B}"),
            "[]",
            "foreign private data",
        ),
        (
            "global",
            "preferences-global".into(),
            "[]",
            "global private data",
        ),
    ] {
        connection
            .execute(
                "INSERT INTO memories (
                    id, created_at, updated_at, last_accessed, access_count, weight,
                    topic, summary, raw_excerpt, keywords, importance, source_type,
                    source_data, related_ids, summary_hash, embedding
                 ) VALUES (?1, '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z', NULL,
                           1, 0.8, ?2, ?3, NULL, '[]', 'medium', NULL, NULL, ?4, NULL, NULL)",
                params![id, topic, summary, related_ids],
            )
            .expect("fixture record");
    }
    for index in 0..70 {
        connection
            .execute(
                "INSERT INTO memories (
                    id, created_at, updated_at, last_accessed, access_count, weight,
                    topic, summary, raw_excerpt, keywords, importance, source_type,
                    source_data, related_ids, summary_hash, embedding
                 ) VALUES (?1, '2026-08-01T00:00:00Z', '2026-08-01T00:00:00Z', NULL,
                           1, 0.5, ?2, 'private bounded data', NULL, '[]', 'medium',
                           NULL, NULL, '[]', NULL, NULL)",
                params![
                    format!("extra-{index}"),
                    format!("topic{index}-{PROJECT_A}")
                ],
            )
            .expect("bounded fixture record");
    }
    drop(connection);

    let snapshot = read_project_snapshot(&database, PROJECT_A).expect("project snapshot");

    assert_eq!(snapshot.memory_count, 72);
    assert_eq!(snapshot.visible_memory_count, 72);
    assert_eq!(snapshot.hidden_memory_count, 0);
    assert_eq!(snapshot.topics.len(), 64);
    assert!(snapshot.truncated);
    assert_eq!(
        snapshot.edges.len(),
        1,
        "reciprocal links collapse into one edge"
    );
    assert!(snapshot.topics.iter().all(|topic| {
        !topic.id.contains(PROJECT_A)
            && !topic.id.contains(PROJECT_B)
            && !topic.label.contains(PROJECT_A)
    }));
    let encoded = serde_json::to_string(&snapshot).expect("snapshot serializes");
    assert!(!encoded.contains("private"));
    assert!(!encoded.contains(PROJECT_A));
    assert!(!encoded.contains(PROJECT_B));
}
