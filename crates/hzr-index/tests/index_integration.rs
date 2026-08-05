#![cfg(unix)]

use std::fs::{self, OpenOptions};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use fs2::FileExt;
use hzr_index::{
    Deadlines, GrepAi, IndexCoordinator, IndexError, IndexPlacement, IndexWatcherState,
    InitOptions, InitOutcome, Workspace,
};
use tempfile::TempDir;

#[tokio::test]
async fn test_workspace_uses_git_root_and_reports_dormant_nested_indexes() {
    let repo = git_repo();
    let nested = repo.path().join("src/deep");
    fs::create_dir_all(&nested).expect("nested source directory must be created");
    let duplicate = repo.path().join("packages/package/.grepai");
    fs::create_dir_all(&duplicate).expect("duplicate index directory must be created");

    let workspace = discover(&nested).await;

    let canonical_root = fs::canonicalize(repo.path()).expect("repository path must canonicalize");
    let canonical_duplicate =
        fs::canonicalize(&duplicate).expect("duplicate path must canonicalize");
    assert_eq!(workspace.identity.root, canonical_root.clone());
    assert_eq!(workspace.index.directory, canonical_root.join(".grepai"));
    assert_eq!(workspace.duplicate_index_dirs, vec![canonical_duplicate]);
    assert!(workspace.require_single_index().is_ok());
    assert!(duplicate.is_dir());
}

#[tokio::test]
async fn test_coordinator_keeps_canonical_index_available_with_dormant_nested_index() {
    let repo = git_repo();
    let data = tempfile::tempdir().expect("managed data root");
    write_source(repo.path(), "pub fn canonical() {}\n");
    fs::write(repo.path().join("fake-grepai-capable"), b"enabled").expect("capability marker");
    let duplicate = repo.path().join("src/.grepai");
    fs::create_dir(&duplicate).expect("dormant nested index directory");
    fs::write(duplicate.join("config.yaml"), "version: 1\n").expect("dormant nested config");
    fs::write(duplicate.join("index.gob"), b"legacy-index").expect("dormant nested vectors");
    let canonical_duplicate =
        fs::canonicalize(&duplicate).expect("dormant nested index must canonicalize");
    let grepai = fake_grepai(repo.path(), "0.35.0");
    let coordinator = IndexCoordinator::new(
        data.path().to_path_buf(),
        PathBuf::from("git"),
        grepai,
        deadlines(),
        true,
    );

    let prepared = coordinator
        .prepare(repo.path())
        .await
        .expect("dormant nested index must not block the canonical index");

    assert_eq!(
        prepared.workspace.duplicate_index_dirs,
        vec![canonical_duplicate]
    );
    assert_eq!(
        fs::read(duplicate.join("index.gob")).expect("dormant vectors remain readable"),
        b"legacy-index"
    );
    assert!(prepared.workspace.index.config.is_file());
    coordinator.shutdown().await.expect("coordinator shutdown");
}

#[tokio::test]
async fn test_coordinator_refuses_an_active_nested_index_writer() {
    let repo = git_repo();
    let data = tempfile::tempdir().expect("managed data root");
    write_source(repo.path(), "pub fn canonical() {}\n");
    let duplicate = repo.path().join("src/.grepai");
    fs::create_dir(&duplicate).expect("nested index directory");
    fs::write(duplicate.join("config.yaml"), "version: 1\n").expect("nested config");
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(duplicate.join("index.gob.lock"))
        .expect("nested writer lock");
    lock.try_lock_exclusive().expect("hold nested writer lock");
    let coordinator = IndexCoordinator::new(
        data.path().to_path_buf(),
        PathBuf::from("git"),
        fake_grepai(repo.path(), "0.35.0"),
        deadlines(),
        true,
    );

    let result = coordinator.prepare(repo.path()).await;

    assert!(matches!(result, Err(IndexError::DuplicateIndexes { .. })));
    assert!(!repo.path().join(".grepai").exists());
    assert!(duplicate.join("config.yaml").is_file());
}

