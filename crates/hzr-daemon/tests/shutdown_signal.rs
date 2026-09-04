#![cfg(unix)]

use std::process::Stdio;
use std::time::Duration;

use hzr_core::Config;
use tokio::process::Command;

#[tokio::test]
async fn sigterm_releases_the_daemon_lock_and_exits_successfully()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let home = directory.path().join("home");
    let xdg = directory.path().join("config");
    std::fs::create_dir_all(&home)?;
    std::fs::create_dir_all(&xdg)?;
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    let mut config = Config {
        data_dir: directory.path().join("data"),
        ..Config::default()
    };
    config.daemon.bind = listener.local_addr()?;
    config.engines.directory = Some(directory.path().join("empty-engines"));
    config.engines.auto_start_icm = false;
    config.engines.auto_index = false;
    #[cfg(target_os = "macos")]
    let config_path = home.join("Library/Application Support/dev.headz0r.hzr/config.toml");
    #[cfg(not(target_os = "macos"))]
    let config_path = xdg.join("hzr/config.toml");
    config.write(&config_path)?;
    drop(listener);

    let mut child = Command::new(env!("CARGO_BIN_EXE_hzrd"))
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &xdg)
        .env("XDG_DATA_HOME", directory.path().join("xdg-data"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let pid = child.id().ok_or("daemon PID missing")?;
    let readiness = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if child.try_wait()?.is_some() {
                return Err(std::io::Error::other(
                    "isolated daemon exited before listen",
                ));
            }
            if tokio::net::TcpStream::connect(config.daemon.bind)
                .await
                .is_ok()
            {
                return Ok::<_, std::io::Error>(());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    if !matches!(readiness, Ok(Ok(()))) {
        child.kill().await?;
        let output = child.wait_with_output().await?;
        return Err(format!(
            "isolated daemon did not start: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let signalled = Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status()
        .await?;
    assert!(signalled.success());
    let exit = tokio::time::timeout(Duration::from_secs(10), child.wait()).await??;
    assert!(
        exit.success(),
        "SIGTERM must run graceful cleanup, got {exit}"
    );
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(config.data_dir.join("runtime/hzrd.lock"))?;
    fs2::FileExt::try_lock_exclusive(&lock)?;
    fs2::FileExt::unlock(&lock)?;
    Ok(())
}
