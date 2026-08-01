use std::path::{Path, PathBuf};

const INDEX_FILE: &str = "index.html";

pub fn assets_directory() -> Option<PathBuf> {
    if let Some(configured) = std::env::var_os("HZR_VISUALIZER_DIR") {
        return valid_directory(PathBuf::from(configured));
    }

    if let Ok(executable) = std::env::current_exe() {
        let executable = executable.canonicalize().unwrap_or(executable);
        if let Some(release_root) = executable.parent().and_then(Path::parent) {
            let release_assets = release_root.join("share/hzr/visualizer");
            if let Some(assets) = valid_directory(release_assets) {
                return Some(assets);
            }
            if release_root
                .parent()
                .and_then(Path::file_name)
                .is_some_and(|name| name == "versions")
            {
                let install_root = release_root.parent().and_then(Path::parent)?;
                let stable_assets = install_root.join("current/share/hzr/visualizer");
                if let Some(assets) = valid_directory(stable_assets) {
                    return Some(assets);
                }
            }
        }
    }

    valid_directory(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../visualizer/dist"))
}

fn valid_directory(path: PathBuf) -> Option<PathBuf> {
    let resolved = path.canonicalize().ok()?;
    if resolved.is_dir() && resolved.join(INDEX_FILE).is_file() {
        Some(resolved)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::valid_directory;

    #[test]
    fn assets_require_a_real_directory_with_an_index() {
        let directory = tempdir().expect("temporary directory");
        assert!(valid_directory(directory.path().to_path_buf()).is_none());

        fs::write(directory.path().join("index.html"), "<!doctype html>")
            .expect("visualizer fixture");
        assert_eq!(
            valid_directory(directory.path().to_path_buf()),
            Some(directory.path().canonicalize().expect("canonical fixture"))
        );
    }
}
