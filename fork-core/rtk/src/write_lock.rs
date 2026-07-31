use anyhow::{Context, Result};
use fs2::FileExt;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

/// File-level lock guard using sidecar `.rtk-lock` files.
/// Lock is released automatically on Drop (fs2 unlocks when fd closes).
pub struct FileLockGuard {
    _file: File, // held open to maintain flock
    #[allow(dead_code)] // used in tests via lock_path() accessor
    lock_path: PathBuf,
}

impl FileLockGuard {
    /// Acquire a blocking exclusive flock on the sidecar lock file for `target`.
    /// Uses `<target>.rtk-lock` in the same directory (NOT the file itself —
    /// atomic rename would destroy flock on the target).
    pub fn acquire(target: &Path) -> Result<Self> {
        let lock_path = lock_path_for(target);

        // Ensure parent directory exists
        if let Some(parent) = lock_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create lock dir {}", parent.display()))?;
        }

        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&lock_path)
            .with_context(|| format!("Failed to open lock file {}", lock_path.display()))?;

        // Blocking exclusive lock — waits if another process holds it
        file.lock_exclusive()
            .with_context(|| format!("Failed to acquire flock on {}", lock_path.display()))?;

        Ok(Self {
            _file: file,
            lock_path,
        })
    }

    /// Returns the path of the sidecar lock file.
    #[cfg(test)]
    pub fn lock_path(&self) -> &Path {
        &self.lock_path
    }
}

/// Compute the sidecar lock path for a target file: `<target>.rtk-lock`
pub fn lock_path_for(target: &Path) -> PathBuf {
    let mut lock = target.as_os_str().to_owned();
    lock.push(".rtk-lock");
    PathBuf::from(lock)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use tempfile::TempDir;

    #[test]
    fn lock_path_for_appends_suffix() {
        let p = Path::new("/tmp/foo.txt");
        assert_eq!(lock_path_for(p), PathBuf::from("/tmp/foo.txt.rtk-lock"));
    }

    #[test]
    fn lock_path_for_nested() {
        let p = Path::new("/a/b/c.json");
        assert_eq!(lock_path_for(p), PathBuf::from("/a/b/c.json.rtk-lock"));
    }

    #[test]
    fn acquire_and_release() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("test.txt");
        fs::write(&target, "hello").unwrap();

        let guard = FileLockGuard::acquire(&target).unwrap();
        assert!(guard.lock_path().exists());
        drop(guard);
        // Lock file may persist (standard flock practice) — that's OK
    }

    #[test]
    fn sequential_acquire_succeeds() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("seq.txt");
        fs::write(&target, "a").unwrap();

        {
            let _g1 = FileLockGuard::acquire(&target).unwrap();
        } // released
        {
            let _g2 = FileLockGuard::acquire(&target).unwrap();
        } // released
    }

    #[test]
    fn concurrent_threads_serialize() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("concurrent.txt");
        fs::write(&target, "0").unwrap();

        let target = Arc::new(target);
        let barrier = Arc::new(Barrier::new(4));
        let mut handles = Vec::new();

        for _ in 0..4 {
            let t = Arc::clone(&target);
            let b = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                b.wait(); // all threads start ~simultaneously
                let _guard = FileLockGuard::acquire(&t).unwrap();
                // Read-modify-write under lock
                let val: u32 = fs::read_to_string(t.as_ref())
                    .unwrap()
                    .trim()
                    .parse()
                    .unwrap();
                fs::write(t.as_ref(), (val + 1).to_string()).unwrap();
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        // All 4 increments must be visible (no lost updates)
        let final_val: u32 = fs::read_to_string(target.as_ref())
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_eq!(final_val, 4, "flock must serialize all 4 increments");
    }
}
