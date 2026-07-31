use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};

use fs2::FileExt;
use uuid::Uuid;

use crate::error::{MemoryError, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IcmLayout {
    pub root: PathBuf,
    pub database: PathBuf,
    pub runtime_dir: PathBuf,
    pub lock_file: PathBuf,
    pub pid_file: PathBuf,
    pub token_file: PathBuf,
    pub token_lock_file: PathBuf,
    pub log_file: PathBuf,
}

impl IcmLayout {
    pub fn prepare(data_root: &Path) -> Result<Self> {
        fs::create_dir_all(data_root).map_err(|source| MemoryError::Io {
            operation: "create HZR data root",
            path: data_root.to_path_buf(),
            source,
        })?;
        let canonical_data_root =
            fs::canonicalize(data_root).map_err(|source| MemoryError::Io {
                operation: "canonicalize HZR data root",
                path: data_root.to_path_buf(),
                source,
            })?;
        let root = canonical_data_root.join("memory").join("icm");
        let runtime_dir = root.join("runtime");
        fs::create_dir_all(&runtime_dir).map_err(|source| MemoryError::Io {
            operation: "create ICM runtime directory",
            path: runtime_dir.clone(),
            source,
        })?;
        Ok(Self {
            database: root.join("memories.db"),
            lock_file: runtime_dir.join("supervisor.lock"),
            pid_file: runtime_dir.join("icm.pid"),
            token_file: root.join("auth.token"),
            token_lock_file: runtime_dir.join("token.lock"),
            log_file: root.join("icm.log"),
            root,
            runtime_dir,
        })
    }

    pub(crate) fn load_or_create_token(&self) -> Result<String> {
        let token_lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&self.token_lock_file)
            .map_err(|source| MemoryError::Io {
                operation: "open ICM token lock",
                path: self.token_lock_file.clone(),
                source,
            })?;
        token_lock
            .lock_exclusive()
            .map_err(|source| MemoryError::Io {
                operation: "lock ICM token",
                path: self.token_lock_file.clone(),
                source,
            })?;
        let result = self.load_or_create_token_locked();
        fs2::FileExt::unlock(&token_lock).map_err(|source| MemoryError::Io {
            operation: "unlock ICM token",
            path: self.token_lock_file.clone(),
            source,
        })?;
        result
    }

    fn load_or_create_token_locked(&self) -> Result<String> {
        let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        match create_secret_file(&self.token_file) {
            Ok(mut file) => {
                file.write_all(token.as_bytes())
                    .and_then(|()| file.sync_all())
                    .map_err(|source| MemoryError::Io {
                        operation: "write ICM authentication token",
                        path: self.token_file.clone(),
                        source,
                    })?;
                Ok(token)
            }
            Err(source) if source.kind() == ErrorKind::AlreadyExists => {
                secure_existing_secret(&self.token_file).map_err(|source| MemoryError::Io {
                    operation: "secure ICM authentication token",
                    path: self.token_file.clone(),
                    source,
                })?;
                let mut token = String::new();
                File::open(&self.token_file)
                    .and_then(|mut file| file.read_to_string(&mut token))
                    .map_err(|source| MemoryError::Io {
                        operation: "read ICM authentication token",
                        path: self.token_file.clone(),
                        source,
                    })?;
                let token = token.trim().to_owned();
                if token.len() < 32 {
                    return Err(MemoryError::InvalidConfig(format!(
                        "ICM token at {} is empty or too short",
                        self.token_file.display()
                    )));
                }
                Ok(token)
            }
            Err(source) => Err(MemoryError::Io {
                operation: "create ICM authentication token",
                path: self.token_file.clone(),
                source,
            }),
        }
    }

    pub(crate) fn open_log(&self) -> Result<File> {
        const MAX_LOG_SIZE: u64 = 10 * 1024 * 1024;

        if fs::metadata(&self.log_file).is_ok_and(|metadata| metadata.len() >= MAX_LOG_SIZE) {
            let rotated = self.log_file.with_extension("log.1");
            if rotated.exists() {
                fs::remove_file(&rotated).map_err(|source| MemoryError::Io {
                    operation: "remove previous ICM log archive",
                    path: rotated.clone(),
                    source,
                })?;
            }
            fs::rename(&self.log_file, &rotated).map_err(|source| MemoryError::Io {
                operation: "rotate ICM log",
                path: self.log_file.clone(),
                source,
            })?;
        }
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_file)
            .map_err(|source| MemoryError::Io {
                operation: "open ICM log",
                path: self.log_file.clone(),
                source,
            })
    }
}

#[cfg(unix)]
fn create_secret_file(path: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;

    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(unix)]
fn secure_existing_secret(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let metadata = fs::metadata(path)?;
    if metadata.mode() & 0o777 != 0o600 {
        let mut permissions = metadata.permissions();
        permissions.set_mode(0o600);
        fs::set_permissions(path, permissions)?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn create_secret_file(path: &Path) -> std::io::Result<File> {
    OpenOptions::new().write(true).create_new(true).open(path)
}

#[cfg(not(unix))]
fn secure_existing_secret(_path: &Path) -> std::io::Result<()> {
    Ok(())
}
