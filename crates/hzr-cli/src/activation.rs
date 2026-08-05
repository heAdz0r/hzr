use std::path::{Path, PathBuf};

use anyhow::Result;
use hzr_core::{ActivationMode, Config, EnabledWorkspace};
use hzr_index::{Deadlines, Workspace};

pub async fn discover(config: &Config, start: &Path) -> Result<Workspace> {
    Ok(Workspace::discover_managed_fast(
        start,
        Path::new("git"),
        &config.data_dir,
        Deadlines::default().version,
    )
    .await?)
}

pub async fn is_enabled(config: &Config, start: &Path) -> Result<bool> {
    if config.activation.mode == ActivationMode::All {
        return Ok(true);
    }
    let workspace = discover(config, start).await?;
    Ok(config.activation.allows(
        &workspace.identity.repository_id,
        &workspace.identity.worktree_id,
    ))
}

pub fn record(workspace: &Workspace) -> EnabledWorkspace {
    EnabledWorkspace {
        repository_id: workspace.identity.repository_id.clone(),
        worktree_id: workspace.identity.worktree_id.clone(),
        root: workspace.identity.root.clone(),
    }
}

pub fn local_instruction_paths(root: &Path) -> [(crate::instructions::Surface, PathBuf); 2] {
    [
        (crate::instructions::Surface::Claude, root.join("CLAUDE.md")),
        (crate::instructions::Surface::Codex, root.join("AGENTS.md")),
    ]
}

#[cfg(test)]
mod tests {
    use hzr_core::{ActivationMode, Config};
    use tempfile::tempdir;

    use super::{discover, is_enabled, record};

    #[tokio::test]
    async fn selected_activation_is_exactly_scoped_to_the_enabled_workspace() {
        let directory = tempdir().expect("temporary directory");
        let enabled = directory.path().join("enabled");
        let baseline = directory.path().join("baseline");
        std::fs::create_dir_all(&enabled).expect("enabled directory");
        std::fs::create_dir_all(&baseline).expect("baseline directory");
        let mut config = Config {
            data_dir: directory.path().join("data"),
            ..Config::default()
        };
        config.activation.mode = ActivationMode::Selected;
        let workspace = discover(&config, &enabled)
            .await
            .expect("workspace identity");
        config.activation.enable(record(&workspace));

        assert!(is_enabled(&config, &enabled).await.expect("enabled check"));
        assert!(
            !is_enabled(&config, &baseline)
                .await
                .expect("baseline check")
        );
    }
}
