use std::ffi::OsString;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::time::Duration;

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::error::{IndexError, Result};
use crate::process;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkspaceIdentity {
    pub root: PathBuf,
    pub git_common_dir: Option<PathBuf>,
    pub repository_id: String,
    pub worktree_id: String,
    pub linked_worktree: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct IndexLayout {
    pub project_entry: PathBuf,
    pub directory: PathBuf,
    pub config: PathBuf,
    pub vectors: PathBuf,
    pub symbols: PathBuf,
    pub repository_graph: PathBuf,
    pub owner_lock: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexPlacementPolicy {
    ProjectLocal,
    Managed { data_root: PathBuf },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexPlacement {
    ManagedSymlink {
        link: PathBuf,
        target: PathBuf,
    },
    LegacyProject {
        directory: PathBuf,
    },
    Missing {
        project_entry: PathBuf,
        intended_directory: PathBuf,
        managed: bool,
    },
    ForeignSymlink {
        link: PathBuf,
        target: PathBuf,
        expected: PathBuf,
    },
    ForeignEntry {
        path: PathBuf,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    pub identity: WorkspaceIdentity,
    pub index: IndexLayout,
    pub placement_policy: IndexPlacementPolicy,
    pub duplicate_index_dirs: Vec<PathBuf>,
    pub git_binary: PathBuf,
}

impl Workspace {
    pub async fn discover(start: &Path, git_binary: &Path, deadline: Duration) -> Result<Self> {
        Self::discover_with_policy(
            start,
            git_binary,
            deadline,
            IndexPlacementPolicy::ProjectLocal,
            true,
        )
        .await
    }

    pub async fn discover_managed(
        start: &Path,
        git_binary: &Path,
        data_root: &Path,
        deadline: Duration,
    ) -> Result<Self> {
        let data_root = normalize_future_path(data_root)?;
        Self::discover_with_policy(
            start,
            git_binary,
            deadline,
            IndexPlacementPolicy::Managed { data_root },
            true,
        )
        .await
    }

    /// Discover identity and canonical placement without recursively auditing the tree.
    /// Callers that use the index must cache an audited `discover_managed` result first.
    pub async fn discover_managed_fast(
        start: &Path,
        git_binary: &Path,
        data_root: &Path,
        deadline: Duration,
    ) -> Result<Self> {
        let data_root = normalize_future_path(data_root)?;
        Self::discover_with_policy(
            start,
            git_binary,
            deadline,
            IndexPlacementPolicy::Managed { data_root },
            false,
        )
        .await
    }

    async fn discover_with_policy(
        start: &Path,
        git_binary: &Path,
        deadline: Duration,
        placement_policy: IndexPlacementPolicy,
        audit_duplicates: bool,
    ) -> Result<Self> {
        let start = canonical_directory(start)?;
        let git = discover_git(&start, git_binary, deadline).await?;
        let (root, git_common_dir, linked_worktree) = match git {
            Some(git) => (git.root, Some(git.common_dir), git.linked_worktree),
            None => (find_non_git_root(&start), None, false),
        };

        let repository_basis = git_common_dir.as_deref().unwrap_or(&root);
        let repository_id = hash_parts(&[repository_basis.as_os_str().as_encoded_bytes()]);
        let worktree_id = hash_parts(&[
            repository_basis.as_os_str().as_encoded_bytes(),
            root.as_os_str().as_encoded_bytes(),
        ]);
        let project_entry = root.join(".grepai");
        let intended_directory = match &placement_policy {
            IndexPlacementPolicy::ProjectLocal => project_entry.clone(),
            IndexPlacementPolicy::Managed { data_root } => data_root
                .join("workspaces")
                .join(&repository_id)
                .join(&worktree_id)
                .join("index/grepai"),
        };
        let placement = detect_placement(&project_entry, &intended_directory, &placement_policy)?;
        let directory = match &placement {
            IndexPlacement::LegacyProject { directory } => directory.clone(),
            _ => intended_directory,
        };
        let index = IndexLayout {
            project_entry: project_entry.clone(),
            config: directory.join("config.yaml"),
            vectors: directory.join("index.gob"),
            symbols: directory.join("symbols.gob"),
            repository_graph: directory.join("rpg.gob"),
            owner_lock: directory.join("hzr-owner.lock"),
            directory,
        };
        let duplicate_index_dirs = if audit_duplicates {
            find_duplicate_indexes(&root, &project_entry)?
        } else {
            Vec::new()
        };

        Ok(Self {
            identity: WorkspaceIdentity {
                root,
                git_common_dir,
                repository_id,
                worktree_id,
                linked_worktree,
            },
            index,
            placement_policy,
            duplicate_index_dirs,
            git_binary: git_binary.to_path_buf(),
        })
    }

    pub fn placement(&self) -> Result<IndexPlacement> {
        let intended = match &self.placement_policy {
            IndexPlacementPolicy::ProjectLocal => self.index.project_entry.clone(),
            IndexPlacementPolicy::Managed { data_root } => data_root
                .join("workspaces")
                .join(&self.identity.repository_id)
                .join(&self.identity.worktree_id)
                .join("index/grepai"),
        };
        detect_placement(&self.index.project_entry, &intended, &self.placement_policy)
    }

    pub fn require_single_index(&self) -> Result<()> {
        // Nested stores left by older grepai/RTK invocations are dormant data, not
        // alternate owners. HZR always launches the pinned engine from the canonical
        // workspace root and takes the canonical owner lock, so refusing all access
        // here only turns a recoverable hygiene finding into a search outage. Keep
        // reporting the paths through status/doctor, and let explicit migration keep
        // its stricter ambiguity gate, but never mutate or activate a nested store.
        require_supported_placement(self.placement()?)?;
        let active = active_duplicate_indexes(&self.duplicate_index_dirs)?;
        if active.is_empty() {
            return Ok(());
        }
        Err(IndexError::DuplicateIndexes {
            canonical: self.index.directory.clone(),
            duplicates: active,
        })
    }

    pub fn require_managed_index(&self) -> Result<()> {
        self.require_single_index()?;
        if let IndexPlacement::LegacyProject { directory } = self.placement()? {
            return Err(IndexError::LegacyIndexRequiresMigration {
                directory,
                workspace: self.identity.root.clone(),
            });
        }
        Ok(())
    }

    pub fn require_initialized(&self) -> Result<()> {
        require_supported_placement(self.placement()?)?;
        if self.index.config.is_file() {
            return Ok(());
        }

        Err(IndexError::NotInitialized {
            config_path: self.index.config.clone(),
        })
    }

    pub fn normalize_filter(&self, filter: Option<&Path>) -> Result<Option<PathBuf>> {
        crate::paths::normalize_filter(&self.identity.root, filter)
    }

    pub fn normalize_result(&self, path: &Path) -> Result<PathBuf> {
        crate::paths::normalize_result(&self.identity.root, path)
    }

    /// Create the canonical managed index directory and project symlink when absent.
    ///
    /// Existing managed placements are a read-only no-op. Legacy directories and
    /// foreign entries are rejected so callers cannot bypass explicit migration.
    pub fn ensure_managed_location(&self) -> Result<()> {
        self.require_managed_index()?;
        self.prepare_index_location()
    }

    /// Re-point a project symlink that HZR itself created under a previous identity.
    ///
    /// Workspace identity is derived from the git common dir when there is one, and from
    /// the canonical directory path otherwise. So a project initialized before `git init`
    /// legitimately changes identity the moment it becomes a repository, and its existing
    /// symlink then looks foreign. Failing there would leave the most common real
    /// sequence — create a directory, work in it, then `git init` — permanently broken.
    ///
    /// This is deliberately narrow. It only acts when the current target lives inside
    /// *this* managed `workspaces/` subtree, i.e. HZR created it; a symlink into another
    /// data root, another user's directory or an arbitrary path is still foreign and is
    /// refused. The built index is moved rather than discarded, so relocation costs no
    /// re-scan, and it is skipped when the new location already holds a store.
    pub fn adopt_relocated_index(&self) -> Result<bool> {
        let IndexPlacementPolicy::Managed { data_root } = &self.placement_policy else {
            return Ok(false);
        };
        let IndexPlacement::ForeignSymlink { link, target, .. } = self.placement()? else {
            return Ok(false);
        };
        let managed_root = data_root.join("workspaces");
        if !target.starts_with(&managed_root) {
            // Not ours: leave it foreign so the operator decides.
            return Ok(false);
        }
        if self.index.directory.exists() {
            // The new identity already has a store; only the stale link needs replacing.
            remove_file(&link)?;
            create_directory_symlink(&self.index.directory, &link)?;
            return Ok(true);
        }
        if let Some(parent) = self.index.directory.parent() {
            create_directory(parent)?;
        }
        std::fs::rename(&target, &self.index.directory).map_err(|source| IndexError::Io {
            operation: "relocate managed index to the new workspace identity",
            path: self.index.directory.clone(),
            source,
        })?;
        remove_file(&link)?;
        create_directory_symlink(&self.index.directory, &link)?;
        Ok(true)
    }

    pub(crate) fn prepare_index_location(&self) -> Result<()> {
        let placement = self.placement()?;
        require_supported_placement(placement.clone())?;
        match placement {
            IndexPlacement::Missing { managed: false, .. } => {
                create_directory(&self.index.project_entry)
            }
            IndexPlacement::Missing { managed: true, .. } => {
                create_directory(&self.index.directory)?;
                match create_directory_symlink(&self.index.directory, &self.index.project_entry) {
                    Ok(()) => Ok(()),
                    Err(IndexError::Io { source, .. })
                        if source.kind() == std::io::ErrorKind::AlreadyExists =>
                    {
                        require_supported_placement(self.placement()?)
                    }
                    Err(error) => Err(error),
                }
            }
            IndexPlacement::ManagedSymlink { .. } | IndexPlacement::LegacyProject { .. } => Ok(()),
            unsupported => require_supported_placement(unsupported),
        }
    }

    pub async fn git_worktree_count(&self, deadline: Duration) -> Result<usize> {
        if self.identity.git_common_dir.is_none() {
            return Ok(1);
        }
        let output = process::output(
            &self.git_binary,
            &[
                OsString::from("worktree"),
                OsString::from("list"),
                OsString::from("--porcelain"),
            ],
            &self.identity.root,
            deadline,
            "list git worktrees",
        )
        .await?;
        let stdout = process::require_success(output, "list git worktrees")?;
        let stdout =
            std::str::from_utf8(&stdout).map_err(|error| IndexError::InvalidEngineOutput {
                engine: "git",
                operation: "list worktrees",
                detail: error.to_string(),
            })?;
        let count = stdout
            .lines()
            .filter(|line| line.starts_with("worktree "))
            .count();
        if count == 0 {
            return Err(IndexError::InvalidEngineOutput {
                engine: "git",
                operation: "list worktrees",
                detail: "porcelain output contained no worktree records".into(),
            });
        }
        Ok(count)
    }
}

fn detect_placement(
    project_entry: &Path,
    intended_directory: &Path,
    policy: &IndexPlacementPolicy,
) -> Result<IndexPlacement> {
    let metadata = match std::fs::symlink_metadata(project_entry) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(IndexPlacement::Missing {
                project_entry: project_entry.to_path_buf(),
                intended_directory: intended_directory.to_path_buf(),
                managed: matches!(policy, IndexPlacementPolicy::Managed { .. }),
            });
        }
        Err(source) => {
            return Err(IndexError::Io {
                operation: "inspect project grepai entry",
                path: project_entry.to_path_buf(),
                source,
            });
        }
    };

    if metadata.file_type().is_symlink() {
        let raw_target = std::fs::read_link(project_entry).map_err(|source| IndexError::Io {
            operation: "read project grepai symlink",
            path: project_entry.to_path_buf(),
            source,
        })?;
        let target = if raw_target.is_absolute() {
            normalize_future_path(&raw_target)?
        } else {
            normalize_future_path(
                &project_entry
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .join(raw_target),
            )?
        };
        let expected = normalize_future_path(intended_directory)?;
        if matches!(policy, IndexPlacementPolicy::Managed { .. }) && target == expected {
            return Ok(IndexPlacement::ManagedSymlink {
                link: project_entry.to_path_buf(),
                target,
            });
        }
        return Ok(IndexPlacement::ForeignSymlink {
            link: project_entry.to_path_buf(),
            target,
            expected,
        });
    }
    if metadata.is_dir() {
        return Ok(IndexPlacement::LegacyProject {
            directory: project_entry.to_path_buf(),
        });
    }
    Ok(IndexPlacement::ForeignEntry {
        path: project_entry.to_path_buf(),
    })
}

fn require_supported_placement(placement: IndexPlacement) -> Result<()> {
    match placement {
        IndexPlacement::ManagedSymlink { .. }
        | IndexPlacement::LegacyProject { .. }
        | IndexPlacement::Missing { .. } => Ok(()),
        IndexPlacement::ForeignSymlink {
            link,
            target,
            expected,
        } => Err(IndexError::ForeignIndexSymlink {
            link,
            target,
            expected,
        }),
        IndexPlacement::ForeignEntry { path } => Err(IndexError::IndexEntryConflict { path }),
    }
}

/// Remove a symlink (not its target). Used only when replacing an HZR-owned link.
fn remove_file(path: &Path) -> Result<()> {
    std::fs::remove_file(path).map_err(|source| IndexError::Io {
        operation: "remove the stale managed index symlink",
        path: path.to_path_buf(),
        source,
    })
}

fn create_directory(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).map_err(|source| IndexError::Io {
        operation: "create grepai index directory",
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(unix)]
fn create_directory_symlink(target: &Path, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link).map_err(|source| IndexError::Io {
        operation: "create managed grepai symlink",
        path: link.to_path_buf(),
        source,
    })
}

#[cfg(windows)]
fn create_directory_symlink(target: &Path, link: &Path) -> Result<()> {
    std::os::windows::fs::symlink_dir(target, link).map_err(|source| IndexError::Io {
        operation: "create managed grepai symlink",
        path: link.to_path_buf(),
        source,
    })
}

fn normalize_future_path(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|source| IndexError::Io {
                operation: "read current directory",
                path: path.to_path_buf(),
                source,
            })?
            .join(path)
    };
    let mut existing = absolute.as_path();
    let mut suffix = Vec::new();
    while !existing.exists() {
        let name = existing
            .file_name()
            .ok_or_else(|| IndexError::InvalidInput {
                field: "data root",
                reason: format!("{} has no existing ancestor", path.display()),
            })?;
        suffix.push(name.to_os_string());
        existing = existing.parent().ok_or_else(|| IndexError::InvalidInput {
            field: "data root",
            reason: format!("{} has no existing ancestor", path.display()),
        })?;
    }
    let mut normalized = std::fs::canonicalize(existing).map_err(|source| IndexError::Io {
        operation: "canonicalize index path",
        path: existing.to_path_buf(),
        source,
    })?;
    normalized.extend(suffix.into_iter().rev());
    Ok(normalized)
}

