use std::path::{Component, Path, PathBuf};

use crate::error::{IndexError, Result};

pub(crate) fn normalize_filter(root: &Path, filter: Option<&Path>) -> Result<Option<PathBuf>> {
    filter
        .map(|path| normalize_within(root, path, "path filter"))
        .transpose()
}

pub(crate) fn normalize_result(root: &Path, path: &Path) -> Result<PathBuf> {
    normalize_within(root, path, "engine result path")
}

fn normalize_within(root: &Path, path: &Path, field: &'static str) -> Result<PathBuf> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(clean_relative(path, field)?)
    };
    let candidate = if candidate.exists() {
        std::fs::canonicalize(&candidate).map_err(|source| IndexError::Io {
            operation: "canonicalize search path",
            path: candidate.clone(),
            source,
        })?
    } else {
        candidate
    };
    let relative = candidate
        .strip_prefix(root)
        .map_err(|_| IndexError::InvalidInput {
            field,
            reason: format!("{} is outside {}", path.display(), root.display()),
        })?;
    if relative.as_os_str().is_empty() {
        return Ok(PathBuf::from("."));
    }
    clean_relative(relative, field)
}

fn clean_relative(path: &Path, field: &'static str) -> Result<PathBuf> {
    let mut clean = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => clean.push(value),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(IndexError::InvalidInput {
                    field,
                    reason: format!("{} escapes the canonical worktree", path.display()),
                });
            }
        }
    }
    Ok(clean)
}
