use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::{IndexError, Result};
use crate::workspace::{IndexPlacementPolicy, Workspace};

pub const WORKSPACE_REGISTRATION_SCHEMA_VERSION: u16 = 1;
const REGISTRATION_FILE: &str = "workspace.json";
const REGISTRATION_SIZE_LIMIT: u64 = 64 * 1024;
static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceRegistration {
    pub schema_version: u16,
    pub root: PathBuf,
    pub repository_id: String,
    pub worktree_id: String,
    pub git_backed: bool,
    pub linked_worktree: bool,
    pub index_directory: PathBuf,
    pub registered_at_ms: u64,
    pub last_seen_at_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceRegistryWarning {
    pub path: PathBuf,
    pub detail: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WorkspaceRegistrySnapshot {
    pub registrations: Vec<WorkspaceRegistration>,
    pub warnings: Vec<WorkspaceRegistryWarning>,
}

impl Workspace {
    /// Record this workspace in the HZR-owned registry used by the local visualizer.
    ///
    /// The record is metadata only: it never owns the index and never changes the
    /// workspace placement. Repeated initialization preserves the first registration
    /// timestamp and advances `last_seen_at_ms` atomically.
    pub fn register(&self) -> Result<WorkspaceRegistration> {
        let path = registration_path(self)?;
        let now = now_ms()?;
        let registered_at_ms = read_registration(&path)
            .ok()
            .filter(|existing| {
                existing.repository_id == self.identity.repository_id
                    && existing.worktree_id == self.identity.worktree_id
            })
            .map_or(now, |existing| existing.registered_at_ms);
        let registration = WorkspaceRegistration {
            schema_version: WORKSPACE_REGISTRATION_SCHEMA_VERSION,
            root: self.identity.root.clone(),
            repository_id: self.identity.repository_id.clone(),
            worktree_id: self.identity.worktree_id.clone(),
            git_backed: self.identity.git_common_dir.is_some(),
            linked_worktree: self.identity.linked_worktree,
            index_directory: self.index.directory.clone(),
            registered_at_ms,
            last_seen_at_ms: now,
        };
        validate_registration(
            &registration,
            path.parent().unwrap_or_else(|| Path::new(".")),
        )?;
        write_registration(&path, &registration)?;
        Ok(registration)
    }
}

pub fn registered_workspaces(data_root: &Path) -> WorkspaceRegistrySnapshot {
    let root = data_root.join("workspaces");
    let root = root.canonicalize().unwrap_or(root);
    let mut snapshot = WorkspaceRegistrySnapshot::default();
    let repository_directories = match read_real_directories(&root) {
        Ok(directories) => directories,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return snapshot,
        Err(error) => {
            snapshot.warnings.push(WorkspaceRegistryWarning {
                path: root,
                detail: error.to_string(),
            });
            return snapshot;
        }
    };

    for repository in repository_directories {
        let worktrees = match read_real_directories(&repository) {
            Ok(directories) => directories,
            Err(error) => {
                snapshot.warnings.push(WorkspaceRegistryWarning {
                    path: repository,
                    detail: error.to_string(),
                });
                continue;
            }
        };
        for worktree in worktrees {
            let path = worktree.join(REGISTRATION_FILE);
            if !is_regular_file_without_symlink(&path) {
                if path.exists() || fs::symlink_metadata(&path).is_ok() {
                    snapshot.warnings.push(WorkspaceRegistryWarning {
                        path,
                        detail: "registration is not a regular file".into(),
                    });
                }
                continue;
            }
            match read_registration(&path)
                .and_then(|registration| validate_registration(&registration, &worktree))
            {
                Ok(registration) => snapshot.registrations.push(registration),
                Err(error) => snapshot.warnings.push(WorkspaceRegistryWarning {
                    path,
                    detail: error.to_string(),
                }),
            }
        }
    }

    // A directory initialized before `git init` receives a path-derived identity and a
    // repository-derived identity afterwards. Retain the newest exact-root record so the
    // visualizer shows one project while the old metadata remains available for audit.
    let mut by_root = HashMap::<PathBuf, WorkspaceRegistration>::new();
    for registration in snapshot.registrations {
        match by_root.get_mut(&registration.root) {
            Some(existing) if existing.last_seen_at_ms < registration.last_seen_at_ms => {
                *existing = registration;
            }
            None => {
                by_root.insert(registration.root.clone(), registration);
            }
            Some(_) => {}
        }
    }
    snapshot.registrations = by_root.into_values().collect();
    snapshot.registrations.sort_by(|left, right| {
        right
            .last_seen_at_ms
            .cmp(&left.last_seen_at_ms)
            .then_with(|| left.root.cmp(&right.root))
    });
    snapshot
}

fn registration_path(workspace: &Workspace) -> Result<PathBuf> {
    let IndexPlacementPolicy::Managed { data_root } = &workspace.placement_policy else {
        return Err(IndexError::InvalidInput {
            field: "workspace registration",
            reason: "only HZR-managed workspaces can be registered".into(),
        });
    };
    Ok(data_root
        .join("workspaces")
        .join(&workspace.identity.repository_id)
        .join(&workspace.identity.worktree_id)
        .join(REGISTRATION_FILE))
}

fn read_real_directories(path: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut directories = Vec::new();
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            directories.push(entry.path());
        }
    }
    Ok(directories)
}

