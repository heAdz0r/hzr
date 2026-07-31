use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::{Deadlines, GrepAi, IndexGeneration, InitOptions, Result, WatchHandle, Workspace};

#[derive(Clone, Debug)]
pub struct PreparedIndex {
    pub workspace: Workspace,
    pub generation: IndexGeneration,
}

#[derive(Clone)]
pub struct IndexCoordinator {
    data_root: PathBuf,
    git_binary: PathBuf,
    grepai_binary: PathBuf,
    deadlines: Deadlines,
    auto_index: bool,
    watchers: Arc<Mutex<HashMap<String, WatchHandle>>>,
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
        }
    }

    pub async fn workspace(&self, start: &Path) -> Result<Workspace> {
        let workspace = Workspace::discover_managed(
            start,
            &self.git_binary,
            &self.data_root,
            self.deadlines.version,
        )
        .await?;
        workspace.require_managed_index()?;
        Ok(workspace)
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

    pub async fn shutdown(&self) -> Result<()> {
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
        let key = grepai.workspace().identity.worktree_id.clone();
        let mut watchers = self.watchers.lock().await;
        if let Some(handle) = watchers.get_mut(&key) {
            if handle.is_running()? {
                return Ok(());
            }
            watchers.remove(&key);
        }
        let handle = grepai.start_watch().await?;
        watchers.insert(key, handle);
        Ok(())
    }
}
