use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use super::support::{encoded_os, metadata, sync_directory};
use super::{IndexEntryKind, IndexError, IndexMigrationEntry, ManifestPath, Result, conflict};

pub(super) struct SnapshotEntry {
    pub(super) manifest: IndexMigrationEntry,
    relative: PathBuf,
}

pub(super) struct TreeSnapshot {
    pub(super) digest: String,
    pub(super) entries: Vec<SnapshotEntry>,
}

pub(super) fn snapshot(root: &Path) -> Result<TreeSnapshot> {
    let root_metadata = metadata(root, "inspect index tree root")?;
    if !root_metadata.file_type().is_dir() || root_metadata.file_type().is_symlink() {
        return Err(conflict(format!(
            "index tree root is not a real directory: {}",
            root.display()
        )));
    }

    let mut entries = Vec::new();
    for entry in WalkDir::new(root).follow_links(false) {
        let entry = entry.map_err(|source| IndexError::Io {
            operation: "walk grepai index tree",
            path: source
                .path()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| root.to_path_buf()),
            source: source
                .into_io_error()
                .unwrap_or_else(|| std::io::Error::other("directory traversal failed")),
        })?;
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .map_err(|_| conflict(format!("{} escaped {}", path.display(), root.display())))?
            .to_path_buf();
        validate_relative_path(&relative)?;
        let entry_metadata = metadata(path, "inspect grepai index entry")?;
        let file_type = entry_metadata.file_type();
        let (kind, size, sha256, symlink_target) = if file_type.is_dir() {
            (IndexEntryKind::Directory, 0, None, None)
        } else if file_type.is_file() {
            (
                IndexEntryKind::File,
                entry_metadata.len(),
                Some(hash_file(path)?),
                None,
            )
        } else if file_type.is_symlink() {
            let target = fs::read_link(path).map_err(|source| IndexError::Io {
                operation: "read grepai index symlink",
                path: path.to_path_buf(),
                source,
            })?;
            validate_symlink_target(&relative, &target)?;
            (
                IndexEntryKind::Symlink,
                encoded_os(target.as_os_str()).len() as u64,
                None,
                Some(ManifestPath::new(&target)),
            )
        } else {
            return Err(conflict(format!(
                "unsupported special file in legacy index: {}",
                path.display()
            )));
        };
        entries.push(SnapshotEntry {
            relative: relative.clone(),
            manifest: IndexMigrationEntry {
                relative_path: ManifestPath::new(&relative),
                kind,
                mode: permission_mode(&entry_metadata),
                size,
                sha256,
                symlink_target,
            },
        });
    }
    entries.sort_by(|left, right| {
        encoded_os(left.relative.as_os_str()).cmp(&encoded_os(right.relative.as_os_str()))
    });
    let encoded = serde_json::to_vec(
        &entries
            .iter()
            .map(|entry| &entry.manifest)
            .collect::<Vec<_>>(),
    )
    .map_err(|error| conflict(format!("cannot encode index tree manifest: {error}")))?;
    Ok(TreeSnapshot {
        digest: hex::encode(Sha256::digest(encoded)),
        entries,
    })
}

pub(super) fn copy_snapshot(
    source: &Path,
    destination: &Path,
    snapshot: &TreeSnapshot,
) -> Result<()> {
    super::support::ensure_absent(destination, "index migration staging directory")?;
    fs::create_dir(destination).map_err(|source_error| IndexError::Io {
        operation: "create index migration staging directory",
        path: destination.to_path_buf(),
        source: source_error,
    })?;

    for entry in snapshot.entries.iter().filter(|entry| {
        !entry.relative.as_os_str().is_empty() && entry.manifest.kind == IndexEntryKind::Directory
    }) {
        let path = destination.join(&entry.relative);
        fs::create_dir(&path).map_err(|source_error| IndexError::Io {
            operation: "create staged index directory",
            path,
            source: source_error,
        })?;
    }
    for entry in snapshot
        .entries
        .iter()
        .filter(|entry| entry.manifest.kind != IndexEntryKind::Directory)
    {
        let source_path = source.join(&entry.relative);
        let destination_path = destination.join(&entry.relative);
        if entry.manifest.kind == IndexEntryKind::File {
            copy_file(&source_path, &destination_path, entry.manifest.mode)?;
        } else {
            let target = fs::read_link(&source_path).map_err(|source_error| IndexError::Io {
                operation: "read source symlink during migration",
                path: source_path.clone(),
                source: source_error,
            })?;
            validate_symlink_target(&entry.relative, &target)?;
            create_symlink(&source_path, &target, &destination_path)?;
        }
    }
    for entry in snapshot
        .entries
        .iter()
        .rev()
        .filter(|entry| entry.manifest.kind == IndexEntryKind::Directory)
    {
        set_permission_mode(&destination.join(&entry.relative), entry.manifest.mode)?;
    }
    for entry in snapshot
        .entries
        .iter()
        .rev()
        .filter(|entry| entry.manifest.kind == IndexEntryKind::Directory)
    {
        sync_directory(&destination.join(&entry.relative))?;
    }
    Ok(())
}

