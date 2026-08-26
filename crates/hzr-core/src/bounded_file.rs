use std::fs::OpenOptions;
use std::io::Read;
use std::path::{Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum BoundedFileError {
    #[error("failed to open {path}: {source}")]
    Open {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("{path} must be a regular non-symlink file")]
    NotRegular { path: PathBuf },
    #[error("{path} exceeds the {max_bytes}-byte size limit")]
    TooLarge { path: PathBuf, max_bytes: u64 },
    #[error("failed to read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
}

pub fn read_bounded_regular_file(path: &Path, max_bytes: u64) -> Result<Vec<u8>, BoundedFileError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .map_err(|source| BoundedFileError::Open {
            path: path.to_path_buf(),
            source,
        })?;
    let metadata = file.metadata().map_err(|source| BoundedFileError::Open {
        path: path.to_path_buf(),
        source,
    })?;
    if !metadata.is_file() {
        return Err(BoundedFileError::NotRegular {
            path: path.to_path_buf(),
        });
    }
    if metadata.len() > max_bytes {
        return Err(BoundedFileError::TooLarge {
            path: path.to_path_buf(),
            max_bytes,
        });
    }
    let mut bytes = Vec::new();
    file.take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|source| BoundedFileError::Read {
            path: path.to_path_buf(),
            source,
        })?;
    if bytes.len() as u64 > max_bytes {
        return Err(BoundedFileError::TooLarge {
            path: path.to_path_buf(),
            max_bytes,
        });
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_reader_rejects_oversized_files() {
        let directory = tempfile::tempdir().expect("temp directory");
        let path = directory.path().join("oversized.json");
        std::fs::write(&path, b"1234").expect("fixture");

        assert!(matches!(
            read_bounded_regular_file(&path, 3),
            Err(BoundedFileError::TooLarge { max_bytes: 3, .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn bounded_reader_rejects_symlinks_without_following_them() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("temp directory");
        let target = directory.path().join("target.json");
        let link = directory.path().join("link.json");
        std::fs::write(&target, b"{}").expect("target");
        symlink(&target, &link).expect("symlink");

        assert!(read_bounded_regular_file(&link, 1024).is_err());
    }
}
