mod support;
mod tree;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{IndexError, Result};
use crate::owner::IndexOwner;
use crate::workspace::active_duplicate_indexes;
use crate::workspace::{IndexPlacement, IndexPlacementPolicy, Workspace};

use self::support::{
    create_directory, create_directory_symlink, encoded_os, ensure_absent,
    ensure_no_prefixed_entry, ensure_no_symlink_components, ensure_safe_target,
    ensure_target_relationships, metadata, platform_path_encoding, read_manifest, sync_directory,
    write_new_manifest,
};
use self::tree::{copy_snapshot, snapshot};

pub const INDEX_MIGRATION_SCHEMA_VERSION: u16 = 1;

const APPLIED_MANIFEST: &str = "grepai-v1.json";
const PREPARED_MANIFEST: &str = "grepai-v1.prepared.json";
const BACKUP_PREFIX: &str = ".grepai.hzr-backup-";
const STAGING_PREFIX: &str = ".grepai.hzr-stage-";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ManifestPath {
    pub display: String,
    pub encoding: String,
    pub hex: String,
}

impl ManifestPath {
    fn new(path: &Path) -> Self {
        Self {
            display: if path.as_os_str().is_empty() {
                ".".into()
            } else {
                path.to_string_lossy().into_owned()
            },
            encoding: platform_path_encoding().into(),
            hex: hex::encode(encoded_os(path.as_os_str())),
        }
    }

