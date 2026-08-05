use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::Result;
use hzr_core::{ActivationConfig, ActivationMode, Config, EnabledWorkspace};
use hzr_index::{Deadlines, Workspace};
use serde::Serialize;

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

/// Снимок режима активации и списка явно включённых workspace.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ActivationStatusReport {
    pub mode: ActivationMode,
    pub enabled_workspaces: Vec<EnabledWorkspace>,
}

impl ActivationStatusReport {
    #[must_use]
    pub fn from_config(activation: &ActivationConfig) -> Self {
        Self {
            mode: activation.mode,
            enabled_workspaces: activation.enabled_workspaces.clone(),
        }
    }
}

#[must_use]
pub fn render_status_text(report: &ActivationStatusReport) -> String {
    let mode = match report.mode {
        ActivationMode::All => "all",
        ActivationMode::Selected => "selected",
    };
    let mut output = format!("activation={mode}\n");
    match report.mode {
        ActivationMode::All => {
            output.push_str("enabled workspaces: all projects (no selection list)\n");
        }
        ActivationMode::Selected if report.enabled_workspaces.is_empty() => {
            output.push_str("enabled workspaces: none\n");
        }
        ActivationMode::Selected => {
            let _ = writeln!(
                output,
                "enabled workspaces ({}):",
                report.enabled_workspaces.len()
            );
            for workspace in &report.enabled_workspaces {
                let _ = writeln!(output, "  {}", workspace.root.display());
            }
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use hzr_core::{ActivationMode, Config, EnabledWorkspace};
    use tempfile::tempdir;

    use super::{ActivationStatusReport, discover, is_enabled, record, render_status_text};

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

    #[test]
    fn status_report_mirrors_config_activation_without_mutation() {
        let mut config = Config::default();
        config.activation.mode = ActivationMode::Selected;
        config.activation.enabled_workspaces.push(EnabledWorkspace {
            repository_id: "a".repeat(64),
            worktree_id: "b".repeat(64),
            root: "/work/app".into(),
        });

        let report = ActivationStatusReport::from_config(&config.activation);
        assert_eq!(report.mode, ActivationMode::Selected);
        assert_eq!(report.enabled_workspaces.len(), 1);
        assert_eq!(
            report.enabled_workspaces[0].root,
            PathBuf::from("/work/app")
        );
        assert_eq!(config.activation.enabled_workspaces.len(), 1);
    }

    #[test]
    fn human_status_lists_selected_roots() {
        let report = ActivationStatusReport {
            mode: ActivationMode::Selected,
            enabled_workspaces: vec![EnabledWorkspace {
                repository_id: "a".repeat(64),
                worktree_id: "b".repeat(64),
                root: "/work/app".into(),
            }],
        };
        let text = render_status_text(&report);
        assert!(text.contains("activation=selected"));
        assert!(text.contains("enabled workspaces (1):"));
        assert!(text.contains("  /work/app"));
    }

    #[test]
    fn human_status_explains_all_mode() {
        let report = ActivationStatusReport {
            mode: ActivationMode::All,
            enabled_workspaces: Vec::new(),
        };
        let text = render_status_text(&report);
        assert!(text.contains("activation=all"));
        assert!(text.contains("all projects"));
    }
}
