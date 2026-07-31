use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use directories::ProjectDirs;
use fs2::FileExt;
use rusqlite::backup::Backup;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

// Only durable user-authored memory surfaces migrate. Hook telemetry, pending raw
// extraction payloads, and derived code-area observations intentionally stay in the
// retained source snapshot instead of entering HZR's explicit-memory store.
const TABLES: [&str; 8] = [
    "memoirs",
    "sessions",
    "memories",
    "concepts",
    "concept_links",
    "facts",
    "feedback",
    "messages",
];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LegacyMemorySource {
    pub path: PathBuf,
    pub size_bytes: u64,
    pub sha256: String,
    pub rows_by_table: BTreeMap<String, u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LegacyMemoryMigration {
    pub source: LegacyMemorySource,
    pub source_backup: PathBuf,
    pub canonical_backup: PathBuf,
    pub manifest_path: PathBuf,
    pub target_topic: String,
    pub imported_rows: u64,
    pub imported_by_table: BTreeMap<String, u64>,
    pub changed: bool,
}

pub fn discover_legacy_icm_database() -> Option<PathBuf> {
    let path = ProjectDirs::from("dev", "icm", "icm")?
        .data_local_dir()
        .join("memories.db");
    path.is_file().then_some(path)
}

pub fn inspect(path: &Path) -> Result<LegacyMemorySource> {
    let path = path
        .canonicalize()
        .with_context(|| format!("resolve legacy ICM database {}", path.display()))?;
    let connection = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("open legacy ICM database {} read-only", path.display()))?;
    ensure_integrity(&connection, "legacy ICM")?;
    let mut rows_by_table = BTreeMap::new();
    for table in TABLES {
        if table_exists(&connection, "main", table)? {
            let count =
                connection.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get::<_, u64>(0)
                })?;
            rows_by_table.insert(table.to_owned(), count);
        }
    }
    let metadata = fs::metadata(&path)?;
    Ok(LegacyMemorySource {
        sha256: sha256_file(&path)?,
        path,
        size_bytes: metadata.len(),
        rows_by_table,
    })
}

pub fn migrate(
    source_path: &Path,
    target_path: &Path,
    migration_root: &Path,
    project: &str,
) -> Result<LegacyMemoryMigration> {
    if project.len() != 64 || !project.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("memory migration project identity must be a 64-character hexadecimal digest");
    }
    let target_topic = format!("legacy-import-{}", project.to_ascii_lowercase());
    let target_parent = target_path
        .parent()
        .context("canonical ICM database has no parent directory")?;
    fs::create_dir_all(target_parent.join("runtime"))?;
    let lock_path = target_parent.join("runtime/supervisor.lock");
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)?;
    lock.try_lock_exclusive().with_context(|| {
        format!(
            "canonical ICM is active at {}; stop the HZR daemon before migration",
            lock_path.display()
        )
    })?;
    if !target_path.is_file() {
        bail!(
            "canonical ICM database {} is absent; start HZR once before migration",
            target_path.display()
        );
    }

    let snapshots = migration_root.join("snapshots");
    let manifests = migration_root.join("manifests");
    fs::create_dir_all(&snapshots)?;
    fs::create_dir_all(&manifests)?;
    let source_backup = persistent_snapshot(source_path, &snapshots, "icm-legacy")?;
    let source = inspect(&source_backup)?;
    let manifest_path = manifests.join(format!("icm-legacy-{}.json", source.sha256));
    if manifest_path.is_file() {
        let bytes = fs::read(&manifest_path)?;
        let mut report: LegacyMemoryMigration = serde_json::from_slice(&bytes)?;
        if report.source.sha256 != source.sha256
            || sha256_file(&report.source_backup)? != source.sha256
            || report.target_topic != target_topic
        {
            bail!("legacy ICM migration manifest failed re-attestation");
        }
        report.changed = false;
        report.imported_rows = 0;
        report.imported_by_table.clear();
        return Ok(report);
    }

    let canonical_backup = persistent_snapshot(target_path, &snapshots, "icm-canonical")?;
    let mut target = Connection::open(target_path)?;
    target.busy_timeout(Duration::from_secs(5))?;
    ensure_integrity(&target, "canonical ICM")?;
    target.execute(
        "ATTACH DATABASE ?1 AS legacy_icm",
        [source_backup.to_string_lossy().as_ref()],
    )?;
    let merge_result = merge_tables(&mut target, &target_topic);
    let detach_result = target.execute_batch("DETACH DATABASE legacy_icm");
    let imported_by_table = merge_result?;
    detach_result?;
    ensure_integrity(&target, "merged canonical ICM")?;
    let imported_rows = imported_by_table.values().sum();
    let report = LegacyMemoryMigration {
        source,
        source_backup,
        canonical_backup,
        manifest_path: manifest_path.clone(),
        target_topic,
        imported_rows,
        imported_by_table,
        changed: imported_rows > 0,
    };
    atomic_json(&manifest_path, &report)?;
    FileExt::unlock(&lock)?;
    Ok(report)
}

