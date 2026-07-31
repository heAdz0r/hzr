use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use http::Uri;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IntegrationLayout {
    root: PathBuf,
}

impl IntegrationLayout {
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    #[must_use]
    pub fn bundled() -> Self {
        Self::new(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../integrations/caveman-code"))
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn bridge(&self) -> PathBuf {
        self.root.join("bridge.mjs")
    }

    #[must_use]
    pub fn package_lock(&self) -> PathBuf {
        self.root.join("package-lock.json")
    }

    #[must_use]
    pub fn installed_package(&self) -> PathBuf {
        self.root
            .join("node_modules/@juliusbrussee/caveman-code/package.json")
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct BearerToken(String);

impl BearerToken {
    pub fn new(value: String) -> Result<Self, ConfigError> {
        let valid = value.len() == 64
            && value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'+' | b'/' | b'=')
            });
        if !valid {
            return Err(ConfigError::InvalidBearerToken);
        }
        Ok(Self(value))
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for BearerToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("BearerToken([REDACTED])")
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HzrApi {
    endpoint: String,
    token: BearerToken,
}

impl HzrApi {
    pub fn new(endpoint: String, token: BearerToken) -> Result<Self, ConfigError> {
        validate_loopback_endpoint(&endpoint)?;
        Ok(Self { endpoint, token })
    }

    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub(crate) fn token(&self) -> &BearerToken {
        &self.token
    }
}

#[derive(Clone)]
pub struct ManagedAgentConfig {
    pub node: PathBuf,
    pub integration: IntegrationLayout,
    pub workspace: PathBuf,
    pub agent_data_dir: PathBuf,
    pub hzr_api: HzrApi,
    pub timeout: Duration,
    pub max_capture_bytes: usize,
}

impl ManagedAgentConfig {
    #[must_use]
    pub fn new(
        node: PathBuf,
        integration: IntegrationLayout,
        workspace: PathBuf,
        agent_data_dir: PathBuf,
        hzr_api: HzrApi,
    ) -> Self {
        Self {
            node,
            integration,
            workspace,
            agent_data_dir,
            hzr_api,
            timeout: Duration::from_secs(30 * 60),
            max_capture_bytes: 8 * 1024 * 1024,
        }
    }
}

impl fmt::Debug for ManagedAgentConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ManagedAgentConfig")
            .field("node", &self.node)
            .field("integration", &self.integration)
            .field("workspace", &self.workspace)
            .field("agent_data_dir", &self.agent_data_dir)
            .field("hzr_api", &self.hzr_api)
            .field("timeout", &self.timeout)
            .field("max_capture_bytes", &self.max_capture_bytes)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseFormat {
    #[default]
    Text,
    Json,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("HZR bearer token must be exactly 64 RFC 6750 bearer-token characters")]
    InvalidBearerToken,
    #[error("HZR daemon endpoint must be an http URL on a loopback host")]
    InvalidEndpoint,
}

fn validate_loopback_endpoint(endpoint: &str) -> Result<(), ConfigError> {
    let uri = endpoint
        .parse::<Uri>()
        .map_err(|_| ConfigError::InvalidEndpoint)?;
    if uri.scheme_str() != Some("http")
        || uri
            .path_and_query()
            .is_some_and(|path| path.path() != "/" || path.query().is_some())
    {
        return Err(ConfigError::InvalidEndpoint);
    }
    let authority = uri.authority().ok_or(ConfigError::InvalidEndpoint)?;
    let host = authority.host().trim_matches(['[', ']']);
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    if !loopback || authority.as_str().contains('@') {
        return Err(ConfigError::InvalidEndpoint);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{BearerToken, ConfigError, HzrApi};

    fn token() -> BearerToken {
        BearerToken::new("a".repeat(64)).expect("valid token")
    }

    #[test]
    fn test_hzr_api_accepts_loopback_hosts() {
        for endpoint in [
            "http://127.0.0.1:47391",
            "http://localhost:47391",
            "http://[::1]:47391",
        ] {
            assert!(HzrApi::new(endpoint.into(), token()).is_ok());
        }
    }

    #[test]
    fn test_hzr_api_rejects_remote_or_credentialed_hosts() {
        for endpoint in [
            "https://127.0.0.1:47391",
            "http://example.com:47391",
            "http://user@127.0.0.1:47391",
            "http://127.0.0.1:47391/v1",
        ] {
            assert!(matches!(
                HzrApi::new(endpoint.into(), token()),
                Err(ConfigError::InvalidEndpoint)
            ));
        }
    }

    #[test]
    fn test_bearer_token_debug_is_redacted() {
        let secret = "f".repeat(64);
        let token = BearerToken::new(secret.clone()).expect("valid token");
        let rendered = format!("{token:?}");
        assert!(!rendered.contains(&secret));
        assert!(rendered.contains("REDACTED"));
    }

    #[test]
    fn test_bearer_token_rejects_invalid_length_or_syntax() {
        assert!(BearerToken::new("short".into()).is_err());
        assert!(BearerToken::new("a".repeat(63)).is_err());
        assert!(BearerToken::new("a".repeat(65)).is_err());
        assert!(BearerToken::new(format!("{} ", "a".repeat(63))).is_err());
        assert!(BearerToken::new(format!("{}\"", "a".repeat(63))).is_err());
    }
}