fn is_regular_file_without_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_file())
        .unwrap_or(false)
}

fn read_registration(path: &Path) -> Result<WorkspaceRegistration> {
    let metadata = fs::symlink_metadata(path).map_err(|source| IndexError::Io {
        operation: "inspect workspace registration",
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.file_type().is_file() {
        return Err(IndexError::InvalidInput {
            field: "workspace registration",
            reason: format!("{} is not a regular file", path.display()),
        });
    }
    if metadata.len() > REGISTRATION_SIZE_LIMIT {
        return Err(IndexError::InvalidInput {
            field: "workspace registration",
            reason: format!(
                "{} exceeds the {} byte limit",
                path.display(),
                REGISTRATION_SIZE_LIMIT
            ),
        });
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    File::open(path)
        .and_then(|file| {
            file.take(REGISTRATION_SIZE_LIMIT + 1)
                .read_to_end(&mut bytes)
        })
        .map_err(|source| IndexError::Io {
            operation: "read workspace registration",
            path: path.to_path_buf(),
            source,
        })?;
    if bytes.len() as u64 > REGISTRATION_SIZE_LIMIT {
        return Err(IndexError::InvalidInput {
            field: "workspace registration",
            reason: format!(
                "{} exceeds the {} byte limit",
                path.display(),
                REGISTRATION_SIZE_LIMIT
            ),
        });
    }
    serde_json::from_slice(&bytes).map_err(|error| IndexError::InvalidInput {
        field: "workspace registration",
        reason: format!("{} contains invalid JSON: {error}", path.display()),
    })
}

fn validate_registration(
    registration: &WorkspaceRegistration,
    worktree_directory: &Path,
) -> Result<WorkspaceRegistration> {
    if registration.schema_version != WORKSPACE_REGISTRATION_SCHEMA_VERSION {
        return Err(IndexError::InvalidInput {
            field: "workspace registration",
            reason: format!("unsupported schema version {}", registration.schema_version),
        });
    }
    if !registration.root.is_absolute() || !registration.index_directory.is_absolute() {
        return Err(IndexError::InvalidInput {
            field: "workspace registration",
            reason: "root and index directory must be absolute".into(),
        });
    }
    if !is_sha256(&registration.repository_id) || !is_sha256(&registration.worktree_id) {
        return Err(IndexError::InvalidInput {
            field: "workspace registration",
            reason: "repository and worktree IDs must be lowercase SHA-256 values".into(),
        });
    }
    let directory_repository_id = worktree_directory
        .parent()
        .and_then(Path::file_name)
        .and_then(|value| value.to_str());
    let directory_worktree_id = worktree_directory
        .file_name()
        .and_then(|value| value.to_str());
    if directory_repository_id != Some(&registration.repository_id)
        || directory_worktree_id != Some(&registration.worktree_id)
        || registration.index_directory != worktree_directory.join("index/grepai")
    {
        return Err(IndexError::InvalidInput {
            field: "workspace registration",
            reason: "identity or index directory does not match the registry path".into(),
        });
    }
    if registration.last_seen_at_ms < registration.registered_at_ms {
        return Err(IndexError::InvalidInput {
            field: "workspace registration",
            reason: "last_seen_at_ms precedes registered_at_ms".into(),
        });
    }
    Ok(registration.clone())
}

fn write_registration(path: &Path, registration: &WorkspaceRegistration) -> Result<()> {
    let parent = path.parent().ok_or_else(|| IndexError::InvalidInput {
        field: "workspace registration",
        reason: format!("{} has no parent directory", path.display()),
    })?;
    fs::create_dir_all(parent).map_err(|source| IndexError::Io {
        operation: "create workspace registration directory",
        path: parent.to_path_buf(),
        source,
    })?;
    if fs::symlink_metadata(path).is_ok_and(|metadata| !metadata.file_type().is_file()) {
        return Err(IndexError::InvalidInput {
            field: "workspace registration",
            reason: format!("{} is not a regular file", path.display()),
        });
    }
    let mut bytes =
        serde_json::to_vec_pretty(registration).map_err(|error| IndexError::InvalidInput {
            field: "workspace registration",
            reason: format!("cannot serialize exact workspace path: {error}"),
        })?;
    bytes.push(b'\n');
    let sequence = TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(
        ".{REGISTRATION_FILE}.tmp-{}-{sequence}",
        std::process::id()
    ));
    let write_result = (|| -> Result<()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary).map_err(|source| IndexError::Io {
            operation: "create temporary workspace registration",
            path: temporary.clone(),
            source,
        })?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|source| IndexError::Io {
                operation: "write temporary workspace registration",
                path: temporary.clone(),
                source,
            })?;
        fs::rename(&temporary, path).map_err(|source| IndexError::Io {
            operation: "replace workspace registration",
            path: path.to_path_buf(),
            source,
        })?;
        sync_directory(parent).map_err(|source| IndexError::Io {
            operation: "sync workspace registration directory",
            path: parent.to_path_buf(),
            source,
        })
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .as_bytes()
            .iter()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn now_ms() -> Result<u64> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| IndexError::InvalidInput {
            field: "workspace registration time",
            reason: error.to_string(),
        })?
        .as_millis();
    u64::try_from(millis).map_err(|error| IndexError::InvalidInput {
        field: "workspace registration time",
        reason: error.to_string(),
    })
}