#[tokio::test]
async fn test_linked_worktrees_share_repository_identity_but_not_index_identity() {
    let container = tempfile::tempdir().expect("temporary container must be created");
    let main = container.path().join("main");
    let linked = container.path().join("linked");
    fs::create_dir(&main).expect("main worktree directory must be created");
    let init = Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(&main)
        .status()
        .expect("git init must execute");
    assert!(init.success());
    write_source(&main, "pub fn main_worktree() {}\n");
    let add = Command::new("git")
        .args(["add", "src/lib.rs"])
        .current_dir(&main)
        .status()
        .expect("git add must execute");
    assert!(add.success());
    let commit = Command::new("git")
        .args([
            "-c",
            "user.name=HZR Test",
            "-c",
            "user.email=hzr@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "fixture",
        ])
        .current_dir(&main)
        .status()
        .expect("git commit must execute");
    assert!(commit.success());
    let worktree = Command::new("git")
        .args(["worktree", "add", "--quiet", "--detach"])
        .arg(&linked)
        .current_dir(&main)
        .status()
        .expect("git worktree add must execute");
    assert!(worktree.success());

    let main_workspace = discover(&main).await;
    let linked_workspace = discover(&linked).await;

    assert_eq!(
        main_workspace.identity.repository_id,
        linked_workspace.identity.repository_id
    );
    assert_ne!(
        main_workspace.identity.worktree_id,
        linked_workspace.identity.worktree_id
    );
    assert!(!main_workspace.identity.linked_worktree);
    assert!(linked_workspace.identity.linked_worktree);
    assert_ne!(
        main_workspace.index.directory,
        linked_workspace.index.directory
    );

    let grepai = fake_grepai(&main, "0.35.0");
    let engine = GrepAi::connect(grepai, main_workspace, deadlines())
        .await
        .expect("stock grepai must connect");
    engine
        .initialize(&InitOptions::default())
        .await
        .expect("main worktree initialization must succeed");
    let watch = engine.start_watch().await;
    assert!(matches!(
        watch,
        Err(IndexError::UnsupportedWatchTopology { worktrees: 2, .. })
    ));
}

#[tokio::test]
async fn test_managed_placement_creates_one_central_index_and_project_symlink() {
    let repo = git_repo();
    let data = tempfile::tempdir().expect("managed data root must be created");
    write_source(repo.path(), "pub fn managed() {}\n");
    let grepai = fake_grepai(repo.path(), "0.35.0");
    let workspace = Workspace::discover_managed(
        repo.path(),
        Path::new("git"),
        data.path(),
        Duration::from_secs(3),
    )
    .await
    .expect("managed workspace discovery must succeed");
    assert!(matches!(
        workspace.placement().expect("placement must be readable"),
        IndexPlacement::Missing { managed: true, .. }
    ));
    let central_index = workspace.index.directory.clone();
    let engine = GrepAi::connect(grepai, workspace, deadlines())
        .await
        .expect("pinned grepai must connect");

    let outcome = engine
        .initialize(&InitOptions::default())
        .await
        .expect("managed grepai initialization must succeed");
    let status = engine.status().expect("managed status must be readable");

    assert_eq!(outcome, InitOutcome::Initialized);
    assert!(matches!(
        status.placement,
        IndexPlacement::ManagedSymlink { .. }
    ));
    assert!(
        fs::symlink_metadata(repo.path().join(".grepai"))
            .expect("project entry must exist")
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::canonicalize(repo.path().join(".grepai")).expect("managed symlink must canonicalize"),
        fs::canonicalize(&central_index).expect("central index must canonicalize")
    );
    assert!(central_index.join("config.yaml").is_file());
}

