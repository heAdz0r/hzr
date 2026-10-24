use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use super::JobRecord;

const MAX_RECEIPT_BYTES: u64 = 256 * 1024 * 1024;
const MAX_RECEIPT_RECORD_BYTES: usize = 16 * 1024;
const APPLICATION_ID: u32 = 0x485a_5245;
const SCHEMA: &str = "CREATE TABLE receipts (operation_id TEXT PRIMARY KEY NOT NULL, record BLOB NOT NULL) WITHOUT ROWID";

#[derive(Clone)]
pub(super) struct ReceiptStore {
    path: PathBuf,
}

impl ReceiptStore {
    pub(super) fn new(directory: &Path) -> io::Result<Self> {
        let store = Self {
            path: directory.join("replay.sqlite"),
        };
        let marker = directory.join("replay-required");
        let required = match private_file(&marker, false) {
            Ok(mut file) => {
                let mut version = String::new();
                Read::by_ref(&mut file)
                    .take(16)
                    .read_to_string(&mut version)?;
                if version != "1\n" {
                    return Err(io::Error::other("invalid execution receipt marker"));
                }
                true
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => false,
            Err(error) => return Err(error),
        };
        let initialize = match private_file(&store.path, !required) {
            Ok(file) => !required && file.metadata()?.len() == 0,
            Err(error) => {
                return Err(io::Error::other(format!(
                    "execution receipts unavailable: {error}"
                )));
            }
        };
        let connection = store.open_unvalidated()?;
        if initialize {
            connection.execute_batch(&format!("BEGIN IMMEDIATE; {SCHEMA}; PRAGMA application_id = {APPLICATION_ID}; PRAGMA user_version = 1; COMMIT;")).map_err(io::Error::other)?;
        }
        Self::validate(&connection)?;
        if !required {
            let mut file = private_file(&marker, true)?;
            file.write_all(b"1\n")?;
            file.sync_all()?;
            #[cfg(unix)]
            fs::File::open(directory)?.sync_all()?;
        }
        Ok(store)
    }

    fn validate(connection: &Connection) -> io::Result<()> {
        let application: u32 = connection
            .pragma_query_value(None, "application_id", |row| row.get(0))
            .map_err(io::Error::other)?;
        let version: u32 = connection
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(io::Error::other)?;
        let schema: Option<String> = connection
            .query_row(
                "SELECT sql FROM sqlite_schema WHERE name = 'receipts' AND type = 'table'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(io::Error::other)?;
        if application != APPLICATION_ID || version != 1 || schema.as_deref() != Some(SCHEMA) {
            return Err(io::Error::other(
                "execution replay receipt schema is missing or incompatible; refusing to forget operation IDs",
            ));
        }
        Ok(())
    }

    fn open(&self) -> io::Result<Connection> {
        let connection = self.open_unvalidated()?;
        Self::validate(&connection)?;
        Ok(connection)
    }

    fn open_unvalidated(&self) -> io::Result<Connection> {
        for path in [
            self.path.clone(),
            self.path.with_extension("sqlite-journal"),
            self.path.with_extension("sqlite-wal"),
            self.path.with_extension("sqlite-shm"),
        ] {
            match fs::symlink_metadata(&path) {
                Ok(metadata) if !metadata.file_type().is_file() => {
                    return Err(io::Error::other("invalid execution receipt storage"));
                }
                Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
                _ => {}
            }
        }
        let connection = Connection::open_with_flags(
            &self.path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(io::Error::other)?;
        connection
            .pragma_update(None, "journal_mode", "DELETE")
            .map_err(io::Error::other)?;
        connection
            // DELETE journals also need their directory entry synced before payload unlink.
            .pragma_update(None, "synchronous", "EXTRA")
            .map_err(io::Error::other)?;
        let page_size: u64 = connection
            .pragma_query_value(None, "page_size", |row| row.get(0))
            .map_err(io::Error::other)?;
        let maximum: u64 = connection
            .query_row(
                &format!("PRAGMA max_page_count = {}", MAX_RECEIPT_BYTES / page_size),
                [],
                |row| row.get(0),
            )
            .map_err(io::Error::other)?;
        if maximum.saturating_mul(page_size) > MAX_RECEIPT_BYTES {
            return Err(io::Error::other(
                "execution replay receipt capacity reached; IDs must not be discarded",
            ));
        }
        Ok(connection)
    }

    pub(super) fn read(&self, id: &str) -> io::Result<Option<JobRecord>> {
        let connection = self.open()?;
        let bytes: Option<Vec<u8>> = connection
            .query_row(
                "SELECT record FROM receipts WHERE operation_id = ?1",
                [id],
                |row| row.get(0),
            )
            .optional()
            .map_err(io::Error::other)?;
        bytes
            .map(|bytes| {
                if bytes.len() > MAX_RECEIPT_RECORD_BYTES {
                    return Err(io::Error::other("invalid execution replay receipt"));
                }
                let record: JobRecord = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
                if record.snapshot.operation_id != id
                    || record.snapshot.state == hzr_exec::ExecJobState::Running
                    || record.snapshot.outcome.is_some()
                    || record
                        .snapshot
                        .error
                        .as_ref()
                        .is_none_or(|error| error.code != "execution_result_expired")
                {
                    return Err(io::Error::other("invalid execution replay identity"));
                }
                Ok(record)
            })
            .transpose()
    }

    pub(super) fn store(&self, records: &[JobRecord]) -> io::Result<()> {
        let mut connection = self.open()?;
        let transaction = connection.transaction().map_err(io::Error::other)?;
        for record in records {
            let bytes = serde_json::to_vec(record).map_err(io::Error::other)?;
            if bytes.len() > MAX_RECEIPT_RECORD_BYTES {
                return Err(io::Error::other(
                    "execution replay receipt exceeds durable limit",
                ));
            }
            transaction
                .execute(
                    "INSERT OR IGNORE INTO receipts (operation_id, record) VALUES (?1, ?2)",
                    params![record.snapshot.operation_id, bytes],
                )
                .map_err(io::Error::other)?;
            let stored: Vec<u8> = transaction
                .query_row(
                    "SELECT record FROM receipts WHERE operation_id = ?1",
                    [&record.snapshot.operation_id],
                    |row| row.get(0),
                )
                .map_err(io::Error::other)?;
            let stored: JobRecord = serde_json::from_slice(&stored).map_err(io::Error::other)?;
            if stored.workspace != record.workspace
                || stored.request_hash != record.request_hash
                || stored.snapshot.operation_id != record.snapshot.operation_id
                || stored.snapshot.state != record.snapshot.state
                || stored.snapshot.revision != record.snapshot.revision
            {
                return Err(io::Error::other(
                    "execution replay receipt conflicts with its durable identity",
                ));
            }
        }
        transaction.commit().map_err(io::Error::other)
    }
}

fn private_file(path: &Path, create: bool) -> io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create_new(create);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = match options.open(path) {
        Err(error) if create && error.kind() == io::ErrorKind::AlreadyExists => {
            return private_file(path, false);
        }
        result => result?,
    };
    if !file.metadata()?.is_file() {
        return Err(io::Error::other(
            "execution receipts must be regular private files",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::{MAX_RECEIPT_BYTES, ReceiptStore};

    #[test]
    fn receipt_quota_is_bounded_and_sqlite_full_does_not_discard_ids() {
        let directory = tempfile::tempdir().expect("store fixture");
        let store = ReceiptStore::new(
            directory
                .path()
                .canonicalize()
                .expect("canonical fixture")
                .as_path(),
        )
        .expect("store");
        let connection = store.open().expect("private database");
        let page_size: u64 = connection
            .pragma_query_value(None, "page_size", |row| row.get(0))
            .expect("page size");
        let limit: u64 = connection
            .pragma_query_value(None, "max_page_count", |row| row.get(0))
            .expect("receipt quota");
        assert_eq!(page_size * limit, MAX_RECEIPT_BYTES);
        connection
            .execute("INSERT INTO receipts VALUES ('existing-id', X'01')", [])
            .expect("retained fixture ID");
        let count: u64 = connection
            .pragma_query_value(None, "page_count", |row| row.get(0))
            .expect("used pages");
        connection
            .pragma_update(None, "max_page_count", count)
            .expect("isolated full quota");
        let failure = connection
            .execute(
                "INSERT INTO receipts VALUES ('new-id', zeroblob(65536))",
                [],
            )
            .expect_err("fail closed at capacity");
        assert_eq!(
            failure.sqlite_error_code(),
            Some(rusqlite::ErrorCode::DiskFull)
        );
        let retained: Vec<u8> = connection
            .query_row(
                "SELECT record FROM receipts WHERE operation_id = 'existing-id'",
                [],
                |row| row.get(0),
            )
            .expect("original ID retained");
        assert_eq!(retained, [1]);
        assert!(
            std::fs::metadata(directory.path().join("replay.sqlite"))
                .expect("database metadata")
                .len()
                <= MAX_RECEIPT_BYTES
        );
    }
}