    fn matches(&self, path: &Path) -> bool {
        self.encoding == platform_path_encoding()
            && self.hex == hex::encode(encoded_os(path.as_os_str()))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexEntryKind {
    Directory,
    File,
    Symlink,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct IndexMigrationEntry {
    pub relative_path: ManifestPath,
    pub kind: IndexEntryKind,
    pub mode: u32,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symlink_target: Option<ManifestPath>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexMigrationState {
    Prepared,
    Applied,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct IndexMigrationManifest {
    pub schema_version: u16,
    pub migration_id: String,
    pub state: IndexMigrationState,
    pub repository_id: String,
    pub worktree_id: String,
    pub workspace_root: ManifestPath,
    pub project_link: ManifestPath,
    pub source: ManifestPath,
    pub target: ManifestPath,
    pub backup: ManifestPath,
    pub tree_sha256: String,
    pub entries: Vec<IndexMigrationEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum IndexMigrationOutcome {
    Applied {
        manifest_path: PathBuf,
        manifest: IndexMigrationManifest,
    },
    AlreadyApplied {
        manifest_path: PathBuf,
        manifest: IndexMigrationManifest,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndexArchiveState {
    Prepared,
    Applied,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct IndexArchiveManifest {
    pub schema_version: u16,
    pub archive_id: String,
    pub state: IndexArchiveState,
    pub repository_id: String,
    pub worktree_id: String,
    pub workspace_root: ManifestPath,
    pub source: ManifestPath,
    pub backup: ManifestPath,
    pub tree_sha256: String,
    pub entries: Vec<IndexMigrationEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum IndexArchiveOutcome {
    Planned {
        manifest_path: PathBuf,
        manifest: IndexArchiveManifest,
    },
    Applied {
        manifest_path: PathBuf,
        manifest: IndexArchiveManifest,
    },
    AlreadyApplied {
        manifest_path: PathBuf,
        manifest: IndexArchiveManifest,
    },
}

pub async fn archive_duplicate_index(
    start: &Path,
    source: &Path,
    git_binary: &Path,
    data_root: &Path,
    deadline: Duration,
    apply: bool,
) -> Result<IndexArchiveOutcome> {
    let workspace = Workspace::discover_managed(start, git_binary, data_root, deadline).await?;
    let source = normalize_archive_source(&workspace.identity.root, source)?;
    let archive_id = hex::encode(Sha256::digest(source.as_os_str().as_encoded_bytes()));
    let manifest_dir = data_root
        .join("migrations")
        .join(&workspace.identity.repository_id)
        .join(&workspace.identity.worktree_id);
    let prepared_path = manifest_dir.join(format!("archive-{archive_id}.prepared.json"));
    let applied_path = manifest_dir.join(format!("archive-{archive_id}.json"));
    let source_present = match fs::symlink_metadata(&source) {
        Ok(metadata) if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() => true,
        Ok(_) => {
            return Err(conflict(format!(
                "duplicate index source is not a real directory: {}",
                source.display()
            )));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(source_error) => {
            return Err(IndexError::Io {
                operation: "inspect duplicate index source",
                path: source.clone(),
                source: source_error,
            });
        }
    };
    if let Some(manifest) = read_manifest::<IndexArchiveManifest>(&applied_path)? {
        validate_archive_manifest(&workspace, &source, &manifest)?;
        validate_archive_backup(&source, &manifest)?;
        if source_present {
            return Err(conflict(format!(
                "duplicate index source {} was recreated after archive {}; refusing to treat a live generation as already archived",
                source.display(),
                manifest.archive_id
            )));
        }
        return Ok(IndexArchiveOutcome::AlreadyApplied {
            manifest_path: applied_path,
            manifest,
        });
    }

    let existing_prepared = read_manifest::<IndexArchiveManifest>(&prepared_path)?;
    if !source_present {
        let mut manifest = existing_prepared.ok_or_else(|| {
            conflict(format!(
                "duplicate index source does not exist and has no recovery manifest: {}",
                source.display()
            ))
        })?;
        validate_archive_manifest(&workspace, &source, &manifest)?;
        validate_archive_backup(&source, &manifest)?;
        if !apply {
            return Ok(IndexArchiveOutcome::Planned {
                manifest_path: applied_path,
                manifest,
            });
        }
        manifest.state = IndexArchiveState::Applied;
        write_new_manifest(&applied_path, &manifest)?;
        return Ok(IndexArchiveOutcome::Applied {
            manifest_path: applied_path,
            manifest,
        });
    }
    if !workspace
        .duplicate_index_dirs
        .iter()
        .any(|path| path == &source)
    {
        return Err(conflict(format!(
            "{} is not a parent-owned duplicate .grepai for workspace {}",
            source.display(),
            workspace.identity.root.display()
        )));
    }
    if !active_duplicate_indexes(std::slice::from_ref(&source))?.is_empty() {
        return Err(conflict(format!(
            "duplicate index has an active writer lock: {}",
            source.display()
        )));
    }
    let source_snapshot = snapshot(&source)?;
    let suffix = migration_suffix(&source_snapshot.digest)?;
    let backup = source
        .parent()
        .ok_or_else(|| conflict("duplicate index has no parent"))?
        .join(format!(".grepai.hzr-archive-{suffix}"));
    let mut manifest = IndexArchiveManifest {
        schema_version: INDEX_MIGRATION_SCHEMA_VERSION,
        archive_id,
        state: IndexArchiveState::Prepared,
        repository_id: workspace.identity.repository_id.clone(),
        worktree_id: workspace.identity.worktree_id.clone(),
        workspace_root: ManifestPath::new(&workspace.identity.root),
        source: ManifestPath::new(&source),
        backup: ManifestPath::new(&backup),
        tree_sha256: source_snapshot.digest,
        entries: source_snapshot
            .entries
            .iter()
            .map(|entry| entry.manifest.clone())
            .collect(),
    };
    if let Some(prepared) = existing_prepared {
        if prepared != manifest {
            return Err(conflict(format!(
                "prepared archive manifest disagrees with source {}",
                source.display()
            )));
        }
    } else if apply {
        create_directory(&manifest_dir, "create index archive manifest directory")?;
        write_new_manifest(&prepared_path, &manifest)?;
    }
    ensure_absent(&backup, "duplicate index archive")?;
    if !apply {
        return Ok(IndexArchiveOutcome::Planned {
            manifest_path: applied_path,
            manifest,
        });
    }
    fs::rename(&source, &backup).map_err(|source_error| IndexError::Io {
        operation: "archive duplicate grepai index",
        path: source.clone(),
        source: source_error,
    })?;
    sync_directory(source.parent().unwrap_or_else(|| Path::new(".")))?;
    if snapshot(&backup)?.digest != manifest.tree_sha256 {
        return Err(conflict(format!(
            "archived duplicate differs from its source manifest: {}",
            backup.display()
        )));
    }
    manifest.state = IndexArchiveState::Applied;
    write_new_manifest(&applied_path, &manifest)?;
    Ok(IndexArchiveOutcome::Applied {
        manifest_path: applied_path,
        manifest,
    })
}

fn normalize_archive_source(workspace_root: &Path, source: &Path) -> Result<PathBuf> {
    let requested = if source.is_absolute() {
        source.to_path_buf()
    } else {
        workspace_root.join(source)
    };
    if requested.file_name().and_then(|name| name.to_str()) != Some(".grepai") {
        return Err(conflict(
            "archive source must name an exact .grepai directory",
        ));
    }
    let parent = requested
        .parent()
        .ok_or_else(|| conflict("archive source has no parent"))?
        .canonicalize()
        .map_err(|source_error| IndexError::Io {
            operation: "canonicalize duplicate index parent",
            path: requested.clone(),
            source: source_error,
        })?;
    let normalized = parent.join(".grepai");
    if normalized == workspace_root.join(".grepai") || !normalized.starts_with(workspace_root) {
        return Err(conflict(format!(
            "archive source must be a nested duplicate inside {}",
            workspace_root.display()
        )));
    }
    Ok(normalized)
}

fn validate_archive_manifest(
    workspace: &Workspace,
    source: &Path,
    manifest: &IndexArchiveManifest,
) -> Result<()> {
    let expected_archive_id = hex::encode(Sha256::digest(source.as_os_str().as_encoded_bytes()));
    if manifest.schema_version != INDEX_MIGRATION_SCHEMA_VERSION
        || manifest.archive_id != expected_archive_id
        || manifest.repository_id != workspace.identity.repository_id
        || manifest.worktree_id != workspace.identity.worktree_id
        || !manifest.workspace_root.matches(&workspace.identity.root)
        || !manifest.source.matches(source)
    {
        return Err(conflict(
            "duplicate index archive manifest does not describe this workspace and source",
        ));
    }
    Ok(())
}

fn validate_archive_backup(source: &Path, manifest: &IndexArchiveManifest) -> Result<()> {
    let suffix = migration_suffix(&manifest.tree_sha256)?;
    let backup = source
        .parent()
        .ok_or_else(|| conflict("duplicate index has no parent"))?
        .join(format!(".grepai.hzr-archive-{suffix}"));
    if !manifest.backup.matches(&backup) {
        return Err(conflict(
            "duplicate index archive manifest names a foreign backup",
        ));
    }
    if snapshot(&backup)?.digest != manifest.tree_sha256 {
        return Err(conflict(format!(
            "duplicate index archive no longer matches its manifest: {}",
            backup.display()
        )));
    }
    Ok(())
}

pub async fn migrate_legacy_index(
    start: &Path,
    git_binary: &Path,
    data_root: &Path,
    deadline: Duration,
) -> Result<IndexMigrationOutcome> {
    let workspace = Workspace::discover_managed(start, git_binary, data_root, deadline).await?;
    if !workspace.duplicate_index_dirs.is_empty() {
        return Err(IndexError::DuplicateIndexes {
            canonical: workspace.index.project_entry.clone(),
            duplicates: workspace.duplicate_index_dirs.clone(),
        });
    }

    let managed_root = match &workspace.placement_policy {
        IndexPlacementPolicy::Managed { data_root } => data_root,
        IndexPlacementPolicy::ProjectLocal => {
            return Err(conflict("migration requires a managed index policy"));
        }
    };
    ensure_target_relationships(&workspace.identity.root, managed_root)?;
    create_directory(managed_root, "create HZR data root")?;
    let target = managed_target(&workspace, managed_root);
    ensure_safe_target(&workspace.identity.root, managed_root, &target)?;
    let manifest_dir = managed_root
        .join("migrations")
        .join(&workspace.identity.repository_id)
        .join(&workspace.identity.worktree_id);
    let prepared_path = manifest_dir.join(PREPARED_MANIFEST);
    let applied_path = manifest_dir.join(APPLIED_MANIFEST);

    match workspace.placement()? {
        IndexPlacement::ManagedSymlink { .. } => {
            replay_applied(&workspace, &target, &prepared_path, &applied_path)
        }
        IndexPlacement::LegacyProject { directory } => migrate_directory(
            &workspace,
            managed_root,
            &directory,
            &target,
            &manifest_dir,
            &prepared_path,
            &applied_path,
        ),
        IndexPlacement::Missing { project_entry, .. } => Err(conflict(format!(
            "no legacy index directory exists at {}",
            project_entry.display()
        ))),
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

fn migrate_directory(
    workspace: &Workspace,
    managed_root: &Path,
    source: &Path,
    target: &Path,
    manifest_dir: &Path,
    prepared_path: &Path,
    applied_path: &Path,
) -> Result<IndexMigrationOutcome> {
    ensure_absent(prepared_path, "prepared migration manifest")?;
    ensure_absent(applied_path, "applied migration manifest")?;
    ensure_absent(target, "canonical index target")?;
    ensure_no_prefixed_entry(&workspace.identity.root, BACKUP_PREFIX)?;

    let config = source.join("config.yaml");
    let config_metadata = metadata(&config, "inspect legacy grepai config")?;
    if !config_metadata.file_type().is_file() {
        return Err(conflict(format!(
            "legacy index config is not a regular file: {}",
            config.display()
        )));
    }

    let _legacy_owner = IndexOwner::acquire_path(source, &source.join("hzr-owner.lock"))?;
    let source_snapshot = snapshot(source)?;
    let suffix = migration_suffix(&source_snapshot.digest)?;
    let migration_id = format!(
        "grepai-{}-{}",
        &workspace.identity.worktree_id[..16],
        suffix
    );
    let backup = workspace
        .identity
        .root
        .join(format!("{BACKUP_PREFIX}{suffix}"));
    let target_parent = target.parent().ok_or_else(|| {
        conflict(format!(
            "canonical target has no parent: {}",
            target.display()
        ))
    })?;
    let staging = target_parent.join(format!("{STAGING_PREFIX}{suffix}"));
    ensure_absent(&backup, "legacy index backup")?;
    ensure_no_prefixed_entry(target_parent, STAGING_PREFIX)?;
    create_directory(target_parent, "create canonical index parent")?;
    ensure_no_symlink_components(managed_root, target_parent)?;

    copy_snapshot(source, &staging, &source_snapshot)?;
    let staged_snapshot = snapshot(&staging)?;
    if staged_snapshot.digest != source_snapshot.digest {
        return Err(conflict(format!(
            "staged index digest {} differs from source {}",
            staged_snapshot.digest, source_snapshot.digest
        )));
    }
    let source_before_switch = snapshot(source)?;
    if source_before_switch.digest != source_snapshot.digest {
        return Err(conflict(
            "legacy index changed while migration was preparing; retry after stopping writers",
        ));
    }

    ensure_no_symlink_components(managed_root, manifest_dir)?;
    create_directory(manifest_dir, "create migration manifest directory")?;
    ensure_no_symlink_components(managed_root, manifest_dir)?;
    let mut manifest = IndexMigrationManifest {
        schema_version: INDEX_MIGRATION_SCHEMA_VERSION,
        migration_id,
        state: IndexMigrationState::Prepared,
        repository_id: workspace.identity.repository_id.clone(),
        worktree_id: workspace.identity.worktree_id.clone(),
        workspace_root: ManifestPath::new(&workspace.identity.root),
        project_link: ManifestPath::new(&workspace.index.project_entry),
        source: ManifestPath::new(source),
        target: ManifestPath::new(target),
        backup: ManifestPath::new(&backup),
        tree_sha256: source_snapshot.digest,
        entries: source_snapshot
            .entries
            .iter()
            .map(|entry| entry.manifest.clone())
            .collect(),
    };
    write_new_manifest(prepared_path, &manifest)?;

    fs::rename(source, &backup).map_err(|source_error| IndexError::Io {
        operation: "move legacy index to recoverable backup",
        path: source.to_path_buf(),
        source: source_error,
    })?;
    sync_directory(&workspace.identity.root)?;
    if snapshot(&backup)?.digest != manifest.tree_sha256 {
        return Err(conflict(format!(
            "backup changed during migration: {}",
            backup.display()
        )));
    }

    fs::rename(&staging, target).map_err(|source_error| IndexError::Io {
        operation: "install canonical grepai index",
        path: target.to_path_buf(),
        source: source_error,
    })?;
    sync_directory(target_parent)?;
    let _canonical_owner = IndexOwner::acquire_path(target, &target.join("hzr-owner.lock"))?;
    create_directory_symlink(target, &workspace.index.project_entry)?;
    sync_directory(&workspace.identity.root)?;
    if snapshot(&backup)?.digest != manifest.tree_sha256 {
        return Err(conflict(format!(
            "backup changed before migration commit: {}",
            backup.display()
        )));
    }

    manifest.state = IndexMigrationState::Applied;
    write_new_manifest(applied_path, &manifest)?;
    Ok(IndexMigrationOutcome::Applied {
        manifest_path: applied_path.to_path_buf(),
        manifest,
    })
}

fn replay_applied(
    workspace: &Workspace,
    target: &Path,
    prepared_path: &Path,
    applied_path: &Path,
) -> Result<IndexMigrationOutcome> {
    let manifest = read_manifest::<IndexMigrationManifest>(applied_path)?.ok_or_else(|| {
        conflict(format!(
            "managed index has no applied migration manifest at {}",
            applied_path.display()
        ))
    })?;
    let _canonical_owner = IndexOwner::acquire_path(target, &target.join("hzr-owner.lock"))?;
    let prepared = read_manifest::<IndexMigrationManifest>(prepared_path)?.ok_or_else(|| {
        conflict(format!(
            "applied migration is missing its prepared manifest at {}",
            prepared_path.display()
        ))
    })?;
    validate_manifest(workspace, target, &manifest)?;
    let mut expected_prepared = manifest.clone();
    expected_prepared.state = IndexMigrationState::Prepared;
    if prepared != expected_prepared {
        return Err(conflict(format!(
            "prepared and applied migration manifests disagree in {}",
            prepared_path.display()
        )));
    }

    let suffix = migration_suffix(&manifest.tree_sha256)?;
    let backup = workspace
        .identity
        .root
        .join(format!("{BACKUP_PREFIX}{suffix}"));
    if !manifest.backup.matches(&backup) {
        return Err(conflict("migration manifest names a foreign backup path"));
    }
    let target_metadata = metadata(target, "inspect migrated canonical index")?;
    if !target_metadata.file_type().is_dir() || target_metadata.file_type().is_symlink() {
        return Err(conflict(format!(
            "canonical migration target is not a real directory: {}",
            target.display()
        )));
    }
    let backup_metadata = metadata(&backup, "inspect migration backup")?;
    if !backup_metadata.file_type().is_dir() || backup_metadata.file_type().is_symlink() {
        return Err(conflict(format!(
            "migration backup is not a real directory: {}",
            backup.display()
        )));
    }
    if snapshot(&backup)?.digest != manifest.tree_sha256 {
        return Err(conflict(format!(
            "migration backup no longer matches its manifest: {}",
            backup.display()
        )));
    }

    Ok(IndexMigrationOutcome::AlreadyApplied {
        manifest_path: applied_path.to_path_buf(),
        manifest,
    })
}

fn validate_manifest(
    workspace: &Workspace,
    target: &Path,
    manifest: &IndexMigrationManifest,
) -> Result<()> {
    if manifest.schema_version != INDEX_MIGRATION_SCHEMA_VERSION {
        return Err(conflict(format!(
            "unsupported migration manifest schema {}",
            manifest.schema_version
        )));
    }
    if manifest.state != IndexMigrationState::Applied
        || manifest.repository_id != workspace.identity.repository_id
        || manifest.worktree_id != workspace.identity.worktree_id
        || !manifest.workspace_root.matches(&workspace.identity.root)
        || !manifest
            .project_link
            .matches(&workspace.index.project_entry)
        || !manifest.source.matches(&workspace.index.project_entry)
        || !manifest.target.matches(target)
    {
        return Err(conflict(
            "migration manifest does not describe this workspace",
        ));
    }
    let suffix = migration_suffix(&manifest.tree_sha256)?;
    let expected_id = format!(
        "grepai-{}-{}",
        &workspace.identity.worktree_id[..16],
        suffix
    );
    if manifest.migration_id != expected_id {
        return Err(conflict("migration manifest identifier is inconsistent"));
    }
    Ok(())
}

fn managed_target(workspace: &Workspace, data_root: &Path) -> PathBuf {
    data_root
        .join("workspaces")
        .join(&workspace.identity.repository_id)
        .join(&workspace.identity.worktree_id)
        .join("index/grepai")
}

fn migration_suffix(digest: &str) -> Result<&str> {
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(conflict("migration tree digest is invalid"));
    }
    Ok(digest)
}

fn conflict(reason: impl Into<String>) -> IndexError {
    IndexError::MigrationConflict {
        reason: reason.into(),
    }
}