fn sync_directory(path: &Path) -> std::io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::time::Duration;

    use tempfile::tempdir;

    use super::{REGISTRATION_SIZE_LIMIT, registered_workspaces};
    use crate::Workspace;

    #[tokio::test]
    async fn repeated_registration_is_idempotent_and_private() {
        let directory = tempdir().expect("temporary directory");
        let workspace_root = directory.path().join("workspace");
        let data_root = directory.path().join("data");
        fs::create_dir_all(&workspace_root).expect("workspace directory");
        let workspace = Workspace::discover_managed(
            &workspace_root,
            Path::new("missing-git"),
            &data_root,
            Duration::from_millis(50),
        )
        .await
        .expect("discover workspace");
        workspace
            .ensure_managed_location()
            .expect("prepare managed location");

        let first = workspace.register().expect("first registration");
        let second = workspace.register().expect("second registration");
        assert_eq!(first.registered_at_ms, second.registered_at_ms);
        assert!(second.last_seen_at_ms >= first.last_seen_at_ms);

        let snapshot = registered_workspaces(&data_root);
        assert!(
            snapshot.warnings.is_empty(),
            "unexpected registry warnings: {:?}",
            snapshot.warnings
        );
        assert_eq!(snapshot.registrations, vec![second.clone()]);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let metadata = fs::metadata(
                data_root
                    .join("workspaces")
                    .join(&second.repository_id)
                    .join(&second.worktree_id)
                    .join("workspace.json"),
            )
            .expect("registration metadata");
            assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        }
    }

    #[tokio::test]
    async fn registry_rejects_oversized_and_symlinked_records() {
        let directory = tempdir().expect("temporary directory");
        let worktree = directory
            .path()
            .join("workspaces")
            .join("a".repeat(64))
            .join("b".repeat(64));
        fs::create_dir_all(&worktree).expect("worktree registry directory");
        let record = worktree.join("workspace.json");
        fs::write(&record, vec![b'x'; REGISTRATION_SIZE_LIMIT as usize + 1])
            .expect("oversized fixture");
        let oversized = registered_workspaces(directory.path());
        assert!(oversized.registrations.is_empty());
        assert_eq!(oversized.warnings.len(), 1);

        fs::remove_file(&record).expect("remove oversized fixture");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc/passwd", &record).expect("symlink fixture");
            let symlinked = registered_workspaces(directory.path());
            assert!(symlinked.registrations.is_empty());
            assert_eq!(symlinked.warnings.len(), 1);
        }
    }
}
