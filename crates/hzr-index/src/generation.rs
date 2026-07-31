use std::path::Path;
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{IndexError, Result};
use crate::workspace::Workspace;

pub const CACHE_SCHEMA_VERSION: u16 = 1;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct IndexGeneration {
    pub config_fingerprint: String,
    pub generation: String,
}

impl IndexGeneration {
    pub fn read(workspace: &Workspace) -> Result<Self> {
        let config_fingerprint = match std::fs::read(&workspace.index.config) {
            Ok(config) => digest(&config),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => "absent".into(),
            Err(source) => {
                return Err(IndexError::Io {
                    operation: "read grepai config",
                    path: workspace.index.config.clone(),
                    source,
                });
            }
        };

        let mut hash = Sha256::new();
        hash.update(CACHE_SCHEMA_VERSION.to_le_bytes());
        hash.update(workspace.identity.worktree_id.as_bytes());
        hash.update(config_fingerprint.as_bytes());
        for path in [
            &workspace.index.vectors,
            &workspace.index.symbols,
            &workspace.index.repository_graph,
        ] {
            update_artifact_state(&mut hash, path)?;
        }

        Ok(Self {
            config_fingerprint,
            generation: hex::encode(hash.finalize()),
        })
    }
}

fn update_artifact_state(hash: &mut Sha256, path: &Path) -> Result<()> {
    hash.update(path.as_os_str().as_encoded_bytes());
    match std::fs::metadata(path) {
        Ok(metadata) => {
            hash.update([1]);
            hash.update(metadata.len().to_le_bytes());
            let modified = metadata.modified().map_err(|source| IndexError::Io {
                operation: "read index modification time",
                path: path.to_path_buf(),
                source,
            })?;
            let nanos = modified
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            hash.update(nanos.to_le_bytes());
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => hash.update([0]),
        Err(source) => {
            return Err(IndexError::Io {
                operation: "read index metadata",
                path: path.to_path_buf(),
                source,
            });
        }
    }
    Ok(())
}

pub(crate) fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