#[tokio::test]
async fn test_ensure_managed_location_is_read_only_after_initialization() {
    let repo = git_repo();
    let data = tempfile::tempdir().expect("managed data root must be created");
    let workspace = Workspace::discover_managed(
        repo.path(),
        Path::new("git"),
        data.path(),
        Duration::from_secs(3),
    )
    .await
    .expect("managed workspace discovery must succeed");

    workspace
        .ensure_managed_location()
        .expect("managed location must be created");
    let link_before = fs::symlink_metadata(&workspace.index.project_entry)
        .expect("managed link metadata")
        .modified()
        .expect("managed link modification time");
    let directory_before = fs::metadata(&workspace.index.directory)
        .expect("managed directory metadata")
        .modified()
        .expect("managed directory modification time");

    workspace
        .ensure_managed_location()
        .expect("existing managed location must be accepted");

    assert_eq!(
        fs::symlink_metadata(&workspace.index.project_entry)
            .expect("managed link metadata after no-op")
            .modified()
            .expect("managed link modification time after no-op"),
        link_before
    );
    assert_eq!(
        fs::metadata(&workspace.index.directory)
            .expect("managed directory metadata after no-op")
            .modified()
            .expect("managed directory modification time after no-op"),
        directory_before
    );
}

#[tokio::test]
async fn test_managed_discovery_adopts_legacy_project_index_without_second_database() {
    let repo = git_repo();
    let data = tempfile::tempdir().expect("managed data root must be created");
    fs::create_dir(repo.path().join(".grepai")).expect("legacy index directory must be created");
    fs::write(repo.path().join(".grepai/config.yaml"), "version: 1\n")
        .expect("legacy config must be written");
    let grepai = fake_grepai(repo.path(), "0.35.0");
    let workspace = Workspace::discover_managed(
        repo.path(),
        Path::new("git"),
        data.path(),
        Duration::from_secs(3),
    )
    .await
    .expect("legacy workspace discovery must succeed");
    assert!(matches!(
        workspace.placement().expect("placement must be readable"),
        IndexPlacement::LegacyProject { .. }
    ));
    let engine = GrepAi::connect(grepai, workspace, deadlines())
        .await
        .expect("pinned grepai must connect");

    let outcome = engine
        .initialize(&InitOptions::default())
        .await
        .expect("legacy index adoption must succeed");

    assert_eq!(outcome, InitOutcome::AlreadyInitialized);
    assert!(!data.path().join("workspaces").exists());
    assert!(repo.path().join(".grepai/config.yaml").is_file());
}

#[tokio::test]
async fn test_coordinator_requires_explicit_legacy_index_migration() {
    let repo = git_repo();
    let data = tempfile::tempdir().expect("managed data root must be created");
    fs::create_dir(repo.path().join(".grepai")).expect("legacy index directory must be created");
    fs::write(repo.path().join(".grepai/config.yaml"), "version: 1\n")
        .expect("legacy config must be written");
    let coordinator = IndexCoordinator::new(
        data.path().to_path_buf(),
        PathBuf::from("git"),
        PathBuf::from("grepai"),
        deadlines(),
        true,
    );
    let discovered = Workspace::discover_managed(
        repo.path(),
        Path::new("git"),
        data.path(),
        Duration::from_secs(3),
    )
    .await
    .expect("discover legacy placement for prepare gate");

    let workspace_result = coordinator.workspace(repo.path()).await;
    let prepare_result = coordinator.prepare_workspace(discovered).await;

    assert!(matches!(
        workspace_result,
        Err(IndexError::LegacyIndexRequiresMigration { .. })
    ));
    assert!(matches!(
        prepare_result,
        Err(IndexError::LegacyIndexRequiresMigration { .. })
    ));
    assert!(!data.path().join("workspaces").exists());
}

#[tokio::test]
async fn test_managed_discovery_blocks_foreign_symlink_without_mutation() {
    let repo = git_repo();
    let data = tempfile::tempdir().expect("managed data root must be created");
    let foreign = tempfile::tempdir().expect("foreign index target must be created");
    std::os::unix::fs::symlink(foreign.path(), repo.path().join(".grepai"))
        .expect("foreign symlink must be created");
    let grepai = fake_grepai(repo.path(), "0.35.0");
    let workspace = Workspace::discover_managed(
        repo.path(),
        Path::new("git"),
        data.path(),
        Duration::from_secs(3),
    )
    .await
    .expect("foreign placement discovery must succeed");
    assert!(matches!(
        workspace.placement().expect("placement must be readable"),
        IndexPlacement::ForeignSymlink { .. }
    ));
    let engine = GrepAi::connect(grepai, workspace, deadlines())
        .await
        .expect("version verification must not mutate placement");

    let result = engine.initialize(&InitOptions::default()).await;

    assert!(matches!(
        result,
        Err(IndexError::ForeignIndexSymlink { .. })
    ));
    assert!(repo.path().join(".grepai").is_symlink());
    assert!(!data.path().join("workspaces").exists());
}

