#![forbid(unsafe_code)]

mod circuit;
mod client;
mod config;
mod error;
mod http_transport;
mod installation;
mod layout;
mod mcp;
mod namespace;
mod release;
mod supervisor;
mod types;

pub use circuit::{CircuitSnapshot, CircuitStatus};
pub use client::IcmClient;
pub use config::IcmConfig;
pub use error::{MemoryError, Result};
pub use installation::verify_installation;
pub use layout::IcmLayout;
pub use namespace::{
    MAX_MEMORY_KIND_BYTES, PROJECT_TOKEN_BYTES, isolate_project_memories, namespaced_topic,
    recall_candidate_limit, topic_belongs_to_project, validate_memory_kind,
};
pub use release::{ICM_COMMIT, ICM_MCP_SERVER_VERSION, ICM_TAG, ICM_VERSION, IcmInstallation};
pub use supervisor::{IcmSupervisor, ServiceStatus, StartOutcome, StopOutcome};
pub use types::{
    IcmTransport, Importance, MemoryRecord, MemoryScope, MemorySource, MemoryStats,
    MemoryTransport, RecallRequest, ServiceHealth, StoreReceipt, StoreRequest,
};
