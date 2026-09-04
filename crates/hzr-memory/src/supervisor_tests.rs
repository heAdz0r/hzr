#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::{Json, Router, http::StatusCode, routing::get};
use serde_json::json;
use tempfile::TempDir;

use super::{IcmSupervisor, ManagedProcess, ServiceStatus, StartOutcome, StopOutcome};
use crate::runtime::{ProcessIdentity, RuntimeState};
use crate::{IcmConfig, IcmTransport, MemoryError};

struct Fixture {
    directory: TempDir,
    config: IcmConfig,
    unhealthy: Arc<AtomicBool>,
    server: tokio::task::JoinHandle<()>,
}

impl Fixture {
    async fn new() -> anyhow::Result<Self> {
        let directory = TempDir::new()?;
        let executable = directory.path().join("fake-icm");
        fs::write(
            &executable,
            "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'icm 0.10.61'; exit 0; fi\nexec sleep 60\n",
        )?;
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let mut config = IcmConfig::from_data_root(executable, directory.path());
        config.bind_addr = listener.local_addr()?;
        config.transport = IcmTransport::Http;
        config.cli_fallback = false;
        config.request_timeout = Duration::from_millis(200);
        config.startup_timeout = Duration::from_secs(3);
        config.shutdown_timeout = Duration::from_secs(1);
        let unhealthy = Arc::new(AtomicBool::new(false));
        let health = Arc::clone(&unhealthy);
        let application = Router::new()
            .route(
                "/health",
                get(move || {
                    let unhealthy = Arc::clone(&health);
                    async move {
                        let status = if unhealthy.load(Ordering::Acquire) {
                            StatusCode::SERVICE_UNAVAILABLE
                        } else {
                            StatusCode::OK
                        };
                        (status, Json(json!({"status":"ok","has_embedder":false})))
                    }
                }),
            )
            .route(
                "/stats",
                get(|| async {
                    Json(json!({"total_memories":0,"total_topics":0,"avg_weight":0.0}))
                }),
            );
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, application).await;
        });
        Ok(Self {
            directory,
            config,
            unhealthy,
            server,
        })
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

#[tokio::test]
async fn crash_identity_restores_endpoint_and_attaches_without_second_child() -> anyhow::Result<()>
{
    let fixture = Fixture::new().await?;
    let owner = IcmSupervisor::new(fixture.config.clone())?;
    let StartOutcome::Started { pid, .. } = owner.start().await? else {
        anyhow::bail!("expected child");
    };
    let managed = std::mem::replace(&mut *owner.process.lock().await, ManagedProcess::Stopped);
    let ManagedProcess::Owned {
        mut child, lock, ..
    } = managed
    else {
        anyhow::bail!("expected owned");
    };
    drop(lock);
    drop(owner);

    let mut next_config = fixture.config.clone();
    next_config
        .bind_addr
        .set_port(if next_config.bind_addr.port() == 11435 {
            11436
        } else {
            11435
        });
    let next = IcmSupervisor::new(next_config)?;
    assert_eq!(next.config.bind_addr, fixture.config.bind_addr);
    assert!(matches!(next.start().await?, StartOutcome::Attached { .. }));
    assert!(ProcessIdentity::capture(pid)?.is_some());
    fixture.unhealthy.store(true, Ordering::Release);
    assert!(!next.stop_unready_owned().await?);
    assert!(matches!(
        next.restart().await,
        Err(MemoryError::NotProcessOwner)
    ));
    assert_eq!(next.stop().await?, StopOutcome::Detached);
    assert!(
        child.try_wait()?.is_none(),
        "an attached process must never be killed"
    );
    let original = fs::read_to_string(&fixture.config.executable)?;
    fs::write(&fixture.config.executable, format!("{original}\n"))?;
    let incompatible = IcmSupervisor::new(fixture.config.clone())?;
    assert!(matches!(
        incompatible.start().await,
        Err(MemoryError::OwnershipUncertain(_))
    ));
    assert!(
        child.try_wait()?.is_none(),
        "changing the installed executable must not kill the orphan"
    );
    child.kill().await?;
    child.wait().await?;
    Ok(())
}

