#![cfg(unix)]

use std::fs;
use std::fs::OpenOptions;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use fs2::FileExt;
use hzr_index::{
    IndexArchiveOutcome, IndexError, IndexMigrationManifest, IndexMigrationOutcome,
    IndexMigrationState, Workspace, archive_duplicate_index, migrate_legacy_index,
};
use tempfile::TempDir;

const MISSING_GIT: &str = "hzr-test-git-command-that-does-not-exist";

#[tokio::test]
async fn test_migrate_legacy_index_preserves_bytes_modes_and_safe_symlinks() {
    let fixture = MigrationFixture::new();
    let executable = fixture.legacy.join("bin/cache-helper");
    fs::create_dir(fixture.legacy.join("bin")).expect("create nested legacy directory");
    fs::write(&executable, b"#!/bin/sh\nexit 0\n").expect("write executable fixture");
    fs::set_permissions(&executable, fs::Permissions::from_mode(0o751)).expect("set fixture mode");
    std::os::unix::fs::symlink("../index.gob", fixture.legacy.join("bin/current-index"))
        .expect("create safe relative symlink");

    let outcome = fixture.apply().await.expect("legacy migration succeeds");
    let (manifest_path, manifest) = match outcome {
        IndexMigrationOutcome::Applied {
            manifest_path,
            manifest,
        } => Some((manifest_path, manifest)),
        IndexMigrationOutcome::AlreadyApplied { .. } => None,
    }
    .expect("first migration must apply");
    let target = fs::canonicalize(fixture.project.path().join(".grepai"))
        .expect("managed project link resolves");
    let backup = backup_path(fixture.project.path(), &manifest.tree_sha256);

    assert_eq!(manifest.state, IndexMigrationState::Applied);
    assert!(manifest_path.is_file());
    assert!(backup.is_dir());
    assert_eq!(
        fs::read(target.join("index.gob")).expect("target bytes"),
        b"index-bytes\0\xff"
    );
    assert_eq!(
        fs::metadata(target.join("bin/cache-helper"))
            .expect("target mode")
            .permissions()
            .mode()
            & 0o777,
        0o751
    );
    assert_eq!(
        fs::read_link(target.join("bin/current-index")).expect("target symlink"),
        Path::new("../index.gob")
    );
    assert_eq!(
        fs::read(backup.join("index.gob")).expect("backup bytes"),
        b"index-bytes\0\xff"
    );
    let persisted: IndexMigrationManifest = serde_json::from_slice(
        &fs::read(&manifest_path).expect("read persisted migration manifest"),
    )
    .expect("parse persisted migration manifest");
    assert_eq!(persisted, manifest);
}

#[tokio::test]
async fn test_migrate_legacy_index_refuses_duplicate_indexes_without_mutation() {
    let fixture = MigrationFixture::new();
    let duplicate = fixture.project.path().join("vendor/module/.grepai");
    fs::create_dir_all(&duplicate).expect("create duplicate index");
    fs::write(duplicate.join("config.yaml"), b"version: 1\n").expect("write duplicate config");

    let result = fixture.apply().await;

    assert!(matches!(result, Err(IndexError::DuplicateIndexes { .. })));
    assert!(fixture.legacy.is_dir());
    assert!(!fixture.data.path().join("workspaces").exists());
}

#[tokio::test]
async fn test_archive_duplicate_index_is_explicit_hashed_and_idempotent() {
    let fixture = MigrationFixture::new();
    let duplicate = fixture.project.path().join("vendor/module/.grepai");
    fs::create_dir_all(&duplicate).expect("create duplicate index");
    fs::write(duplicate.join("config.yaml"), b"version: 1\n").expect("write duplicate config");
    fs::write(duplicate.join("index.gob"), b"duplicate vectors").expect("write vectors");

    let planned = archive_duplicate_index(
        fixture.project.path(),
        &duplicate,
        Path::new(MISSING_GIT),
        fixture.data.path(),
        Duration::from_secs(1),
        false,
    )
    .await
    .expect("archive dry-run");
    assert!(matches!(planned, IndexArchiveOutcome::Planned { .. }));
    assert!(duplicate.is_dir());
    assert!(!fixture.data.path().join("migrations").exists());

    let applied = archive_duplicate_index(
        fixture.project.path(),
        &duplicate,
        Path::new(MISSING_GIT),
        fixture.data.path(),
        Duration::from_secs(1),
        true,
    )
    .await
    .expect("archive apply");
    let (manifest_path, backup) = match applied {
        IndexArchiveOutcome::Applied {
            manifest_path,
            manifest,
        } => Some((manifest_path, PathBuf::from(manifest.backup.display))),
        _ => None,
    }
    .expect("first archive must apply");
    assert!(manifest_path.is_file());
    assert!(!duplicate.exists());
    assert_eq!(
        fs::read(backup.join("index.gob")).expect("archived vectors"),
        b"duplicate vectors"
    );

    let replay = archive_duplicate_index(
        fixture.project.path(),
        &duplicate,
        Path::new(MISSING_GIT),
        fixture.data.path(),
        Duration::from_secs(1),
        true,
    )
    .await
    .expect("archive replay");
    assert!(matches!(replay, IndexArchiveOutcome::AlreadyApplied { .. }));

    fs::create_dir_all(&duplicate).expect("recreate duplicate generation");
    fs::write(duplicate.join("config.yaml"), b"version: 2\n")
        .expect("write recreated duplicate config");
    let recreated = archive_duplicate_index(
        fixture.project.path(),
        &duplicate,
        Path::new(MISSING_GIT),
        fixture.data.path(),
        Duration::from_secs(1),
        true,
    )
    .await;
    assert!(matches!(
        recreated,
        Err(IndexError::MigrationConflict { ref reason })
            if reason.contains("recreated after archive")
    ));
    assert!(duplicate.is_dir());
}

