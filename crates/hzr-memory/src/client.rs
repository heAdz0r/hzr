use std::fmt;
use std::process::{Output, Stdio};
use std::sync::Arc;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use tokio::process::Command;
use tokio::sync::Mutex;

use crate::circuit::{CircuitBreaker, CircuitSnapshot};
use crate::config::IcmConfig;
use crate::error::{MemoryError, Result};
use crate::http_transport::{self, AttemptError};
use crate::installation::{bounded_text, verify_installation};
use crate::layout::IcmLayout;
use crate::mcp::{self, SharedMcp};
use crate::types::{
    IcmTransport, MemoryRecord, MemoryStats, MemoryTransport, RecallRequest, ServiceHealth,
    StoreReceipt, StoreRequest,
};

#[derive(Clone)]
pub struct IcmClient {
    config: IcmConfig,
    layout: IcmLayout,
    token: Arc<str>,
    base_url: Arc<str>,
    http: reqwest::Client,
    circuit: CircuitBreaker,
    cli_verified: Arc<Mutex<bool>>,
    mcp: SharedMcp,
}

impl fmt::Debug for IcmClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("IcmClient")
            .field("endpoint", &self.base_url)
            .field("database", &self.layout.database)
            .finish_non_exhaustive()
    }
}

impl IcmClient {
    pub fn from_config(config: IcmConfig) -> Result<Self> {
        config.validate()?;
        let layout = IcmLayout::prepare(config.data_root())?;
        let token = layout.load_or_create_token()?;
        Self::new(config, layout, token, mcp::shared())
    }

    pub(crate) fn new(
        config: IcmConfig,
        layout: IcmLayout,
        token: String,
        mcp: SharedMcp,
    ) -> Result<Self> {
        let base_url = format!("http://{}", config.bind_addr);
        let http = reqwest::Client::builder()
            .connect_timeout(config.request_timeout)
            .timeout(config.request_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|source| MemoryError::HttpRequest {
                operation: "client initialization",
                source,
            })?;
        Ok(Self {
            circuit: CircuitBreaker::new(
                config.circuit_failure_threshold,
                config.circuit_reset_timeout,
            ),
            config,
            layout,
            token: Arc::from(token),
            base_url: Arc::from(base_url),
            http,
            cli_verified: Arc::new(Mutex::new(false)),
            mcp,
        })
    }

    pub fn endpoint(&self) -> &str {
        &self.base_url
    }

    pub fn database_path(&self) -> &std::path::Path {
        &self.layout.database
    }

    pub async fn circuit_snapshot(&self) -> CircuitSnapshot {
        self.circuit.snapshot().await
    }

    pub async fn health(&self) -> Result<ServiceHealth> {
        match self.config.transport {
            IcmTransport::StdioMcp => {
                mcp::ping(
                    &self.mcp,
                    self.config.request_timeout,
                    self.config.embeddings,
                )
                .await
            }
            IcmTransport::Http => self
                .get_json("/health")
                .await
                .map_err(|error| error.into_public("health", self.config.request_timeout)),
        }
    }

    pub async fn readiness(&self) -> Result<ServiceHealth> {
        let health = self.health().await?;
        if health.status != "ok" {
            return Err(MemoryError::UnexpectedResponse {
                operation: "health",
                message: format!("status was {:?}", health.status),
            });
        }
        if self.config.transport == IcmTransport::Http {
            let stats = self.stats().await?;
            if !stats.avg_weight.is_finite() || stats.total_topics > stats.total_memories {
                return Err(MemoryError::UnexpectedResponse {
                    operation: "readiness",
                    message: "stats invariants were not satisfied".into(),
                });
            }
        }
        Ok(health)
    }

    pub async fn stats(&self) -> Result<MemoryStats> {
        if self.config.transport != IcmTransport::Http {
            return Err(MemoryError::UnsupportedTransport {
                operation: "stats",
                transport: "stdio_mcp",
            });
        }
        self.get_json("/stats?format=json")
            .await
            .map_err(|error| error.into_public("stats", self.config.request_timeout))
    }