#[tokio::test]
async fn pending_launch_and_live_legacy_pid_refuse_another_writer() -> anyhow::Result<()> {
    let fixture = Fixture::new().await?;
    let supervisor = IcmSupervisor::new(fixture.config.clone())?;
    RuntimeState::starting(&fixture.config, supervisor.layout())?.persist(supervisor.layout())?;
    assert!(matches!(
        supervisor.start().await,
        Err(MemoryError::OwnershipUncertain(_))
    ));
    assert!(!supervisor.layout().pid_file.exists());
    RuntimeState::remove(supervisor.layout());
    fs::write(
        &supervisor.layout().pid_file,
        std::process::id().to_string(),
    )?;
    assert!(matches!(
        supervisor.start().await,
        Err(MemoryError::OwnershipUncertain(_))
    ));
    assert!(ProcessIdentity::capture(std::process::id())?.is_some());
    Ok(())
}

#[tokio::test]
async fn mismatched_process_start_is_never_signalled_or_attached() -> anyhow::Result<()> {
    let fixture = Fixture::new().await?;
    let supervisor = IcmSupervisor::new(fixture.config.clone())?;
    let mut previous = RuntimeState::starting(&fixture.config, supervisor.layout())?;
    previous.process = Some(ProcessIdentity {
        pid: std::process::id(),
        start: "different-boot-or-start".into(),
    });
    previous.persist(supervisor.layout())?;
    assert!(matches!(
        supervisor.start().await?,
        StartOutcome::Started { .. }
    ));
    assert!(ProcessIdentity::capture(std::process::id())?.is_some());
    supervisor.stop().await?;
    Ok(())
}

#[tokio::test]
async fn unhealthy_owned_child_is_reaped_and_replaced() -> anyhow::Result<()> {
    let fixture = Fixture::new().await?;
    let supervisor = IcmSupervisor::new(fixture.config.clone())?;
    let StartOutcome::Started { pid: first, .. } = supervisor.start().await? else {
        anyhow::bail!("child");
    };
    assert!(!supervisor.stop_unready_owned().await?);
    fixture.unhealthy.store(true, Ordering::Release);
    assert!(supervisor.stop_unready_owned().await?);
    assert!(ProcessIdentity::capture(first)?.is_none());
    assert!(matches!(supervisor.status().await, ServiceStatus::Stopped));
    fixture.unhealthy.store(false, Ordering::Release);
    let StartOutcome::Started { pid: second, .. } = supervisor.start().await? else {
        anyhow::bail!("replacement");
    };
    assert_ne!(first, second);
    assert_eq!(supervisor.stop().await?, StopOutcome::Stopped);
    assert!(
        !fixture
            .directory
            .path()
            .join("memory/icm/runtime/service.json")
            .exists()
    );
    Ok(())
}

#[test]
fn legacy_pid_rejects_symlinks_oversized_and_invalid_values() -> anyhow::Result<()> {
    let directory = TempDir::new()?;
    let layout = crate::IcmLayout::prepare(directory.path())?;
    assert_eq!(crate::runtime::legacy_pid(&layout)?, None);
    for invalid in [
        "0",
        "-1",
        "2147483648",
        "invalid",
        "12345678901234567890123456789012345",
    ] {
        fs::write(&layout.pid_file, invalid)?;
        assert!(crate::runtime::legacy_pid(&layout).is_err());
    }
    fs::remove_file(&layout.pid_file)?;
    let outside = directory.path().join("outside-pid");
    fs::write(&outside, "123")?;
    std::os::unix::fs::symlink(&outside, &layout.pid_file)?;
    assert!(crate::runtime::legacy_pid(&layout).is_err());
    fs::remove_file(&layout.pid_file)?;
    fs::write(&layout.pid_file, "123\n")?;
    assert_eq!(crate::runtime::legacy_pid(&layout)?, Some(123));
    Ok(())
}

#[test]
fn runtime_manifest_rejects_symlink_and_wrong_database() -> anyhow::Result<()> {
    let directory = TempDir::new()?;
    let layout = crate::IcmLayout::prepare(directory.path())?;
    let outside = directory.path().join("outside");
    fs::write(&outside, "{}")?;
    let path = layout.runtime_dir.join("service.json");
    std::os::unix::fs::symlink(&outside, &path)?;
    assert!(RuntimeState::load(&layout).is_err());
    fs::remove_file(&path)?;
    fs::write(
        &path,
        r#"{"version":1,"database":"/wrong/database","executable":"/fake/icm","endpoint":"127.0.0.1:12345","process":null}"#,
    )?;
    assert!(RuntimeState::load(&layout).is_err());
    Ok(())
}
