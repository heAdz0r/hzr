use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use hzr_core::Config;
use hzr_index::{Deadlines, IndexPlacement, Workspace, WorkspaceIdentity};
use serde::Serialize;

use crate::diagnostics::resolve_binary;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LegacyArtifact {
    pub kind: String,
    pub path: PathBuf,
    pub symlink: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct MigrationScan {
    pub schema_version: u16,
    pub read_only: bool,
    pub workspace: PathBuf,
    pub identity: Option<WorkspaceIdentity>,
    pub index_placement: Option<IndexPlacement>,
    pub duplicate_indexes: Vec<PathBuf>,
    pub artifacts: Vec<LegacyArtifact>,
    pub warnings: Vec<String>,
}

pub async fn scan(config: &Config, workspace: &Path) -> MigrationScan {
    let mut warnings = Vec::new();
    let discovered = Workspace::discover_managed(
        workspace,
        Path::new("git"),
        &config.data_dir,
        Deadlines::default().version,
    )
    .await;
    let (identity, index_placement, duplicate_indexes) = match discovered {
        Ok(discovered) => {
            let placement = match discovered.placement() {
                Ok(placement) => Some(placement),
                Err(error) => {
                    warnings.push(error.to_string());
                    None
                }
            };
            (
                Some(discovered.identity),
                placement,
                discovered.duplicate_index_dirs,
            )
        }
        Err(error) => {
            warnings.push(error.to_string());
            (None, None, Vec::new())
        }
    };

    let mut candidates = Vec::new();
    candidates.extend([
        ("rgai_workspace", workspace.join("rgai")),
        ("rgai_workspace", workspace.join(".rgai")),
        ("icm_workspace", workspace.join(".icm")),
        ("caveman_settings", workspace.join(".caveman")),
        ("caveman_code_settings", workspace.join(".caveman-code")),
    ]);
    if let Some(home) = home_directory() {
        candidates.extend([
            ("rtk_state", home.join(".rtk")),
            ("rtk_config", home.join(".config/rtk")),
            ("rtk_state", home.join(".local/share/rtk")),
            ("rtk_state", home.join("Library/Application Support/rtk")),
            ("icm_state", home.join(".icm")),
            ("icm_config", home.join(".config/icm")),
            ("icm_state", home.join(".local/share/icm")),
            ("icm_state", home.join("Library/Application Support/icm")),
            ("caveman_settings", home.join(".caveman")),
            ("caveman_settings", home.join(".config/caveman")),
            ("caveman_code_settings", home.join(".config/caveman-code")),
        ]);
    }

    let mut artifacts = candidates
        .into_iter()
        .filter_map(|(kind, path)| artifact(kind, path))
        .collect::<Vec<_>>();
    for name in ["rgai", "grepai", "rtk", "icm", "caveman"] {
        if let Some(path) = resolve_binary(Path::new(name)) {
            artifacts.push(LegacyArtifact {
                kind: format!("executable_{name}"),
                path,
                symlink: false,
            });
        }
    }

    let mut marker_roots = artifacts
        .iter()
        .filter(|artifact| {
            matches!(
                artifact.kind.as_str(),
                "rtk_state" | "icm_state" | "icm_workspace"
            )
        })
        .map(|artifact| artifact.path.clone())
        .collect::<Vec<_>>();
    marker_roots.push(workspace.join(".grepai"));
    for root in marker_roots {
        collect_markers(&root, 3, &mut artifacts, &mut warnings);
    }
    artifacts.sort_by(|left, right| (&left.kind, &left.path).cmp(&(&right.kind, &right.path)));
    artifacts.dedup_by(|left, right| left.kind == right.kind && left.path == right.path);

    MigrationScan {
        schema_version: 1,
        read_only: true,
        workspace: workspace.to_path_buf(),
        identity,
        index_placement,
        duplicate_indexes,
        artifacts,
        warnings,
    }
}

fn artifact(kind: &str, path: PathBuf) -> Option<LegacyArtifact> {
    fs::symlink_metadata(&path)
        .ok()
        .map(|metadata| LegacyArtifact {
            kind: kind.into(),
            path,
            symlink: metadata.file_type().is_symlink(),
        })
}

fn collect_markers(
    root: &Path,
    depth: usize,
    artifacts: &mut Vec<LegacyArtifact>,
    warnings: &mut Vec<String>,
) {
    if depth == 0 || !root.is_dir() {
        return;
    }
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) => {
            warnings.push(format!("cannot inspect {}: {error}", root.display()));
            return;
        }
    };
    let mut seen = BTreeSet::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                warnings.push(format!(
                    "cannot inspect an entry in {}: {error}",
                    root.display()
                ));
                continue;
            }
        };
        let path = entry.path();
        let file_type = match entry.file_type() {
            Ok(file_type) => file_type,
            Err(error) => {
                warnings.push(format!("cannot inspect {}: {error}", path.display()));
                continue;
            }
        };
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_markers(&path, depth - 1, artifacts, warnings);
            continue;
        }
        let kind = match path.file_name().and_then(|name| name.to_str()) {
            Some("memories.db") => Some("icm_database"),
            Some(name) if name.ends_with(".pid") => Some("process_marker"),
            _ => None,
        };
        if let Some(kind) = kind {
            if seen.insert(path.clone()) {
                artifacts.push(LegacyArtifact {
                    kind: kind.into(),
                    path,
                    symlink: false,
                });
            }
        }
    }
}

fn home_directory() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::collect_markers;

    #[test]
    fn test_collect_markers_finds_database_without_following_symlinks() {
        let directory = tempdir().expect("temporary directory");
        let nested = directory.path().join("memory/icm");
        fs::create_dir_all(&nested).expect("create fixture");
        fs::write(nested.join("memories.db"), []).expect("write database fixture");
        let mut artifacts = Vec::new();
        let mut warnings = Vec::new();

        collect_markers(directory.path(), 3, &mut artifacts, &mut warnings);

        assert!(warnings.is_empty());
        assert!(
            artifacts
                .iter()
                .any(|artifact| artifact.kind == "icm_database")
        );
    }
}
