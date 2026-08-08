use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::Deserialize;
use sha2::{Digest, Sha256};
use thiserror::Error;
use tokio::process::Command;
use tokio::time::timeout;

use crate::IntegrationLayout;
use crate::process::{ProcessGroupGuard, configure_process_group};

pub const CAVEMAN_CODE_NPM_VERSION: &str = "0.65.2";
pub const CAVEMAN_CODE_NPM_INTEGRITY: &str = "sha512-rs7sOI7WCycpBq8qNQ3MQagxfiXAgymfyj2BjPnoaVNNPsgtFK08calhYGEhMkrH0N6prHt0KHJm4AOuuMNEpw==";
pub const PACKAGE_LOCK_SHA256: &str =
    "c0523558139f1f6d957488f224f9b1fc9b4ade5b0a3316758ca20f56937beed1";
pub const NODE_MINIMUM_VERSION: NodeVersion = NodeVersion {
    major: 20,
    minor: 18,
    patch: 1,
};
pub const NODE_MAXIMUM_VERSION_EXCLUSIVE: NodeVersion = NodeVersion {
    major: 26,
    minor: 0,
    patch: 0,
};

const BUNDLED_BRIDGE: &[u8] = include_bytes!("../../../integrations/caveman-code/bridge.mjs");
#[cfg(test)]
const BUNDLED_PACKAGE_LOCK: &[u8] =
    include_bytes!("../../../integrations/caveman-code/package-lock.json");

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct NodeVersion {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeMetadata {
    pub version: String,
    pub integrity: String,
    pub package_lock_sha256: String,
    pub bridge_sha256: String,
    pub installed_package: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PreflightReport {
    pub node_version: NodeVersion,
    pub bridge: PathBuf,
    pub runtime: RuntimeMetadata,
}

#[derive(Debug, Error)]
pub enum PreflightError {
    #[error("failed to inspect {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid JSON in {path}: {source}")]
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
    #[error("Node version command failed: {0}")]
    NodeCommand(std::io::Error),
    #[error("Node version command exceeded five seconds")]
    NodeTimeout,
    #[error("Node version command exited unsuccessfully: {0}")]
    NodeStatus(String),
    #[error("invalid Node version output: {0}")]
    InvalidNodeVersion(String),
    #[error("Node {actual} is too old; HZR requires >=20.18.1 for the exact npm lock")]
    NodeTooOld { actual: String },
    #[error("Node {actual} is unsupported; HZR requires Node <26")]
    NodeTooNew { actual: String },
    #[error("artifact digest mismatch for {path}: expected {expected}, found {actual}")]
    DigestMismatch {
        path: PathBuf,
        expected: String,
        actual: String,
    },
    #[error("Caveman package is absent from package-lock.json")]
    MissingLockEntry,
    #[error("Caveman package pin mismatch: expected {expected}, found {actual}")]
    PinMismatch { expected: String, actual: String },
}

#[derive(Deserialize)]
struct PackageLock {
    packages: BTreeMap<String, LockedPackage>,
}

#[derive(Deserialize)]
struct LockedPackage {
    version: Option<String>,
    integrity: Option<String>,
}

#[derive(Deserialize)]
struct PackageManifest {
    version: String,
}

pub async fn preflight(
    node: &Path,
    integration: &IntegrationLayout,
) -> Result<PreflightReport, PreflightError> {
    let node_version = inspect_node(node).await?;
    let bridge = canonical_file(&integration.bridge())?;
    let bridge_bytes = read_file(&bridge)?;
    let bridge_sha256 = verify_embedded_artifact(&bridge, &bridge_bytes, BUNDLED_BRIDGE)?;

    let package_lock_path = canonical_file(&integration.package_lock())?;
    let package_lock_bytes = read_file(&package_lock_path)?;
    let package_lock_sha256 =
        verify_digest(&package_lock_path, &package_lock_bytes, PACKAGE_LOCK_SHA256)?;
    let package_lock = parse_json::<PackageLock>(&package_lock_path, &package_lock_bytes)?;
    let lock_entry = package_lock
        .packages
        .get("node_modules/@juliusbrussee/caveman-code")
        .ok_or(PreflightError::MissingLockEntry)?;
    let lock_version = lock_entry
        .version
        .as_deref()
        .ok_or(PreflightError::MissingLockEntry)?;
    let lock_integrity = lock_entry
        .integrity
        .as_deref()
        .ok_or(PreflightError::MissingLockEntry)?;
    verify_pin(lock_version, lock_integrity)?;

    let installed_package = canonical_file(&integration.installed_package())?;
    let manifest = read_json::<PackageManifest>(&installed_package)?;
    if manifest.version != CAVEMAN_CODE_NPM_VERSION {
        return Err(PreflightError::PinMismatch {
            expected: CAVEMAN_CODE_NPM_VERSION.into(),
            actual: manifest.version,
        });
    }

    Ok(PreflightReport {
        node_version,
        bridge,
        runtime: RuntimeMetadata {
            version: lock_version.into(),
            integrity: lock_integrity.into(),
            package_lock_sha256,
            bridge_sha256,
            installed_package,
        },
    })
}

async fn inspect_node(node: &Path) -> Result<NodeVersion, PreflightError> {
    let mut command = Command::new(node);
    command
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    configure_process_group(&mut command);
    let child = command.spawn().map_err(PreflightError::NodeCommand)?;
    let mut process_group = ProcessGroupGuard::new(&child).map_err(PreflightError::NodeCommand)?;
    let output = timeout(Duration::from_secs(5), child.wait_with_output())
        .await
        .map_err(|_| PreflightError::NodeTimeout)?
        .map_err(PreflightError::NodeCommand)?;
    process_group
        .finish()
        .map_err(PreflightError::NodeCommand)?;
    if !output.status.success() {
        return Err(PreflightError::NodeStatus(
            String::from_utf8_lossy(&output.stderr).trim().into(),
        ));
    }
    let rendered = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    validate_node_version(&rendered)
}

fn validate_node_version(rendered: &str) -> Result<NodeVersion, PreflightError> {
    let version = parse_node_version(rendered)?;
    if version < NODE_MINIMUM_VERSION {
        return Err(PreflightError::NodeTooOld {
            actual: rendered.into(),
        });
    }
    if version >= NODE_MAXIMUM_VERSION_EXCLUSIVE {
        return Err(PreflightError::NodeTooNew {
            actual: rendered.into(),
        });
    }
    Ok(version)
}

fn parse_node_version(rendered: &str) -> Result<NodeVersion, PreflightError> {
    let mut values = rendered.strip_prefix('v').unwrap_or(rendered).split('.');
    let Some(major) = values.next() else {
        return Err(PreflightError::InvalidNodeVersion(rendered.into()));
    };
    let Some(minor) = values.next() else {
        return Err(PreflightError::InvalidNodeVersion(rendered.into()));
    };
    let Some(patch) = values.next() else {
        return Err(PreflightError::InvalidNodeVersion(rendered.into()));
    };
    if values.next().is_some() {
        return Err(PreflightError::InvalidNodeVersion(rendered.into()));
    }
    Ok(NodeVersion {
        major: major
            .parse()
            .map_err(|_| PreflightError::InvalidNodeVersion(rendered.into()))?,
        minor: minor
            .parse()
            .map_err(|_| PreflightError::InvalidNodeVersion(rendered.into()))?,
        patch: patch
            .parse()
            .map_err(|_| PreflightError::InvalidNodeVersion(rendered.into()))?,
    })
}

fn canonical_file(path: &Path) -> Result<PathBuf, PreflightError> {
    let canonical = fs::canonicalize(path).map_err(|source| PreflightError::Io {
        path: path.into(),
        source,
    })?;
    let metadata = fs::metadata(&canonical).map_err(|source| PreflightError::Io {
        path: canonical.clone(),
        source,
    })?;
    if !metadata.is_file() {
        return Err(PreflightError::Io {
            path: canonical,
            source: std::io::Error::other("expected a regular file"),
        });
    }
    Ok(canonical)
}

fn read_file(path: &Path) -> Result<Vec<u8>, PreflightError> {
    fs::read(path).map_err(|source| PreflightError::Io {
        path: path.into(),
        source,
    })
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, PreflightError> {
    parse_json(path, &read_file(path)?)
}

fn parse_json<T: for<'de> Deserialize<'de>>(
    path: &Path,
    content: &[u8],
) -> Result<T, PreflightError> {
    serde_json::from_slice(content).map_err(|source| PreflightError::Json {
        path: path.into(),
        source,
    })
}

fn verify_embedded_artifact(
    path: &Path,
    actual: &[u8],
    expected: &[u8],
) -> Result<String, PreflightError> {
    let expected = sha256(expected);
    verify_digest(path, actual, &expected)
}

fn verify_digest(path: &Path, content: &[u8], expected: &str) -> Result<String, PreflightError> {
    let actual = sha256(content);
    if actual != expected {
        return Err(PreflightError::DigestMismatch {
            path: path.into(),
            expected: expected.into(),
            actual,
        });
    }
    Ok(actual)
}

fn sha256(content: &[u8]) -> String {
    format!("{:x}", Sha256::digest(content))
}

fn verify_pin(version: &str, integrity: &str) -> Result<(), PreflightError> {
    if version != CAVEMAN_CODE_NPM_VERSION {
        return Err(PreflightError::PinMismatch {
            expected: CAVEMAN_CODE_NPM_VERSION.into(),
            actual: version.into(),
        });
    }
    if integrity != CAVEMAN_CODE_NPM_INTEGRITY {
        return Err(PreflightError::PinMismatch {
            expected: CAVEMAN_CODE_NPM_INTEGRITY.into(),
            actual: integrity.into(),
        });
    }
    Ok(())
}

impl std::fmt::Display for NodeVersion {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        BUNDLED_PACKAGE_LOCK, NODE_MAXIMUM_VERSION_EXCLUSIVE, NODE_MINIMUM_VERSION,
        PACKAGE_LOCK_SHA256, PreflightError, parse_node_version, sha256, validate_node_version,
        verify_digest,
    };

    #[test]
    fn test_parse_node_version_valid_semver() {
        assert_eq!(
            parse_node_version("v20.18.1").expect("valid Node version"),
            NODE_MINIMUM_VERSION
        );
    }

    #[test]
    fn test_parse_node_version_rejects_incomplete_semver() {
        assert!(parse_node_version("v20.6").is_err());
        assert!(parse_node_version("v20.18.1.1").is_err());
    }

    #[test]
    fn test_supported_node_range_keeps_node_25_and_excludes_node_26() {
        let node_25 = validate_node_version("v25.5.0").expect("supported Node 25 version");

        assert!(node_25 >= NODE_MINIMUM_VERSION);
        assert!(node_25 < NODE_MAXIMUM_VERSION_EXCLUSIVE);
        assert!(matches!(
            validate_node_version("v26.0.0"),
            Err(PreflightError::NodeTooNew { .. })
        ));
        assert!(matches!(
            validate_node_version("v20.18.0"),
            Err(PreflightError::NodeTooOld { .. })
        ));
    }

    #[test]
    fn test_package_lock_digest_matches_compiled_provenance() {
        assert_eq!(sha256(BUNDLED_PACKAGE_LOCK), PACKAGE_LOCK_SHA256);
    }

    #[test]
    fn test_package_lock_digest_rejects_tampering() {
        assert!(matches!(
            verify_digest(
                Path::new("package-lock.json"),
                b"tampered",
                PACKAGE_LOCK_SHA256
            ),
            Err(PreflightError::DigestMismatch { .. })
        ));
    }
}
