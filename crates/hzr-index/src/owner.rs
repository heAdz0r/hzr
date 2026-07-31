use fs2::FileExt;
use std::fs::{File, OpenOptions};
use std::path::Path;

use crate::error::{IndexError, Result};
use crate::workspace::Workspace;

pub(crate) struct IndexOwner {
    file: File,
}

impl IndexOwner {
    pub(crate) fn acquire(workspace: &Workspace) -> Result<Self> {
        Self::acquire_path(&workspace.index.directory, &workspace.index.owner_lock)
    }

    pub(crate) fn acquire_path(directory: &Path, lock_path: &Path) -> Result<Self> {
        std::fs::create_dir_all(directory).map_err(|source| IndexError::Io {
            operation: "create canonical grepai directory",
            path: directory.to_path_buf(),
            source,
        })?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)
            .map_err(|source| IndexError::Io {
                operation: "open grepai owner lock",
                path: lock_path.to_path_buf(),
                source,
            })?;
        if let Err(source) = file.try_lock_exclusive() {
            if source.kind() == std::io::ErrorKind::WouldBlock {
                return Err(IndexError::IndexOwnerBusy {
                    lock_path: lock_path.to_path_buf(),
                });
            }
            return Err(IndexError::Io {
                operation: "acquire grepai owner lock",
                path: lock_path.to_path_buf(),
                source,
            });
        }

        Ok(Self { file })
    }
}

impl Drop for IndexOwner {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}
