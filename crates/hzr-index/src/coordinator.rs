use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::{Mutex, RwLock};

use crate::IndexStatus;
use crate::{Deadlines, GrepAi, IndexGeneration, InitOptions, Result, WatchHandle, Workspace};

#[derive(Clone, Debug)]
pub struct PreparedIndex {
    pub workspace: Workspace,
    pub generation: IndexGeneration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndexWatcherState {
    Live,
    Standby,
    Failed,
}

#[derive(Clone, Debug)]
pub struct IndexWatcherSnapshot {
    pub state: IndexWatcherState,
    pub pid: Option<u32>,
    pub uptime_ms: Option<u64>,
    pub ready_marker_observed: bool,
}

#[derive(Clone, Debug)]
pub struct IndexCoordinatorSnapshot {
    pub workspace: Workspace,
    pub index: IndexStatus,
    pub watcher: IndexWatcherSnapshot,
}

#[derive(Clone)]
pub struct IndexCoordinator {
    data_root: PathBuf,
    git_binary: PathBuf,
    grepai_binary: PathBuf,
    deadlines: Deadlines,
    auto_index: bool,
    watchers: Arc<Mutex<HashMap<String, WatchHandle>>>,
    watcher_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
    watcher_lifecycle: Arc<RwLock<()>>,
    workspaces: Arc<Mutex<HashMap<PathBuf, Workspace>>>,
}

impl IndexCoordinator {
    #[must_use]
    pub fn new(
        data_root: PathBuf,
        git_binary: PathBuf,
        grepai_binary: PathBuf,
        deadlines: Deadlines,
        auto_index: bool,
    ) -> Self {
        Self {
            data_root,
            git_binary,
            grepai_binary,
            deadlines,
            auto_index,
            watchers: Arc::new(Mutex::new(HashMap::new())),
            watcher_locks: Arc::new(Mutex::new(HashMap::new())),
            watcher_lifecycle: Arc::new(RwLock::new(())),
            workspaces: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn workspace(&self, start: &Path) -> Result<Workspace> {
        let discovered = Workspace::discover_managed_fast(
            start,
            &self.git_binary,
            &self.data_root,
            self.deadlines.version,
        )
        .await?;
        if let Some(workspace) = self
            .workspaces
            .lock()
            .await
            .get(&discovered.identity.root)
            .cloned()
        {
            workspace.require_managed_index()?;
            return Ok(workspace);
        }
        let workspace = Workspace::discover_managed(
            &discovered.identity.root,
            &self.git_binary,
            &self.data_root,
            self.deadlines.version,
        )
        .await?;
        workspace.require_managed_index()?;
        self.workspaces
            .lock()
            .await
            .insert(workspace.identity.root.clone(), workspace.clone());
        Ok(workspace)
    }

    pub async fn workspace_for_builtin_search(&self, start: &Path) -> Result<Workspace> {
        Workspace::discover_managed_fast(
            start,
            &self.git_binary,
            &self.data_root,
            self.deadlines.version,
        )
        .await
    }

    #[must_use]
    pub fn grepai_binary(&self) -> &Path {
        &self.grepai_binary
    }

    pub async fn prepare(&self, start: &Path) -> Result<PreparedIndex> {
        let workspace = self.workspace(start).await?;
        self.prepare_workspace(workspace).await
    }

    pub async fn prepare_workspace(&self, workspace: Workspace) -> Result<PreparedIndex> {
        workspace.require_managed_index()?;
        let grepai = GrepAi::connect(
            self.grepai_binary.clone(),
            workspace.clone(),
            self.deadlines.clone(),
        )
        .await?;
        if self.auto_index {
            grepai.initialize(&InitOptions::default()).await?;
            self.ensure_watcher(&grepai).await?;
        } else {
            workspace.require_initialized()?;
        }
        let generation = IndexGeneration::read(&workspace)?;
        Ok(PreparedIndex {
            workspace,
            generation,
        })
    }

    pub async fn status(&self, start: &Path) -> Result<IndexCoordinatorSnapshot> {
        let workspace = self.workspace(start).await?;
        let initialized = workspace.index.config.is_file();
        let index = IndexStatus {
            placement: workspace.placement()?,
            initialized,
            vectors_present: workspace.index.vectors.is_file(),
            symbols_present: workspace.index.symbols.is_file(),
            repository_graph_present: workspace.index.repository_graph.is_file(),
            duplicate_index_dirs: workspace.duplicate_index_dirs.clone(),
            generation: initialized
                .then(|| IndexGeneration::read(&workspace))
                .transpose()?,
        };
        let watcher = {
            let mut watchers = self.watchers.lock().await;
            match watchers.get_mut(&workspace.identity.worktree_id) {
                Some(handle) => {
                    let pid = handle.pid();
                    let uptime_ms = u64::try_from(handle.uptime().as_millis()).unwrap_or(u64::MAX);
                    let state = if handle.is_running()? {
                        IndexWatcherState::Live
                    } else {
                        IndexWatcherState::Failed
                    };
                    IndexWatcherSnapshot {
                        state,
                        pid,
                        uptime_ms: Some(uptime_ms),
                        ready_marker_observed: true,
                    }
                }
                None => IndexWatcherSnapshot {
                    state: IndexWatcherState::Standby,
                    pid: None,
                    uptime_ms: None,
                    ready_marker_observed: false,
                },
            }
        };
        Ok(IndexCoordinatorSnapshot {
            workspace,
            index,
            watcher,
        })
    }

    pub async fn shutdown(&self) -> Result<()> {
        let _lifecycle = self.watcher_lifecycle.write().await;
        let handles = {
            let mut watchers = self.watchers.lock().await;
            std::mem::take(&mut *watchers)
                .into_values()
                .collect::<Vec<_>>()
        };
        for handle in handles {
            handle.shutdown().await?;
        }
        Ok(())
    }

    async fn ensure_watcher(&self, grepai: &GrepAi) -> Result<()> {
        let _lifecycle = self.watcher_lifecycle.read().await;
        let key = grepai.workspace().identity.worktree_id.clone();
        let watcher_lock = {
            let mut locks = self.watcher_locks.lock().await;
            Arc::clone(
                locks
                    .entry(key.clone())
                    .or_insert_with(|| Arc::new(Mutex::new(()))),
            )
        };
        let _watcher = watcher_lock.lock().await;
        {
            let mut watchers = self.watchers.lock().await;
            if let Some(handle) = watchers.get_mut(&key) {
                if handle.is_running()? {
                    return Ok(());
                }
                watchers.remove(&key);
            }
        }
        let handle = grepai.start_watch().await?;
        self.watchers.lock().await.insert(key, handle);
        Ok(())
    }
}