fn merge_tables(target: &mut Connection, target_topic: &str) -> Result<BTreeMap<String, u64>> {
    let transaction = target.transaction()?;
    transaction.execute_batch("PRAGMA defer_foreign_keys = ON")?;
    let mut imported = BTreeMap::new();
    for table in TABLES {
        if !table_exists(&transaction, "main", table)?
            || !table_exists(&transaction, "legacy_icm", table)?
        {
            continue;
        }
        let target_columns = columns(&transaction, "main", table)?;
        let source_columns = columns(&transaction, "legacy_icm", table)?;
        let mut target_column_set = target_columns.clone();
        let mut source_column_set = source_columns;
        target_column_set.sort();
        source_column_set.sort();
        if target_column_set != source_column_set || !target_columns.iter().any(|name| name == "id")
        {
            bail!("legacy ICM table {table} is not schema-compatible with the canonical database");
        }
        reject_conflicting_ids(&transaction, table, &target_columns)?;
        let column_list = target_columns.join(", ");
        let selected = target_columns
            .iter()
            .map(|column| match (table, column.as_str()) {
                ("memories", "topic") => "?1".to_owned(),
                ("memories", "embedding") => "NULL".to_owned(),
                _ => format!("legacy_icm.{table}.{column}"),
            })
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "INSERT INTO main.{table} ({column_list}) \
             SELECT {selected} FROM legacy_icm.{table} \
             WHERE NOT EXISTS (SELECT 1 FROM main.{table} WHERE main.{table}.id = legacy_icm.{table}.id)"
        );
        let count = if table == "memories" {
            transaction.execute(&sql, params![target_topic])?
        } else {
            transaction.execute(&sql, [])?
        } as u64;
        imported.insert(table.to_owned(), count);
    }
    transaction.commit()?;
    Ok(imported)
}

fn reject_conflicting_ids(connection: &Connection, table: &str, columns: &[String]) -> Result<()> {
    let comparisons = columns
        .iter()
        .filter(|column| !(table == "memories" && matches!(column.as_str(), "topic" | "embedding")))
        .map(|column| format!("main.{table}.{column} IS legacy_icm.{table}.{column}"))
        .collect::<Vec<_>>()
        .join(" AND ");
    let sql = format!(
        "SELECT legacy_icm.{table}.id FROM legacy_icm.{table} \
         JOIN main.{table} ON main.{table}.id = legacy_icm.{table}.id \
         WHERE NOT ({comparisons}) LIMIT 1"
    );
    let conflict = connection
        .query_row(&sql, [], |row| row.get::<_, String>(0))
        .optional()?;
    if let Some(id) = conflict {
        bail!("legacy ICM table {table} conflicts with canonical row id {id}");
    }
    Ok(())
}

