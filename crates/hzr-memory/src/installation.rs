use std::fs::File;
use std::io::Read;
use std::process::Stdio;

use sha2::{Digest, Sha256};
use tokio::process::Command;

use crate::config::IcmConfig;
use crate::error::{MemoryError, Result};
use crate::release::{ICM_VERSION, IcmInstallation};

pub async fn verify_installation(config: &IcmConfig) -> Result<IcmInstallation> {
    config.validate()?;
    let mut command = Command::new(&config.executable);
    command
        .arg("--version")
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(config.cli_timeout, command.output())
        .await
        .map_err(|_| MemoryError::VersionProbeTimeout {
            timeout: config.cli_timeout,
        })?
        .map_err(|source| MemoryError::BinaryUnavailable {
            executable: config.executable.clone(),
            source,
        })?;
    if !output.status.success() {
        return Err(MemoryError::VersionProbeFailed {
            status: output.status,
            stderr: bounded_text(&output.stderr, 8 * 1024),
        });
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut fields = text.split_whitespace();
    let (Some("icm"), Some(version), None) = (fields.next(), fields.next(), fields.next()) else {
        return Err(MemoryError::InvalidVersionOutput {
            output: text.trim().to_owned(),
        });
    };
    let version = version.to_owned();
    if version != ICM_VERSION {
        return Err(MemoryError::VersionMismatch {
            expected: ICM_VERSION,
            actual: version,
        });
    }

    let sha256 = match &config.expected_executable_sha256 {
        Some(expected) => {
            if !config.executable_is_explicit_path() {
                return Err(MemoryError::InvalidConfig(
                    "checksum verification requires an explicit executable path".into(),
                ));
            }
            let path = config.executable.clone();
            let actual = tokio::task::spawn_blocking(move || sha256_file(&path))
                .await
                .map_err(|error| MemoryError::ChecksumTask(error.to_string()))??;
            if !actual.eq_ignore_ascii_case(expected) {
                return Err(MemoryError::ChecksumMismatch {
                    expected: expected.to_ascii_lowercase(),
                    actual,
                });
            }
            Some(actual)
        }
        None => None,
    };

    Ok(IcmInstallation {
        executable: config.executable.clone(),
        version,
        sha256,
    })
}

pub(crate) fn sha256_file(path: &std::path::Path) -> Result<String> {
    let mut file = File::open(path).map_err(|source| MemoryError::Io {
        operation: "open ICM executable for checksum verification",
        path: path.to_path_buf(),
        source,
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|source| MemoryError::Io {
            operation: "read ICM executable for checksum verification",
            path: path.to_path_buf(),
            source,
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub(crate) fn bounded_text(bytes: &[u8], limit: usize) -> String {
    String::from_utf8_lossy(bytes).chars().take(limit).collect()
}
