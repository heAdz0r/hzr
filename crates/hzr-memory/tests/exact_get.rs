use hzr_memory::read_memory_by_id;
use rusqlite::{Connection, params};

#[test]
fn exact_get_preserves_content_and_confines_namespaces() {
    let directory = tempfile::tempdir().expect("temporary store");
    let database = directory.path().join("memory.db");
    let connection = Connection::open(&database).expect("store");
    connection.execute_batch("CREATE TABLE memories (id TEXT PRIMARY KEY, topic TEXT, updated_at TEXT, summary TEXT, raw_excerpt TEXT)").expect("schema");
    let project = "a".repeat(64);
    let foreign = "b".repeat(64);
    for (id, topic) in [
        ("local", format!("architecture-{project}")),
        ("foreign", format!("architecture-{foreign}")),
        ("global", "preferences-global".into()),
        ("legacy", format!("legacy-import-{project}")),
    ] {
        connection
            .execute(
                "INSERT INTO memories VALUES (?1, ?2, 'revision', '  exact\ncontent  ', '')",
                params![id, topic],
            )
            .expect("fixture");
    }
    let record = read_memory_by_id(&database, &project, "local", false)
        .expect("lookup")
        .expect("owned record");
    assert_eq!(record.summary, "  exact\ncontent  ");
    assert_eq!(record.raw_excerpt.as_deref(), Some(""));
    for id in ["foreign", "global", "legacy", "absent", "' OR 1=1"] {
        assert!(
            read_memory_by_id(&database, &project, id, false)
                .expect("lookup")
                .is_none()
        );
    }
    assert!(
        read_memory_by_id(&database, &project, "global", true)
            .expect("global")
            .is_some()
    );
    assert!(
        read_memory_by_id(&database, &project, "local", true)
            .expect("local in global")
            .is_none()
    );
}
