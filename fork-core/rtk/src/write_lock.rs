use anyhow::{Context, Result};
use fs2::FileExt;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

/// File-level lock guard using a per-user lock file keyed by the target's path.
/// Lock is released automatically on Drop (fs2 unlocks when fd closes).
pub struct FileLockGuard {
    _file: File, // held open to maintain flock
    #[allow(dead_code)] // used in tests via lock_path() accessor
    lock_path: PathBuf,
}

impl FileLockGuard {
    /// Acquire a blocking exclusive flock on the lock file for `target`.
    /// The lock is never the target itself — atomic rename would destroy flock on it.
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

/// Compute the lock path for a target file.
///
/// 0.10.0: locks live in a per-user directory, named by the SHA-256 of the target's
/// canonical path. The old `<target>.rtk-lock` sidecar was never removed (removing a flock
/// file races with the next locker), so every edited file left a zero-byte sibling in the
/// caller's repository — 224 of them in HZR's own tree, two committed by accident.
pub fn lock_path_for(target: &Path) -> PathBuf {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(canonical_target(target).as_os_str().as_encoded_bytes());
    let name: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    lock_dir().join(format!("{name}.lock"))
}

/// The target's path with its parent canonicalized, so `./a`, `a` and a symlinked parent
/// all name one lock. The target itself may not exist yet (create).
fn canonical_target(target: &Path) -> PathBuf {
    if let Ok(path) = fs::canonicalize(target) {
        return path;
    }
    let absolute = std::path::absolute(target).unwrap_or_else(|_| target.to_path_buf());
    match (absolute.parent(), absolute.file_name()) {
        (Some(parent), Some(name)) => fs::canonicalize(parent)
            .map(|parent| parent.join(name))
            .unwrap_or(absolute),
        _ => absolute,
    }
}

/// `RTK_LOCK_DIR`, else the user cache directory, else the temp directory.
fn lock_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("RTK_LOCK_DIR").filter(|dir| !dir.is_empty()) {
        return PathBuf::from(dir);
    }
    dirs::cache_dir()
        .map(|dir| dir.join("rtk").join("write-locks"))
        .unwrap_or_else(|| std::env::temp_dir().join("rtk-write-locks"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use tempfile::TempDir;

    // 0.10.0: the lock never lands next to the target
    #[test]
    fn lock_path_is_outside_the_target_directory() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("foo.txt");
        let lock = lock_path_for(&target);
        assert!(!lock.starts_with(tmp.path()), "{}", lock.display());
        assert_eq!(lock.extension().and_then(|e| e.to_str()), Some("lock"));
    }

    #[test]
    fn equivalent_spellings_share_one_lock() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir(tmp.path().join("sub")).unwrap();
        let direct = tmp.path().join("sub/c.json");
        let dotted = tmp.path().join("sub/../sub/./c.json");
        assert_eq!(lock_path_for(&direct), lock_path_for(&dotted));
        assert_ne!(lock_path_for(&direct), lock_path_for(&tmp.path().join("d.json")));
    }

    #[test]
    fn acquire_leaves_no_file_beside_the_target() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("clean.txt");
        fs::write(&target, "x").unwrap();
        drop(FileLockGuard::acquire(&target).unwrap());
        let entries: Vec<_> = fs::read_dir(tmp.path()).unwrap().collect();
        assert_eq!(entries.len(), 1, "only the target remains");
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
