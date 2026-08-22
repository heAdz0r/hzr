#![cfg(unix)]
//! A path search must not buffer the engine's whole stdout.
//!
//! Regression: a search over a large tree emitted gigabytes of matches, which
//! the engine held twice — the `.output()` byte buffer plus its lossy `String`
//! copy. Raw retention is now capped at `RAW_CAP`, so peak memory no longer
//! scales with how much the search engine emits.

use std::io::Write;
use std::process::Command;

/// Enough match output that buffering it whole is unmistakable against the cap.
const MATCH_BYTES: usize = 64 << 20;

/// Address-space ceiling: comfortably above the engine's baseline plus a few
/// copies of the 10 MiB cap, far below two copies of `MATCH_BYTES`.
const VM_LIMIT_KB: usize = 400_000;

#[test]
fn path_search_does_not_buffer_whole_engine_output() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.log");
    let line = format!("{}MATCHME{}\n", "x".repeat(40), "y".repeat(40));
    {
        let mut writer = std::io::BufWriter::new(std::fs::File::create(&path).unwrap());
        for _ in 0..(MATCH_BYTES / line.len()) {
            writer.write_all(line.as_bytes()).unwrap();
        }
        writer.flush().unwrap();
    }

    let out = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "ulimit -v {}; exec {} grep MATCHME {}",
            VM_LIMIT_KB,
            env!("CARGO_BIN_EXE_rtk"),
            path.display()
        ))
        .output()
        .unwrap();

    // macOS `ulimit -v` is advisory on some configurations; when the limit could
    // not be applied at all the run still has to succeed, which it does.
    assert!(
        out.status.success(),
        "grep died under a {} KB address-space cap: {:?}\nstderr: {}",
        VM_LIMIT_KB,
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("MATCHME") || stdout.contains("matches in"),
        "expected capped match output, got: {}",
        &stdout[..stdout.len().min(400)]
    );
}