#[tokio::test]
async fn test_watch_has_one_owner_and_is_supervised() {
    let repo = git_repo();
    write_source(repo.path(), "pub fn watched() {}\n");
    let grepai = fake_grepai(repo.path(), "0.35.0");
    fs::write(repo.path().join("fake-grepai-capable"), b"enabled")
        .expect("capability marker must be written");
    let engine = connect(repo.path(), grepai).await;
    engine
        .initialize(&InitOptions::default())
        .await
        .expect("grepai initialization must succeed");

    let mut watcher = engine
        .start_watch()
        .await
        .expect("supervised watcher must become ready");
    let second = engine.start_watch().await;

    assert!(
        watcher
            .is_running()
            .expect("watch liveness must be readable")
    );
    assert!(repo.path().join(".grepai/isolated-watch-arg").is_file());
    assert!(matches!(second, Err(IndexError::IndexOwnerBusy { .. })));
    watcher
        .shutdown()
        .await
        .expect("supervised watcher must stop cleanly");
}

#[tokio::test]
async fn test_coordinator_reuses_one_watcher_for_repeated_prepare() {
    let repo = git_repo();
    let data = tempfile::tempdir().expect("managed data root");
    write_source(repo.path(), "pub fn coordinated() {}\n");
    fs::write(repo.path().join("fake-grepai-capable"), b"enabled").expect("capability marker");
    let grepai = fake_grepai(repo.path(), "0.35.0");
    let coordinator = IndexCoordinator::new(
        data.path().to_path_buf(),
        PathBuf::from("git"),
        grepai,
        deadlines(),
        true,
    );

    let first = coordinator
        .prepare(repo.path())
        .await
        .expect("first prepare");
    let second = coordinator
        .prepare(repo.path())
        .await
        .expect("second prepare");

    assert_eq!(
        first.workspace.identity.worktree_id,
        second.workspace.identity.worktree_id
    );
    assert_eq!(
        fs::canonicalize(repo.path().join(".grepai")).expect("project index target"),
        fs::canonicalize(first.workspace.index.directory).expect("managed index target")
    );
    coordinator.shutdown().await.expect("coordinator shutdown");
}

#[tokio::test]
async fn test_coordinator_status_proves_index_artifacts_and_live_watcher() {
    let repo = git_repo();
    let data = tempfile::tempdir().expect("managed data root");
    write_source(repo.path(), "pub fn observable() {}\n");
    fs::write(repo.path().join("fake-grepai-capable"), b"enabled").expect("capability marker");
    let grepai = fake_grepai(repo.path(), "0.35.0");
    let coordinator = IndexCoordinator::new(
        data.path().to_path_buf(),
        PathBuf::from("git"),
        grepai,
        deadlines(),
        true,
    );
    coordinator
        .prepare(repo.path())
        .await
        .expect("prepared index");

    let snapshot = coordinator.status(repo.path()).await.expect("typed status");

    assert!(snapshot.index.initialized);
    assert!(snapshot.index.vectors_present);
    assert!(snapshot.index.symbols_present);
    assert_eq!(snapshot.watcher.state, IndexWatcherState::Live);
    assert!(snapshot.watcher.pid.is_some());
    coordinator.shutdown().await.expect("coordinator shutdown");
}

#[tokio::test]
async fn test_connect_rejects_unpinned_grepai_version() {
    let repo = git_repo();
    let grepai = fake_grepai(repo.path(), "0.36.0");
    let workspace = discover(repo.path()).await;

    let result = GrepAi::connect(grepai, workspace, deadlines()).await;

    assert!(matches!(
        result,
        Err(IndexError::UnsupportedVersion {
            expected: "0.35.0",
            ..
        })
    ));
}

