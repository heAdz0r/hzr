use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use fs2::FileExt;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DaemonLockError {
    #[error("another hzrd instance already owns {path}")]
    AlreadyOwned { path: PathBuf },
    #[error("unsafe daemon lock target {path}: expected a regular file")]
    UnsafeTarget { path: PathBuf },
    #[error("failed to {operation} daemon lock {path}: {source}")]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
}

pub(crate) struct DaemonLock {
    file: File,
}

impl DaemonLock {
    pub(crate) fn acquire(data_root: &Path) -> Result<Self, DaemonLockError> {
        let path = data_root.join("runtime/hzrd.lock");
        reject_unsafe_target(&path)?;

        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        configure_secure_open(&mut options);
        let file = options.open(&path).map_err(|source| {
            if is_symlink_error(&source) {
                DaemonLockError::UnsafeTarget { path: path.clone() }
            } else {
                DaemonLockError::Io {
                    operation: "open",
                    path: path.clone(),
                    source,
                }
            }
        })?;

        if !file
            .metadata()
            .map_err(|source| DaemonLockError::Io {
                operation: "inspect",
                path: path.clone(),
                source,
            })?
            .file_type()
            .is_file()
        {
            return Err(DaemonLockError::UnsafeTarget { path });
        }
        set_private_permissions(&file, &path)?;

        match FileExt::try_lock_exclusive(&file) {
            Ok(()) => Ok(Self { file }),
            Err(source) if source.kind() == io::ErrorKind::WouldBlock => {
                Err(DaemonLockError::AlreadyOwned { path })
            }
            Err(source) => Err(DaemonLockError::Io {
                operation: "lock",
                path,
                source,
            }),
        }
    }
}

impl Drop for DaemonLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

fn reject_unsafe_target(path: &Path) -> Result<(), DaemonLockError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(()),
        Ok(_) => Err(DaemonLockError::UnsafeTarget {
            path: path.to_path_buf(),
        }),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(DaemonLockError::Io {
            operation: "inspect",
            path: path.to_path_buf(),
            source,
        }),
    }
}

#[cfg(unix)]
fn configure_secure_open(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;

    options
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
}

#[cfg(not(unix))]
fn configure_secure_open(_options: &mut OpenOptions) {}

#[cfg(unix)]
fn is_symlink_error(error: &io::Error) -> bool {
    error.raw_os_error() == Some(libc::ELOOP)
}

#[cfg(not(unix))]
fn is_symlink_error(_error: &io::Error) -> bool {
    false
}

#[cfg(unix)]
fn set_private_permissions(file: &File, path: &Path) -> Result<(), DaemonLockError> {
    use std::os::unix::fs::PermissionsExt;

    file.set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(|source| DaemonLockError::Io {
            operation: "secure",
            path: path.to_path_buf(),
            source,
        })
}

#[cfg(not(unix))]
fn set_private_permissions(_file: &File, _path: &Path) -> Result<(), DaemonLockError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::{DaemonLock, DaemonLockError};

    fn data_root(directory: &TempDir) -> std::path::PathBuf {
        let root = directory.path().join("data");
        fs::create_dir_all(root.join("runtime")).expect("runtime directory");
        root
    }

    #[test]
    fn test_acquire_refuses_second_owner_and_releases_without_deleting_file() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let root = data_root(&directory);
        let first = DaemonLock::acquire(&root).expect("first lock");

        assert!(matches!(
            DaemonLock::acquire(&root),
            Err(DaemonLockError::AlreadyOwned { .. })
        ));

        drop(first);
        assert!(root.join("runtime/hzrd.lock").is_file());
        DaemonLock::acquire(&root).expect("released lock can be acquired");
    }

    #[test]
    fn test_acquire_rejects_non_regular_target() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let root = data_root(&directory);
        fs::create_dir(root.join("runtime/hzrd.lock")).expect("lock directory");

        assert!(matches!(
            DaemonLock::acquire(&root),
            Err(DaemonLockError::UnsafeTarget { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn test_acquire_rejects_symlink_target() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("temporary directory");
        let root = data_root(&directory);
        let target = directory.path().join("target");
        fs::write(&target, b"state").expect("target file");
        symlink(&target, root.join("runtime/hzrd.lock")).expect("lock symlink");

        assert!(matches!(
            DaemonLock::acquire(&root),
            Err(DaemonLockError::UnsafeTarget { .. })
        ));
        assert_eq!(fs::read(target).expect("target remains"), b"state");
    }

    #[cfg(unix)]
    #[test]
    fn test_acquire_sets_private_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().expect("temporary directory");
        let root = data_root(&directory);
        let path = root.join("runtime/hzrd.lock");
        fs::write(&path, b"").expect("existing lock file");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).expect("relaxed permissions");

        let _lock = DaemonLock::acquire(&root).expect("lock acquisition");
        let mode = fs::metadata(path)
            .expect("lock metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
