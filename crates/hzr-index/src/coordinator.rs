use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Mutex, RwLock};
use tokio::time::Instant;

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

#[derive(Clone, Debug)]
pub struct IndexCoordinatorRegistrySnapshot {
    pub active_watchers: usize,
    pub watcher_limit: usize,
    pub watcher_idle_ttl_ms: u64,
    pub watchers: Vec<IndexRegistryWatcherSnapshot>,
}

#[derive(Clone, Debug)]
pub struct IndexRegistryWatcherSnapshot {
    pub worktree_id: String,
    pub state: IndexWatcherState,
    pub pid: Option<u32>,
    pub uptime_ms: u64,
    pub idle_ms: u64,
}

struct WatcherEntry {
    handle: WatchHandle,
    last_used: Instant,
    failed_at: Option<Instant>,
}

const DEFAULT_WATCHER_LIMIT: usize = 8;
const DEFAULT_WATCHER_IDLE_TTL: Duration = Duration::from_secs(15 * 60);

#[derive(Clone)]
pub struct IndexCoordinator {
    data_root: PathBuf,
    git_binary: PathBuf,
    grepai_binary: PathBuf,
    deadlines: Deadlines,
    auto_index: bool,
    watchers: Arc<Mutex<HashMap<String, WatcherEntry>>>,
    watcher_budget: Arc<Mutex<()>>,
    watcher_lifecycle: Arc<RwLock<()>>,
    watcher_limit: usize,
    watcher_idle_ttl: Duration,
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
        Self::with_watcher_limits(
            data_root,
            git_binary,
            grepai_binary,
            deadlines,
            auto_index,
            DEFAULT_WATCHER_LIMIT,
            DEFAULT_WATCHER_IDLE_TTL,
        )
    }

    #[must_use]
    pub fn with_watcher_limits(
        data_root: PathBuf,
        git_binary: PathBuf,
        grepai_binary: PathBuf,
        deadlines: Deadlines,
        auto_index: bool,
        watcher_limit: usize,
        watcher_idle_ttl: Duration,
    ) -> Self {
        Self {
            data_root,
            git_binary,
            grepai_binary,
            deadlines,
            auto_index,
            watchers: Arc::new(Mutex::new(HashMap::new())),
            watcher_budget: Arc::new(Mutex::new(())),
            watcher_lifecycle: Arc::new(RwLock::new(())),
            watcher_limit: watcher_limit.max(1),
            watcher_idle_ttl,
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
        self.reap_idle_watchers().await?;
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
                Some(entry) => {
                    let pid = entry.handle.pid();
                    let uptime_ms =
                        u64::try_from(entry.handle.uptime().as_millis()).unwrap_or(u64::MAX);
                    let state = if entry.handle.is_running()? {
                        entry.last_used = Instant::now();
                        entry.failed_at = None;
                        IndexWatcherState::Live
                    } else {
                        entry.failed_at.get_or_insert_with(Instant::now);
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
                .into_iter()
                .map(|(key, entry)| (key, entry.handle))
                .collect::<Vec<_>>()
        };
        shutdown_watchers(handles).await?;
        self.workspaces.lock().await.clear();
        Ok(())
    }

    pub async fn registry_snapshot(&self) -> Result<IndexCoordinatorRegistrySnapshot> {
        let now = Instant::now();
        let mut watchers = self.watchers.lock().await;
        let mut entries = Vec::with_capacity(watchers.len());
        for (worktree_id, entry) in watchers.iter_mut() {
            let state = if entry.handle.is_running()? {
                entry.failed_at = None;
                IndexWatcherState::Live
            } else {
                entry.failed_at.get_or_insert(now);
                IndexWatcherState::Failed
            };
            entries.push(IndexRegistryWatcherSnapshot {
                worktree_id: worktree_id.clone(),
                state,
                pid: entry.handle.pid(),
                uptime_ms: millis(entry.handle.uptime()),
                idle_ms: millis(now.saturating_duration_since(entry.last_used)),
            });
        }
        entries.sort_by(|left, right| left.worktree_id.cmp(&right.worktree_id));
        let active_watchers = entries
            .iter()
            .filter(|watcher| watcher.state == IndexWatcherState::Live)
            .count();
        Ok(IndexCoordinatorRegistrySnapshot {
            active_watchers,
            watcher_limit: self.watcher_limit,
            watcher_idle_ttl_ms: millis(self.watcher_idle_ttl),
            watchers: entries,
        })
    }

    pub async fn reap_idle_watchers(&self) -> Result<usize> {
        let _lifecycle = self.watcher_lifecycle.read().await;
        let _budget = self.watcher_budget.lock().await;
        let evicted = self.take_evictions(false).await?;
        let count = evicted.len();
        shutdown_watchers(evicted).await?;
        Ok(count)
    }

    async fn ensure_watcher(&self, grepai: &GrepAi) -> Result<()> {
        let _lifecycle = self.watcher_lifecycle.read().await;
        let _budget = self.watcher_budget.lock().await;
        let key = grepai.workspace().identity.worktree_id.clone();
        {
            let mut watchers = self.watchers.lock().await;
            if let Some(entry) = watchers.get_mut(&key) {
                if entry.handle.is_running()? {
                    entry.last_used = Instant::now();
                    return Ok(());
                }
                watchers.remove(&key);
            }
        }
        shutdown_watchers(self.take_evictions(true).await?).await?;
        let handle = grepai.start_watch().await?;
        self.watchers.lock().await.insert(
            key,
            WatcherEntry {
                handle,
                last_used: Instant::now(),
                failed_at: None,
            },
        );
        Ok(())
    }

    async fn take_evictions(&self, reserve_slot: bool) -> Result<Vec<(String, WatchHandle)>> {
        let now = Instant::now();
        let mut watchers = self.watchers.lock().await;
        let mut keys = Vec::new();
        let mut failed = Vec::new();
        for (key, entry) in watchers.iter_mut() {
            if !entry.handle.is_running()? {
                // Keep a bounded failed tombstone visible to health/dashboard until its
                // normal idle TTL expires. `ensure_watcher` still removes it immediately
                // when routed traffic explicitly restarts this worktree.
                let failed_at = *entry.failed_at.get_or_insert(now);
                failed.push(key.clone());
                if now.saturating_duration_since(failed_at) >= self.watcher_idle_ttl {
                    keys.push(key.clone());
                }
            } else if now.saturating_duration_since(entry.last_used) >= self.watcher_idle_ttl {
                entry.failed_at = None;
                keys.push(key.clone());
            }
        }
        let target = self.watcher_limit.saturating_sub(usize::from(reserve_slot));
        while watchers.len().saturating_sub(keys.len()) > target {
            let oldest_failed = watchers
                .iter()
                .filter(|(key, _)| failed.contains(key) && !keys.contains(key))
                .min_by_key(|(_, entry)| entry.failed_at.unwrap_or(entry.last_used))
                .map(|(key, _)| key.clone());
            let oldest = oldest_failed.or_else(|| {
                watchers
                    .iter()
                    .filter(|(key, _)| !keys.contains(key))
                    .min_by_key(|(_, entry)| entry.last_used)
                    .map(|(key, _)| key.clone())
            });
            let Some(oldest) = oldest else {
                break;
            };
            keys.push(oldest);
        }
        Ok(keys
            .into_iter()
            .filter_map(|key| watchers.remove(&key).map(|entry| (key, entry.handle)))
            .collect())
    }
}

async fn shutdown_watchers(mut watchers: Vec<(String, WatchHandle)>) -> Result<()> {
    watchers.sort_by(|left, right| left.0.cmp(&right.0));
    let mut first_error = None;
    for (_, watcher) in watchers {
        if let Err(error) = watcher.shutdown().await {
            first_error.get_or_insert(error);
        }
    }
    if let Some(error) = first_error {
        return Err(error);
    }
    Ok(())
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}