fn git_repo() -> TempDir {
    let repo = tempfile::tempdir().expect("temporary repository must be created");
    let status = Command::new("git")
        .args(["init", "--quiet"])
        .current_dir(repo.path())
        .status()
        .expect("git init must execute");
    assert!(status.success());
    repo
}

async fn discover(path: &Path) -> Workspace {
    Workspace::discover(path, Path::new("git"), Duration::from_secs(3))
        .await
        .expect("workspace discovery must succeed")
}

async fn connect(root: &Path, binary: PathBuf) -> GrepAi {
    GrepAi::connect(binary, discover(root).await, deadlines())
        .await
        .expect("pinned grepai must connect")
}

fn deadlines() -> Deadlines {
    Deadlines {
        // These tests intentionally launch several isolated fake engines in parallel.
        // Keep the bounded probe above worst-case scheduler latency under workspace CI.
        version: Duration::from_secs(15),
        initialize: Duration::from_secs(5),
        watch_start: Duration::from_secs(5),
        watch_stop: Duration::from_secs(5),
    }
}

fn write_source(root: &Path, content: &str) {
    fs::create_dir_all(root.join("src")).expect("source directory must be created");
    fs::write(root.join("src/lib.rs"), content).expect("source file must be written");
}

fn fake_grepai(root: &Path, version: &str) -> PathBuf {
    let path = root.join("fake-grepai");
    let script = format!(
        r#"#!/bin/sh
set -eu
command_name="${{1:-}}"
case "$command_name" in
  version)
    printf 'grepai version {version}\n'
    ;;
  init)
    mkdir -p .grepai
    printf 'version: 1\n' > .grepai/config.yaml
    : > .grepai/index.gob
    : > .grepai/symbols.gob
    ;;
  search)
    query="${{2:-}}"
    pwd > .grepai/search-cwd
    if [ "$query" = "find fallback failure" ]; then
      printf '{{"error":"embedding service unavailable"}}\n'
    else
      printf '[{{"file_path":"src/lib.rs","start_line":1,"end_line":1,"score":0.91,"content":"pub fn indexed_symbol() {{}}\\n","feature_path":"feature/index","symbol_name":"indexed_symbol"}}]\n'
    fi
    ;;
  watch)
    if [ "${{2:-}}" = "--help" ]; then
      printf 'Usage: grepai watch [flags]\n'
      if [ -f fake-grepai-capable ]; then
        printf '      --no-worktree-discovery   Watch only the current worktree\n'
      fi
      exit 0
    fi
    shift
    stop=0
    log_dir=""
    while [ "$#" -gt 0 ]; do
      case "$1" in
        --stop) stop=1 ;;
        --log-dir) shift; log_dir="$1" ;;
        --no-worktree-discovery) touch .grepai/isolated-watch-arg ;;
      esac
      shift
    done
    if [ "$stop" -eq 1 ]; then
      if [ -f "$log_dir/fake.pid" ]; then
        watcher_pid=$(cat "$log_dir/fake.pid")
        kill -TERM "$watcher_pid"
      fi
      exit 0
    fi
    mkdir -p "$log_dir"
    printf '%s\n' "$$" > "$log_dir/fake.pid"
    printf 'ready\n%s\n' "$$" > "$log_dir/fake.ready"
    cleanup() {{ rm -f "$log_dir/fake.pid" "$log_dir/fake.ready"; exit 0; }}
    trap cleanup INT TERM
    while :; do sleep 1; done
    ;;
  *)
    printf 'unsupported fake command: %s\n' "$command_name" >&2
    exit 2
    ;;
esac
"#
    );
    fs::write(&path, script).expect("fake grepai must be written");
    let mut permissions = fs::metadata(&path)
        .expect("fake grepai metadata must be readable")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&path, permissions).expect("fake grepai must be executable");
    path
}
