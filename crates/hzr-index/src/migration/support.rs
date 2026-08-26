use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path};

use serde::Serialize;
use serde::de::DeserializeOwned;

use super::{IndexError, Result, conflict};

#[cfg(unix)]
use std::fs::File;

pub(super) fn write_new_manifest<T: Serialize>(path: &Path, manifest: &T) -> Result<()> {
    let mut encoded = serde_json::to_vec_pretty(manifest)
        .map_err(|error| conflict(format!("cannot encode migration manifest: {error}")))?;
    encoded.push(b'\n');
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|source| IndexError::Io {
            operation: "create migration manifest",
            path: path.to_path_buf(),
            source,
        })?;
    set_private_file_mode(path)?;
    file.write_all(&encoded).map_err(|source| IndexError::Io {
        operation: "write migration manifest",
        path: path.to_path_buf(),
        source,
    })?;
    file.sync_all().map_err(|source| IndexError::Io {
        operation: "sync migration manifest",
        path: path.to_path_buf(),
        source,
    })?;
    sync_directory(path.parent().unwrap_or_else(|| Path::new(".")))
}

pub(super) fn read_manifest<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(IndexError::Io {
                operation: "read migration manifest",
                path: path.to_path_buf(),
                source,
            });
        }
    };
    serde_json::from_slice(&bytes).map(Some).map_err(|error| {
        conflict(format!(
            "invalid migration manifest {}: {error}",
            path.display()
        ))
    })
}

pub(super) fn ensure_safe_target(
    workspace_root: &Path,
    data_root: &Path,
    target: &Path,
) -> Result<()> {
    ensure_target_relationships(workspace_root, data_root)?;
    if target.starts_with(workspace_root) || workspace_root.starts_with(target) {
        return Err(conflict(format!(
            "canonical target overlaps the workspace: {}",
            target.display()
        )));
    }
    if !target.starts_with(data_root) {
        return Err(conflict(format!(
            "canonical target {} escapes data root {}",
            target.display(),
            data_root.display()
        )));
    }
    ensure_no_symlink_components(data_root, target.parent().unwrap_or(data_root))
}

pub(super) fn ensure_target_relationships(workspace_root: &Path, data_root: &Path) -> Result<()> {
    if data_root.starts_with(workspace_root) {
        return Err(conflict(format!(
            "HZR data root must be outside the workspace: {}",
            data_root.display()
        )));
    }
    Ok(())
}

pub(super) fn ensure_no_symlink_components(root: &Path, path: &Path) -> Result<()> {
    let relative = path.strip_prefix(root).map_err(|_| {
        conflict(format!(
            "path {} escapes managed root {}",
            path.display(),
            root.display()
        ))
    })?;
    let root_metadata = metadata(root, "inspect managed data root")?;
    if !root_metadata.file_type().is_dir() || root_metadata.file_type().is_symlink() {
        return Err(conflict(format!(
            "managed data root is not a real directory: {}",
            root.display()
        )));
    }
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(component) = component else {
            return Err(conflict(format!("unsafe managed path: {}", path.display())));
        };
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(entry) if entry.file_type().is_symlink() || !entry.file_type().is_dir() => {
                return Err(conflict(format!(
                    "managed path contains a foreign entry: {}",
                    current.display()
                )));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(source) => {
                return Err(IndexError::Io {
                    operation: "inspect managed path",
                    path: current,
                    source,
                });
            }
        }
    }
    Ok(())
}

pub(super) fn ensure_no_prefixed_entry(parent: &Path, prefix: &str) -> Result<()> {
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(IndexError::Io {
                operation: "inspect migration conflict directory",
                path: parent.to_path_buf(),
                source,
            });
        }
    };
    for entry in entries {
        let entry = entry.map_err(|source| IndexError::Io {
            operation: "inspect migration conflict entry",
            path: parent.to_path_buf(),
            source,
        })?;
        if entry.file_name().to_string_lossy().starts_with(prefix) {
            return Err(conflict(format!(
                "partial or conflicting migration entry exists: {}",
                entry.path().display()
            )));
        }
    }
    Ok(())
}

pub(super) fn ensure_absent(path: &Path, label: &str) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(conflict(format!(
            "{label} already exists: {}",
            path.display()
        ))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(IndexError::Io {
            operation: "inspect migration path",
            path: path.to_path_buf(),
            source,
        }),
    }
}

pub(super) fn metadata(path: &Path, operation: &'static str) -> Result<fs::Metadata> {
    fs::symlink_metadata(path).map_err(|source| IndexError::Io {
        operation,
        path: path.to_path_buf(),
        source,
    })
}

pub(super) fn create_directory(path: &Path, operation: &'static str) -> Result<()> {
    fs::create_dir_all(path).map_err(|source| IndexError::Io {
        operation,
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(unix)]
pub(super) fn encoded_os(value: &OsStr) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;

    value.as_bytes().to_vec()
}

#[cfg(windows)]
pub(super) fn encoded_os(value: &OsStr) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;

    value.encode_wide().flat_map(u16::to_le_bytes).collect()
}

#[cfg(unix)]
pub(super) const fn platform_path_encoding() -> &'static str {
    "unix_bytes"
}

#[cfg(windows)]
pub(super) const fn platform_path_encoding() -> &'static str {
    "windows_utf16le"
}

#[cfg(unix)]
fn set_private_file_mode(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|source| IndexError::Io {
        operation: "set private migration manifest permissions",
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(not(unix))]
fn set_private_file_mode(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
pub(super) fn create_directory_symlink(target: &Path, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link).map_err(|source| IndexError::Io {
        operation: "activate canonical grepai index",
        path: link.to_path_buf(),
        source,
    })
}

#[cfg(windows)]
pub(super) fn create_directory_symlink(target: &Path, link: &Path) -> Result<()> {
    std::os::windows::fs::symlink_dir(target, link).map_err(|source| IndexError::Io {
        operation: "activate canonical grepai index",
        path: link.to_path_buf(),
        source,
    })
}

#[cfg(unix)]
pub(super) fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|source| IndexError::Io {
            operation: "sync migration directory",
            path: path.to_path_buf(),
            source,
        })
}

#[cfg(not(unix))]
pub(super) fn sync_directory(_path: &Path) -> Result<()> {
    Ok(())
}