struct GitWorkspace {
    root: PathBuf,
    common_dir: PathBuf,
    linked_worktree: bool,
}

async fn discover_git(
    start: &Path,
    git_binary: &Path,
    deadline: Duration,
) -> Result<Option<GitWorkspace>> {
    let root_output = process::output(
        git_binary,
        &["rev-parse".into(), "--show-toplevel".into()],
        start,
        deadline,
        "discover git root",
    )
    .await;
    let root_output = match root_output {
        Ok(output) if output.status.success() => output,
        Ok(_) => return Ok(None),
        Err(IndexError::CommandUnavailable { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    let root = parse_path_output(&root_output.stdout, "git root")?;
    let root = canonical_directory(&root)?;

    let common_args = [
        OsString::from("rev-parse"),
        OsString::from("--path-format=absolute"),
        OsString::from("--git-common-dir"),
    ];
    let common_output = process::output(
        git_binary,
        &common_args,
        &root,
        deadline,
        "discover git common directory",
    )
    .await?;
    let common_bytes = process::require_success(common_output, "discover git common directory")?;
    let common_dir = parse_path_output(&common_bytes, "git common directory")?;
    let common_dir = canonical_directory(&common_dir)?;
    let linked_worktree = root.join(".git").is_file();

    Ok(Some(GitWorkspace {
        root,
        common_dir,
        linked_worktree,
    }))
}

fn canonical_directory(path: &Path) -> Result<PathBuf> {
    let directory = if path.is_file() {
        path.parent().ok_or_else(|| IndexError::InvalidInput {
            field: "workspace path",
            reason: format!("{} has no parent directory", path.display()),
        })?
    } else {
        path
    };
    std::fs::canonicalize(directory).map_err(|source| IndexError::Io {
        operation: "canonicalize workspace path",
        path: directory.to_path_buf(),
        source,
    })
}

fn parse_path_output(bytes: &[u8], field: &'static str) -> Result<PathBuf> {
    let value = std::str::from_utf8(bytes)
        .map_err(|error| IndexError::InvalidEngineOutput {
            engine: "git",
            operation: "workspace discovery",
            detail: error.to_string(),
        })?
        .trim();
    if value.is_empty() {
        return Err(IndexError::InvalidEngineOutput {
            engine: "git",
            operation: "workspace discovery",
            detail: format!("empty {field}"),
        });
    }
    Ok(PathBuf::from(value))
}

fn find_non_git_root(start: &Path) -> PathBuf {
    start
        .ancestors()
        .find(|path| path.join(".grepai/config.yaml").is_file())
        .unwrap_or(start)
        .to_path_buf()
}

fn find_duplicate_indexes(root: &Path, canonical: &Path) -> Result<Vec<PathBuf>> {
    let mut duplicates = Vec::new();
    let mut entries = WalkDir::new(root).follow_links(false).into_iter();
    while let Some(entry) = entries.next() {
        let entry = entry.map_err(|source| IndexError::Io {
            operation: "scan for grepai indexes",
            path: source
                .path()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| root.to_path_buf()),
            source: source
                .into_io_error()
                .unwrap_or_else(|| std::io::Error::other("directory traversal failed")),
        })?;
        let name = entry.file_name().to_string_lossy();
        if entry.file_type().is_dir()
            && matches!(name.as_ref(), ".git" | "target" | "node_modules" | ".venv")
        {
            entries.skip_current_dir();
            continue;
        }
        if name != ".grepai" || !(entry.file_type().is_dir() || entry.file_type().is_symlink()) {
            continue;
        }
        if entry.path() != canonical {
            duplicates.push(entry.path().to_path_buf());
        }
        if entry.file_type().is_dir() {
            entries.skip_current_dir();
        }
    }
    duplicates.sort();
    Ok(duplicates)
}

fn active_duplicate_indexes(duplicates: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut active = Vec::new();
    for duplicate in duplicates {
        let lock_path = duplicate.join("index.gob.lock");
        let metadata = match std::fs::symlink_metadata(&lock_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(source) => {
                return Err(IndexError::Io {
                    operation: "inspect nested grepai writer lock",
                    path: lock_path,
                    source,
                });
            }
        };
        if !metadata.file_type().is_file() {
            active.push(duplicate.clone());
            continue;
        }
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&lock_path)
            .map_err(|source| IndexError::Io {
                operation: "open nested grepai writer lock",
                path: lock_path.clone(),
                source,
            })?;
        match lock.try_lock_exclusive() {
            Ok(()) => drop(lock),
            Err(source) if source.kind() == std::io::ErrorKind::WouldBlock => {
                active.push(duplicate.clone());
            }
            Err(source) => {
                return Err(IndexError::Io {
                    operation: "probe nested grepai writer lock",
                    path: lock_path,
                    source,
                });
            }
        }
    }
    Ok(active)
}

fn hash_parts(parts: &[&[u8]]) -> String {
    let mut hash = Sha256::new();
    for part in parts {
        hash.update((part.len() as u64).to_le_bytes());
        hash.update(part);
    }
    hex::encode(hash.finalize())
}
