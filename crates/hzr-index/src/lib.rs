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
    IndexCoordinator, IndexCoordinatorSnapshot, IndexWatcherSnapshot, IndexWatcherState,
    PreparedIndex,
};
pub use error::{ErrorCode, IndexError, Result};
pub use generation::{CACHE_SCHEMA_VERSION, IndexGeneration};
pub use grepai::{
    Deadlines, EmbeddingProvider, GrepAi, IndexStatus, InitOptions, InitOutcome,
    SINGLE_WORKTREE_WATCH_FLAG, SUPPORTED_GREPAI_VERSION, StoreBackend,
};
pub use migration::{
    INDEX_MIGRATION_SCHEMA_VERSION, IndexEntryKind, IndexMigrationEntry, IndexMigrationManifest,
    IndexMigrationOutcome, IndexMigrationState, ManifestPath, migrate_legacy_index,
};
pub use registry::{
    WORKSPACE_REGISTRATION_SCHEMA_VERSION, WorkspaceRegistration, WorkspaceRegistrySnapshot,
    WorkspaceRegistryWarning, registered_workspaces,
};
pub use watch::WatchHandle;
pub use workspace::{
    IndexLayout, IndexPlacement, IndexPlacementPolicy, Workspace, WorkspaceIdentity,
};