fn copy_file(source: &Path, destination: &Path, mode: u32) -> Result<()> {
    let mut input = File::open(source).map_err(|source_error| IndexError::Io {
        operation: "open legacy index file",
        path: source.to_path_buf(),
        source: source_error,
    })?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|source_error| IndexError::Io {
            operation: "create staged index file",
            path: destination.to_path_buf(),
            source: source_error,
        })?;
    std::io::copy(&mut input, &mut output).map_err(|source_error| IndexError::Io {
        operation: "copy legacy index file",
        path: destination.to_path_buf(),
        source: source_error,
    })?;
    output.sync_all().map_err(|source_error| IndexError::Io {
        operation: "sync staged index file",
        path: destination.to_path_buf(),
        source: source_error,
    })?;
    set_permission_mode(destination, mode)
}

fn hash_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).map_err(|source| IndexError::Io {
        operation: "open grepai index file for hashing",
        path: path.to_path_buf(),
        source,
    })?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|source| IndexError::Io {
            operation: "hash grepai index file",
            path: path.to_path_buf(),
            source,
        })?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    Ok(hex::encode(hash.finalize()))
}

fn validate_relative_path(path: &Path) -> Result<()> {
    if path
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
        || path.as_os_str().is_empty()
    {
        return Ok(());
    }
    Err(conflict(format!(
        "unsafe relative path in legacy index: {}",
        path.display()
    )))
}

fn validate_symlink_target(relative: &Path, target: &Path) -> Result<()> {
    if target.as_os_str().is_empty() || target.is_absolute() {
        return Err(conflict(format!(
            "unsafe symlink target at {}: {}",
            relative.display(),
            target.display()
        )));
    }
    let mut depth = relative.parent().map_or(0, |parent| {
        parent
            .components()
            .filter(|component| matches!(component, Component::Normal(_)))
            .count()
    });
    for component in target.components() {
        match component {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir if depth > 0 => depth -= 1,
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(conflict(format!(
                    "symlink at {} escapes the legacy index: {}",
                    relative.display(),
                    target.display()
                )));
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
fn permission_mode(metadata: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;

    metadata.permissions().mode() & 0o7777
}

#[cfg(not(unix))]
fn permission_mode(metadata: &fs::Metadata) -> u32 {
    if metadata.permissions().readonly() {
        0o444
    } else {
        0o666
    }
}

#[cfg(unix)]
fn set_permission_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|source| IndexError::Io {
        operation: "preserve migrated index permissions",
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(not(unix))]
fn set_permission_mode(path: &Path, mode: u32) -> Result<()> {
    let mut permissions = metadata(path, "read migrated index permissions")?.permissions();
    permissions.set_readonly(mode & 0o200 == 0);
    fs::set_permissions(path, permissions).map_err(|source| IndexError::Io {
        operation: "preserve migrated index permissions",
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(unix)]
fn create_symlink(_source: &Path, target: &Path, link: &Path) -> Result<()> {
    std::os::unix::fs::symlink(target, link).map_err(|source| IndexError::Io {
        operation: "copy safe index symlink",
        path: link.to_path_buf(),
        source,
    })
}

#[cfg(windows)]
fn create_symlink(source: &Path, target: &Path, link: &Path) -> Result<()> {
    use std::os::windows::fs::FileTypeExt;

    let file_type = metadata(source, "inspect source symlink type")?.file_type();
    let result = if file_type.is_symlink_dir() {
        std::os::windows::fs::symlink_dir(target, link)
    } else if file_type.is_symlink_file() {
        std::os::windows::fs::symlink_file(target, link)
    } else {
        return Err(conflict(format!(
            "cannot determine Windows symlink type: {}",
            source.display()
        )));
    };
    result.map_err(|source| IndexError::Io {
        operation: "copy safe index symlink",
        path: link.to_path_buf(),
        source,
    })
}