fn columns(connection: &Connection, schema: &str, table: &str) -> Result<Vec<String>> {
    let mut statement = connection.prepare(&format!("PRAGMA {schema}.table_info('{table}')"))?;
    Ok(statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

fn table_exists(connection: &Connection, schema: &str, table: &str) -> Result<bool> {
    Ok(connection.query_row(
        &format!(
            "SELECT EXISTS(SELECT 1 FROM {schema}.sqlite_master WHERE type='table' AND name=?1)"
        ),
        [table],
        |row| row.get(0),
    )?)
}

fn ensure_integrity(connection: &Connection, label: &str) -> Result<()> {
    let result: String = connection.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if result != "ok" {
        bail!("{label} database failed integrity_check: {result}");
    }
    Ok(())
}

fn persistent_snapshot(source: &Path, directory: &Path, prefix: &str) -> Result<PathBuf> {
    let temporary = tempfile::NamedTempFile::new_in(directory)?;
    let source_connection = Connection::open_with_flags(
        source,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let mut snapshot_connection = Connection::open(temporary.path())?;
    {
        let backup = Backup::new(&source_connection, &mut snapshot_connection)?;
        backup.run_to_completion(128, Duration::from_millis(1), None)?;
    }
    drop(snapshot_connection);
    drop(source_connection);
    let sha256 = sha256_file(temporary.path())?;
    let path = directory.join(format!("{prefix}-{sha256}.sqlite"));
    if path.exists() {
        if sha256_file(&path)? != sha256 {
            bail!(
                "content-addressed snapshot {} failed re-attestation",
                path.display()
            );
        }
    } else {
        temporary.persist(&path)?;
    }
    Ok(path)
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(hex::encode(digest.finalize()))
}

fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().context("manifest has no parent directory")?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    temporary.write_all(&bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROJECT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn database(path: &Path, rows: &[(&str, &str, &str)]) {
        let connection = Connection::open(path).expect("create fixture database");
        connection
            .execute_batch(
                "CREATE TABLE memories (
                    id TEXT PRIMARY KEY,
                    topic TEXT NOT NULL,
                    summary TEXT NOT NULL,
                    embedding BLOB
                 );
                 CREATE TABLE facts (id TEXT PRIMARY KEY, value TEXT NOT NULL);",
            )
            .expect("create memory schema");
        for (id, topic, summary) in rows {
            connection
                .execute(
                    "INSERT INTO memories (id, topic, summary, embedding) VALUES (?1, ?2, ?3, X'0102')",
                    params![id, topic, summary],
                )
                .expect("insert fixture memory");
            connection
                .execute(
                    "INSERT INTO facts (id, value) VALUES (?1, ?2)",
                    params![format!("fact-{id}"), summary],
                )
                .expect("insert fixture fact");
        }
    }

    #[test]
    fn migration_snapshots_merges_and_is_idempotent() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let source = temp.path().join("legacy.sqlite");
        let target = temp.path().join("canonical/memories.db");
        fs::create_dir_all(target.parent().expect("target parent")).expect("target directory");
        database(&source, &[("legacy-id", "old-topic", "legacy content")]);
        database(
            &target,
            &[("canonical-id", "current-topic", "current content")],
        );
        let source_before = sha256_file(&source).expect("source digest");

        let first = migrate(&source, &target, &temp.path().join("migrations"), PROJECT)
            .expect("first migration");
        assert!(first.changed);
        assert_eq!(first.imported_rows, 2);
        assert!(first.source_backup.is_file());
        assert!(first.canonical_backup.is_file());
        assert!(first.manifest_path.is_file());
        assert_eq!(sha256_file(&source).expect("source digest"), source_before);
        let target_connection = Connection::open(&target).expect("open target");
        let imported: (String, Option<Vec<u8>>) = target_connection
            .query_row(
                "SELECT topic, embedding FROM memories WHERE id='legacy-id'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("imported memory");
        assert_eq!(imported.0, format!("legacy-import-{PROJECT}"));
        assert!(imported.1.is_none(), "derived embeddings must be rebuilt");

        let target_before_retry = sha256_file(&target).expect("target digest");
        let second = migrate(&source, &target, &temp.path().join("migrations"), PROJECT)
            .expect("idempotent migration");
        assert!(!second.changed);
        assert_eq!(second.imported_rows, 0);
        assert_eq!(
            sha256_file(&target).expect("target digest"),
            target_before_retry
        );
    }

    #[test]
    fn migration_rejects_conflicting_ids_without_partial_import() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let source = temp.path().join("legacy.sqlite");
        let target = temp.path().join("canonical/memories.db");
        fs::create_dir_all(target.parent().expect("target parent")).expect("target directory");
        database(&source, &[("same-id", "old-topic", "legacy content")]);
        database(
            &target,
            &[("same-id", "current-topic", "different content")],
        );
        let before = sha256_file(&target).expect("target digest");

        let error = migrate(&source, &target, &temp.path().join("migrations"), PROJECT)
            .expect_err("conflict must fail closed");
        assert!(
            error
                .to_string()
                .contains("conflicts with canonical row id same-id")
        );
        assert_eq!(sha256_file(&target).expect("target digest"), before);
    }

    #[test]
    fn migration_refuses_an_active_canonical_owner() {
        let temp = tempfile::tempdir().expect("temporary directory");
        let source = temp.path().join("legacy.sqlite");
        let target = temp.path().join("canonical/memories.db");
        let runtime = target.parent().expect("target parent").join("runtime");
        fs::create_dir_all(&runtime).expect("runtime directory");
        database(&source, &[("legacy-id", "old-topic", "legacy content")]);
        database(&target, &[]);
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(runtime.join("supervisor.lock"))
            .expect("lock file");
        lock.try_lock_exclusive().expect("hold owner lock");

        let error = migrate(&source, &target, &temp.path().join("migrations"), PROJECT)
            .expect_err("active owner must block migration");
        assert!(error.to_string().contains("canonical ICM is active"));
    }
}