#[tokio::test]
async fn test_migrate_legacy_index_refuses_existing_canonical_target() {
    let fixture = MigrationFixture::new();
    let workspace = Workspace::discover_managed(
        fixture.project.path(),
        Path::new(MISSING_GIT),
        fixture.data.path(),
        Duration::from_secs(1),
    )
    .await
    .expect("discover managed identity");
    let target = fixture
        .data
        .path()
        .join("workspaces")
        .join(workspace.identity.repository_id)
        .join(workspace.identity.worktree_id)
        .join("index/grepai");
    fs::create_dir_all(&target).expect("create conflicting target");
    fs::write(target.join("foreign"), b"do not replace").expect("write conflict marker");

    let result = fixture.apply().await;

    assert!(matches!(result, Err(IndexError::MigrationConflict { .. })));
    assert!(fixture.legacy.is_dir());
    assert_eq!(
        fs::read(target.join("foreign")).expect("conflict remains"),
        b"do not replace"
    );
}

#[tokio::test]
async fn test_migrate_legacy_index_replay_is_idempotent_and_keeps_backup() {
    let fixture = MigrationFixture::new();
    let first = fixture.apply().await.expect("first migration succeeds");
    let first_manifest = match first {
        IndexMigrationOutcome::Applied { manifest, .. } => Some(manifest),
        IndexMigrationOutcome::AlreadyApplied { .. } => None,
    }
    .expect("first call must apply");
    let backup = backup_path(fixture.project.path(), &first_manifest.tree_sha256);

    let replay = fixture.apply().await.expect("migration replay succeeds");

    assert!(matches!(
        replay,
        IndexMigrationOutcome::AlreadyApplied { ref manifest, .. }
            if manifest.migration_id == first_manifest.migration_id
    ));
    assert!(backup.is_dir());
    assert_eq!(
        fs::read(backup.join("index.gob")).expect("backup remains intact"),
        b"index-bytes\0\xff"
    );
}

#[tokio::test]
async fn test_migrate_legacy_index_refuses_escaping_inner_symlink() {
    let fixture = MigrationFixture::new();
    std::os::unix::fs::symlink("../../outside", fixture.legacy.join("escape"))
        .expect("create escaping symlink fixture");

    let result = fixture.apply().await;

    assert!(matches!(result, Err(IndexError::MigrationConflict { .. })));
    assert!(fixture.legacy.is_dir());
    assert!(!fixture.data.path().join("workspaces").exists());
}

#[tokio::test]
async fn test_migrate_legacy_index_refuses_active_legacy_writer() {
    let fixture = MigrationFixture::new();
    let lock_path = fixture.legacy.join("hzr-owner.lock");
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .expect("open writer lock fixture");
    lock.try_lock_exclusive().expect("hold writer lock fixture");

    let result = fixture.apply().await;

    assert!(matches!(result, Err(IndexError::IndexOwnerBusy { .. })));
    assert!(fixture.legacy.is_dir());
    assert!(!fixture.data.path().join("workspaces").exists());
    FileExt::unlock(&lock).expect("release writer lock fixture");
}

struct MigrationFixture {
    project: TempDir,
    data: TempDir,
    legacy: PathBuf,
}

impl MigrationFixture {
    fn new() -> Self {
        let project = tempfile::tempdir().expect("temporary project");
        let data = tempfile::tempdir().expect("temporary data root");
        let legacy = project.path().join(".grepai");
        fs::create_dir(&legacy).expect("create legacy index");
        fs::write(legacy.join("config.yaml"), b"version: 1\n").expect("write legacy config");
        fs::write(legacy.join("index.gob"), b"index-bytes\0\xff").expect("write legacy index");
        Self {
            project,
            data,
            legacy,
        }
    }

    async fn apply(&self) -> Result<IndexMigrationOutcome, IndexError> {
        migrate_legacy_index(
            self.project.path(),
            Path::new(MISSING_GIT),
            self.data.path(),
            Duration::from_secs(1),
        )
        .await
    }
}

fn backup_path(project: &Path, digest: &str) -> PathBuf {
    project.join(format!(".grepai.hzr-backup-{digest}"))
}
