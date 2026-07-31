use std::sync::Arc;

use hzr_context::ContextPlanner;
use hzr_core::Config;
use hzr_exec::{ExecutionPipeline, ForkRuntimePaths, PinnedRtkAdapter, RtkAdapterConfig};
use hzr_memory::{IcmConfig, IcmSupervisor, StartOutcome};
use tokio::sync::RwLock;

use crate::DaemonError;
use crate::approval::ApprovalStore;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub approvals: ApprovalStore,
    pub context: Arc<ContextPlanner>,
    pub memory: Arc<IcmSupervisor>,
    pub memory_start: Arc<RwLock<MemoryStartState>>,
    pub rtk: Arc<PinnedRtkAdapter>,
    pub executor: ExecutionPipeline,
}

#[derive(Clone, Debug)]
pub enum MemoryStartState {
    Disabled,
    Starting,
    Ready(StartOutcome),
    Degraded(String),
}

impl AppState {
    pub async fn initialize(config: Config) -> Result<Self, DaemonError> {
        config
            .validate()
            .map_err(|error| DaemonError::Config(error.to_string()))?;
        config
            .ensure_layout()
            .map_err(|error| DaemonError::Config(error.to_string()))?;

        let mut icm_config =
            IcmConfig::from_data_root(config.engines.binary("icm"), config.data_dir.clone());
        icm_config.embeddings = config.engines.icm_embeddings;
        let memory = Arc::new(IcmSupervisor::new(icm_config).map_err(DaemonError::Memory)?);
        let memory_start = if config.engines.auto_start_icm {
            MemoryStartState::Starting
        } else {
            MemoryStartState::Disabled
        };
        let memory_start = Arc::new(RwLock::new(memory_start));
        let rtk = PinnedRtkAdapter::detect(RtkAdapterConfig {
            binary: config.engines.binary("rtk"),
            runtime_paths: Some(ForkRuntimePaths::from_data_root(&config.data_dir)),
            ..RtkAdapterConfig::default()
        })
        .await;
        let context = Arc::new(ContextPlanner::from_config(
            &config,
            memory.client(),
            rtk.runner(),
        ));
        if config.engines.auto_start_icm {
            let background_memory = Arc::clone(&memory);
            let background_state = Arc::clone(&memory_start);
            tokio::spawn(async move {
                let next = match background_memory.start().await {
                    Ok(outcome) => MemoryStartState::Ready(outcome),
                    Err(error) => MemoryStartState::Degraded(error.to_string()),
                };
                *background_state.write().await = next;
            });
        }
        Ok(Self {
            config: Arc::new(config),
            approvals: ApprovalStore::default(),
            context,
            memory,
            memory_start,
            rtk: Arc::new(rtk),
            executor: ExecutionPipeline,
        })
    }
}
