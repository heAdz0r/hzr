use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use hzr_codec::Transform;
use hzr_core::{Config, EngineManifest};
use hzr_exec::{ExecutionOutcome, RewriteDecision};
use hzr_memory::{MemoryRecord, MemoryTransport};
use hzr_protocol::{
    CodecApiRequest, ContextPlanApiRequest, ContextPlanApiResponse, ErrorResponse, ExecApiRequest,
    ExecApprovalApiRequest, HealthResponse, MemoryForgetApiRequest, MemoryMutationApiResponse,
    MemoryPruneApiRequest, MemoryRecallApiRequest, MemoryStoreApiRequest, MemoryUpdateApiRequest,
    OperationApiRequest, OperationApiResponse, SearchApiRequest, SearchApiResponse,
};
use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub struct DaemonClient {
    http: reqwest::Client,
    endpoint: String,
    token: String,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct MemoryStoreResponse {
    pub transport: MemoryTransport,
    pub memory: Option<MemoryRecord>,
}

impl DaemonClient {
    pub fn from_config(config: &Config) -> Result<Self, ClientError> {
        if !config.daemon.bind.ip().is_loopback() {
            return Err(ClientError::NonLoopback(config.daemon.bind));
        }
        let token_path = config.data_dir.join("runtime/hzrd.token");
        let token = read_token(&token_path)?;
        let http = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!("hzr/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_millis(config.daemon.request_timeout_ms))
            .build()
            .map_err(ClientError::Build)?;
        Ok(Self {
            http,
            endpoint: format!("http://{}", config.daemon.bind),
            token,
        })
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub(crate) fn token(&self) -> &str {
        &self.token
    }

    pub async fn health(&self) -> Result<HealthResponse, ClientError> {
        self.get("/v1/health").await
    }

    pub async fn engines(&self) -> Result<EngineManifest, ClientError> {
        self.get("/v1/engines").await
    }

    pub async fn search(
        &self,
        request: &SearchApiRequest,
    ) -> Result<SearchApiResponse, ClientError> {
        self.post("/v1/search", request).await
    }

    pub async fn context_plan(
        &self,
        request: &ContextPlanApiRequest,
    ) -> Result<ContextPlanApiResponse, ClientError> {
        self.post("/v1/context/plan", request).await
    }

    pub async fn memory_recall(
        &self,
        request: &MemoryRecallApiRequest,
    ) -> Result<hzr_memory::MemoryRecallResponse, ClientError> {
        self.post("/v1/memory/recall", request).await
    }

    pub async fn memory_store(
        &self,
        request: &MemoryStoreApiRequest,
    ) -> Result<MemoryStoreResponse, ClientError> {
        self.post("/v1/memory/store", request).await
    }

    pub async fn memory_forget(
        &self,
        request: &MemoryForgetApiRequest,
    ) -> Result<MemoryMutationApiResponse, ClientError> {
        self.post("/v1/memory/forget", request).await
    }

    pub async fn memory_update(
        &self,
        request: &MemoryUpdateApiRequest,
    ) -> Result<MemoryMutationApiResponse, ClientError> {
        self.post("/v1/memory/update", request).await
    }

    pub async fn memory_prune(
        &self,
        request: &MemoryPruneApiRequest,
    ) -> Result<MemoryMutationApiResponse, ClientError> {
        self.post("/v1/memory/prune", request).await
    }

    pub async fn exec_rewrite(
        &self,
        request: &ExecApiRequest,
    ) -> Result<RewriteDecision, ClientError> {
        self.post("/v1/exec/rewrite", request).await
    }

    pub async fn exec_run(
        &self,
        request: &ExecApiRequest,
    ) -> Result<ExecutionOutcome, ClientError> {
        self.post("/v1/exec/run", request).await
    }

    pub async fn exec_approval(
        &self,
        request: &ExecApprovalApiRequest,
    ) -> Result<ExecutionOutcome, ClientError> {
        self.post("/v1/exec/approval", request).await
    }

    pub async fn codec_compile(&self, request: &CodecApiRequest) -> Result<Transform, ClientError> {
        self.post("/v1/codec/compile", request).await
    }

    pub async fn record_operation(
        &self,
        request: &OperationApiRequest,
    ) -> Result<OperationApiResponse, ClientError> {
        self.post("/v1/operations", request).await
    }

    async fn get<T: DeserializeOwned>(&self, path: &'static str) -> Result<T, ClientError> {
        self.send::<(), T>(Method::GET, path, None).await
    }

    async fn post<B: serde::Serialize + ?Sized, T: DeserializeOwned>(
        &self,
        path: &'static str,
        body: &B,
    ) -> Result<T, ClientError> {
        self.send(Method::POST, path, Some(body)).await
    }

    async fn send<B: serde::Serialize + ?Sized, T: DeserializeOwned>(
        &self,
        method: Method,
        path: &'static str,
        body: Option<&B>,
    ) -> Result<T, ClientError> {
        let mut request = self
            .http
            .request(method, format!("{}{path}", self.endpoint))
            .bearer_auth(&self.token);
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await.map_err(ClientError::Transport)?;
        let status = response.status();
        let bytes = response.bytes().await.map_err(ClientError::Transport)?;
        if !status.is_success() {
            return match serde_json::from_slice::<ErrorResponse>(&bytes) {
                Ok(error) => Err(ClientError::Api {
                    status,
                    code: error.code,
                    message: error.message,
                    recoverable: error.recoverable,
                }),
                Err(_) => Err(ClientError::Http {
                    status,
                    body: bounded_diagnostic(&bytes),
                }),
            };
        }
        serde_json::from_slice(&bytes).map_err(|source| ClientError::Decode { path, source })
    }
}

fn read_token(path: &Path) -> Result<String, ClientError> {
    let metadata = fs::symlink_metadata(path).map_err(|source| ClientError::TokenRead {
        path: path.to_path_buf(),
        source,
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(ClientError::InvalidTokenFile(path.to_path_buf()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(ClientError::InsecureTokenPermissions(path.to_path_buf()));
        }
    }
    let token = fs::read_to_string(path).map_err(|source| ClientError::TokenRead {
        path: path.to_path_buf(),
        source,
    })?;
    let token = token.trim().to_owned();
    if token.len() < 64 || !token.is_ascii() || token.chars().any(char::is_whitespace) {
        return Err(ClientError::InvalidToken(path.to_path_buf()));
    }
    Ok(token)
}

fn bounded_diagnostic(bytes: &[u8]) -> String {
    const LIMIT: usize = 4_096;
    String::from_utf8_lossy(&bytes[..bytes.len().min(LIMIT)]).into_owned()
}

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("daemon endpoint is not loopback: {0}")]
    NonLoopback(std::net::SocketAddr),
    #[error("failed to build daemon HTTP client: {0}")]
    Build(reqwest::Error),
    #[error("failed to read daemon token {path}; run `hzr daemon serve`")]
    TokenRead {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("daemon token path is not a regular file: {0}")]
    InvalidTokenFile(PathBuf),
    #[error("daemon token has insecure permissions: {0}")]
    InsecureTokenPermissions(PathBuf),
    #[error("daemon token is invalid: {0}")]
    InvalidToken(PathBuf),
    #[error("daemon request failed: {0}")]
    Transport(reqwest::Error),
    #[error("daemon returned HTTP {status}: {code}: {message} (recoverable={recoverable})")]
    Api {
        status: StatusCode,
        code: String,
        message: String,
        recoverable: bool,
    },
    #[error("daemon returned HTTP {status}: {body}")]
    Http { status: StatusCode, body: String },
    #[error("daemon returned invalid JSON for {path}: {source}")]
    Decode {
        path: &'static str,
        source: serde_json::Error,
    },
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;
    use std::fs;

    use hzr_core::Config;
    use tempfile::tempdir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::{DaemonClient, read_token};

    #[test]
    fn test_read_token_accepts_private_daemon_secret() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("hzrd.token");
        fs::write(&path, "a".repeat(64)).expect("write token");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
                .expect("set private permissions");
        }

        assert_eq!(read_token(&path).expect("valid token"), "a".repeat(64));
    }

    #[test]
    fn test_read_token_alternate_error_chain_does_not_duplicate_os_error() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("missing-hzrd.token");
        let error = read_token(&path).expect_err("missing token must fail");
        let os_error = error.source().map(ToString::to_string).unwrap_or_default();
        assert!(
            !os_error.is_empty(),
            "token read error must retain its source"
        );
        let rendered = format!("{:#}", anyhow::Error::new(error));

        assert!(rendered.starts_with(&format!(
            "failed to read daemon token {}; run `hzr daemon serve`",
            path.display()
        )));
        assert_eq!(rendered.matches(&os_error).count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn test_read_token_rejects_group_readable_secret() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("hzrd.token");
        fs::write(&path, "a".repeat(64)).expect("write token");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640))
            .expect("set insecure permissions");

        assert!(read_token(&path).is_err());
    }

    #[tokio::test]
    async fn test_health_uses_bearer_auth_and_typed_response() {
        let directory = tempdir().expect("temporary directory");
        let runtime = directory.path().join("runtime");
        fs::create_dir_all(&runtime).expect("create runtime directory");
        let token_path = runtime.join("hzrd.token");
        let token = "z".repeat(64);
        fs::write(&token_path, &token).expect("write token");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            fs::set_permissions(&token_path, fs::Permissions::from_mode(0o600))
                .expect("set private permissions");
        }
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test server");
        let address = listener.local_addr().expect("test server address");
        let expected_auth = format!("authorization: Bearer {token}").to_ascii_lowercase();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept request");
            let mut request = vec![0_u8; 8 * 1024];
            let read = stream.read(&mut request).await.expect("read request");
            let rendered = String::from_utf8_lossy(&request[..read]).to_ascii_lowercase();
            assert!(rendered.contains("get /v1/health http/1.1"));
            assert!(rendered.contains(&expected_auth));
            let body = br#"{"protocol_version":1,"hzr_version":"0.4.1","state":"ready","workspace_root":null,"engines":[],"capabilities":[]}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("write response headers");
            stream.write_all(body).await.expect("write response body");
        });
        let mut config = Config {
            data_dir: directory.path().to_path_buf(),
            ..Config::default()
        };
        config.daemon.bind = address;

        let health = DaemonClient::from_config(&config)
            .expect("client")
            .health()
            .await
            .expect("typed health response");
        server.await.expect("test server completion");

        assert_eq!(health.hzr_version, "0.4.1");
        assert_eq!(health.protocol_version, 1);
    }
}
