//! 0.11.0 (US-007): a signal to rtk while it captures a child's output is relayed
//! to the child's process group; rtk prints what it captured, then dies by the
//! same signal. Upstream RTK 9900d70 / 5b1d523 / 9418aa8.
#![cfg(unix)]

use std::io::Read;
use std::os::unix::process::ExitStatusExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn sigterm_flushes_captured_output_and_dies_by_the_signal() {
    let dir = tempfile::tempdir().expect("temp dir");
    let script = dir.path().join("slow.sh");
    // The grandchild `sleep` holds the output pipe: only a group-wide relay ends it.
    std::fs::write(
        &script,
        "#!/bin/sh\necho 'error: before signal'\nsleep 30\n",
    )
    .unwrap();
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755))
        .unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .arg("err")
        .arg(&script)
        .env("RTK_DB_PATH", dir.path().join("history.sqlite"))
        .env("RTK_TEE", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn rtk");
    std::thread::sleep(Duration::from_millis(800));
    let started = Instant::now();
    // SAFETY: signalling our own child process.
    unsafe {
        libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
    }
    let status = child.wait().expect("wait rtk");
    let mut stdout = String::new();
    child.stdout.take().unwrap().read_to_string(&mut stdout).unwrap();

    assert!(started.elapsed() < Duration::from_secs(10), "rtk hung on the grandchild");
    assert_eq!(status.signal(), Some(libc::SIGTERM), "{status:?}");
    assert!(stdout.contains("error: before signal"), "captured output lost: {stdout:?}");
}
