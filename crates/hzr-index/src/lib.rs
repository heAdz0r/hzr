mod coordinator;
mod error;
mod generation;
mod grepai;
mod migration;
mod owner;
mod paths;
mod process;
mod registry;
mod watch;
mod workspace;

pub use coordinator::{
    IndexCoordinator, IndexCoordinatorRegistrySnapshot, IndexCoordinatorSnapshot,
    IndexRegistryWatcherSnapshot, IndexWatcherSnapshot, IndexWatcherState, PreparedIndex,
};
pub use error::{ErrorCode, IndexError, Result};
pub use generation::{CACHE_SCHEMA_VERSION, IndexGeneration};
// 0.11.0 (heAdz0r/hzr#22): restore_gitignore_if_only_grepai_added is exported for tests.
pub use grepai::{
    Deadlines, EmbeddingProvider, GrepAi, IndexStatus, InitOptions, InitOutcome,
    SINGLE_WORKTREE_WATCH_FLAG, SUPPORTED_GREPAI_VERSION, StoreBackend,
    is_only_grepai_gitignore_append, restore_gitignore_if_only_grepai_added,
};
pub use migration::{
    INDEX_MIGRATION_SCHEMA_VERSION, IndexArchiveManifest, IndexArchiveOutcome, IndexArchiveState,
    IndexEntryKind, IndexMigrationEntry, IndexMigrationManifest, IndexMigrationOutcome,
    IndexMigrationState, ManifestPath, archive_duplicate_index, migrate_legacy_index,
};
pub use registry::{
    WORKSPACE_REGISTRATION_SCHEMA_VERSION, WorkspaceRegistration, WorkspaceRegistrySnapshot,
    WorkspaceRegistryWarning, registered_workspaces,
};
pub use watch::WatchHandle;
// 0.11.2: `is_managed_index_link` joins the workspace exports
pub use workspace::{
    IndexLayout, IndexPlacement, IndexPlacementPolicy, Workspace, WorkspaceIdentity,
    is_managed_index_link,
};