    pub async fn recall(&self, request: &RecallRequest) -> Result<Vec<MemoryRecord>> {
        validate_recall(request)?;
        if self.config.transport == IcmTransport::StdioMcp {
            return self.recall_cli(request).await;
        }
        if let Err(error) = self.circuit.before_request().await {
            return if self.config.cli_fallback {
                self.recall_cli(request).await
            } else {
                Err(error)
            };
        }

        match self
            .post_json::<RecallPayload, _>("/recall?format=json", request)
            .await
        {
            Ok(payload) => {
                self.circuit.record_success().await;
                Ok(payload.into_records())
            }
            Err(error) => {
                let fallback = error.safe_recall_fallback();
                if error.affects_availability() {
                    self.circuit.record_failure().await;
                }
                if fallback && self.config.cli_fallback {
                    self.recall_cli(request).await
                } else {
                    Err(error.into_public("recall", self.config.request_timeout))
                }
            }
        }
    }

    pub async fn store(&self, request: &StoreRequest) -> Result<StoreReceipt> {
        validate_store(request)?;
        if self.config.transport == IcmTransport::StdioMcp {
            return self.store_mcp(request).await;
        }
        if let Err(error) = self.circuit.before_request().await {
            return if self.config.cli_fallback {
                self.store_cli(request).await
            } else {
                Err(error)
            };
        }

        match self
            .post_json::<Vec<MemoryRecord>, _>("/store?format=json", request)
            .await
        {
            Ok(mut records) => {
                self.circuit.record_success().await;
                if records.len() != 1 {
                    return Err(MemoryError::UnexpectedResponse {
                        operation: "store",
                        message: format!("expected one memory, received {}", records.len()),
                    });
                }
                Ok(StoreReceipt {
                    transport: MemoryTransport::Http,
                    memory: records.pop(),
                })
            }
            Err(error) => {
                let fallback = error.safe_store_fallback();
                if error.affects_availability() {
                    self.circuit.record_failure().await;
                }
                if fallback && self.config.cli_fallback {
                    self.store_cli(request).await
                } else {
                    Err(error.into_public("store", self.config.request_timeout))
                }
            }
        }
    }

    pub(crate) async fn disconnect_mcp(&self) {
        mcp::disconnect(&self.mcp).await;
    }

    pub(crate) fn mcp(&self) -> &SharedMcp {
        &self.mcp
    }

    async fn store_mcp(&self, request: &StoreRequest) -> Result<StoreReceipt> {
        if let Err(error) = self.circuit.before_request().await {
            return if self.config.cli_fallback {
                self.store_cli(request).await
            } else {
                Err(error)
            };
        }
        match mcp::store(&self.mcp, request, self.config.request_timeout).await {
            Ok(()) => {
                self.circuit.record_success().await;
                Ok(StoreReceipt {
                    transport: MemoryTransport::StdioMcp,
                    memory: None,
                })
            }
            Err(error) => {
                let unavailable = matches!(&error, MemoryError::McpUnavailable);
                if mcp_availability_failure(&error) {
                    self.circuit.record_failure().await;
                }
                if unavailable && self.config.cli_fallback {
                    self.store_cli(request).await
                } else {
                    Err(error)
                }
            }
        }
    }

    async fn ensure_cli_verified(&self) -> Result<()> {
        let mut verified = self.cli_verified.lock().await;
        if !*verified {
            verify_installation(&self.config).await?;
            *verified = true;
        }
        Ok(())
    }

    async fn recall_cli(&self, request: &RecallRequest) -> Result<Vec<MemoryRecord>> {
        self.ensure_cli_verified().await?;
        let mut command = self.cli_command();
        command
            .arg("recall")
            .arg(&request.query)
            .arg("--limit")
            .arg(request.limit.to_string())
            .arg("--format")
            .arg("json");
        if let Some(topic) = &request.topic {
            command.arg("--topic").arg(topic);
        }
        if let Some(keyword) = &request.keyword {
            command.arg("--keyword").arg(keyword);
        }
        if let Some(project) = &request.project {
            command.arg("--project").arg(project);
        }
        let output = self.run_cli("recall", command).await?;
        serde_json::from_slice(&output.stdout).map_err(|source| MemoryError::Protocol {
            operation: "recall CLI",
            source,
        })
    }

