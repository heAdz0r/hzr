use std::ffi::OsString;
use std::path::Path;
use std::process::{ExitStatus, Output, Stdio};
use std::time::Duration;

use tokio::process::Command;
use tokio::time::timeout;

use crate::error::{IndexError, Result};

const DIAGNOSTIC_LIMIT: usize = 16 * 1024;

pub(crate) async fn output(
    program: &Path,
    args: &[OsString],
    cwd: &Path,
    deadline: Duration,
    operation: &'static str,
) -> Result<Output> {
    let mut command = Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let child = command
        .spawn()
        .map_err(|source| IndexError::CommandUnavailable {
            operation,
            program: program.to_path_buf(),
            source,
        })?;

    timeout(deadline, child.wait_with_output())
        .await
        .map_err(|_| IndexError::DeadlineExceeded {
            operation,
            duration: deadline,
        })?
        .map_err(|source| IndexError::Io {
            operation,
            path: program.to_path_buf(),
            source,
        })
}

pub(crate) fn require_success(output: Output, operation: &'static str) -> Result<Vec<u8>> {
    if output.status.success() {
        return Ok(output.stdout);
    }

    Err(command_failed(operation, output.status, &output.stderr))
}

pub(crate) fn command_failed(
    operation: &'static str,
    status: ExitStatus,
    stderr: &[u8],
) -> IndexError {
    IndexError::CommandFailed {
        operation,
        code: status.code(),
        stderr: diagnostic(stderr),
    }
}

pub(crate) fn diagnostic(bytes: &[u8]) -> String {
    let start = bytes.len().saturating_sub(DIAGNOSTIC_LIMIT);
    String::from_utf8_lossy(&bytes[start..]).trim().to_owned()
}