    async fn store_cli(&self, request: &StoreRequest) -> Result<StoreReceipt> {
        if request.keywords.iter().any(|keyword| keyword.contains(',')) {
            return Err(MemoryError::InvalidRequest(
                "ICM CLI fallback cannot preserve a keyword containing a comma".into(),
            ));
        }
        self.ensure_cli_verified().await?;
        let mut command = self.cli_command();
        command
            .arg("store")
            .arg("--topic")
            .arg(&request.topic)
            .arg("--content")
            .arg(&request.content)
            .arg("--importance")
            .arg(request.importance.to_string());
        if !request.keywords.is_empty() {
            command.arg("--keywords").arg(request.keywords.join(","));
        }
        if let Some(raw) = &request.raw {
            command.arg("--raw").arg(raw);
        }
        self.run_cli("store", command).await?;
        Ok(StoreReceipt {
            transport: MemoryTransport::Cli,
            memory: None,
        })
    }

    fn cli_command(&self) -> Command {
        let mut command = Command::new(&self.config.executable);
        command
            .arg("--db")
            .arg(&self.layout.database)
            .stdin(Stdio::null())
            .kill_on_drop(true);
        if !self.config.embeddings {
            command.arg("--no-embeddings");
        }
        command
    }

    async fn run_cli(&self, operation: &'static str, mut command: Command) -> Result<Output> {
        let output = tokio::time::timeout(self.config.cli_timeout, command.output())
            .await
            .map_err(|_| MemoryError::CliTimeout {
                operation,
                timeout: self.config.cli_timeout,
            })?
            .map_err(|source| MemoryError::BinaryUnavailable {
                executable: self.config.executable.clone(),
                source,
            })?;
        if !output.status.success() {
            return Err(MemoryError::CliFailed {
                operation,
                status: output.status,
                stderr: bounded_text(&output.stderr, 8 * 1024),
            });
        }
        Ok(output)
    }

    async fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
    ) -> std::result::Result<T, AttemptError> {
        http_transport::get_json(&self.http, &self.token, &self.base_url, path).await
    }

    async fn post_json<T: DeserializeOwned, B: serde::Serialize + ?Sized>(
        &self,
        path: &str,
        body: &B,
    ) -> std::result::Result<T, AttemptError> {
        http_transport::post_json(&self.http, &self.token, &self.base_url, path, body).await
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RecallPayload {
    Records(Vec<MemoryRecord>),
    Empty { results: Vec<MemoryRecord> },
}

impl RecallPayload {
    fn into_records(self) -> Vec<MemoryRecord> {
        match self {
            Self::Records(records) | Self::Empty { results: records } => records,
        }
    }
}

fn validate_recall(request: &RecallRequest) -> Result<()> {
    if request.query.trim().is_empty() {
        return Err(MemoryError::InvalidRequest(
            "recall query must not be empty".into(),
        ));
    }
    if !(1..=100).contains(&request.limit) {
        return Err(MemoryError::InvalidRequest(
            "recall limit must be between 1 and 100".into(),
        ));
    }
    Ok(())
}

fn validate_store(request: &StoreRequest) -> Result<()> {
    if request.topic.trim().is_empty() {
        return Err(MemoryError::InvalidRequest(
            "store topic must not be empty".into(),
        ));
    }
    if request.content.trim().is_empty() {
        return Err(MemoryError::InvalidRequest(
            "store content must not be empty".into(),
        ));
    }
    Ok(())
}

fn mcp_availability_failure(error: &MemoryError) -> bool {
    matches!(
        error,
        MemoryError::McpUnavailable
            | MemoryError::McpTimeout { .. }
            | MemoryError::McpIo { .. }
            | MemoryError::McpProtocol { .. }
    )
}
