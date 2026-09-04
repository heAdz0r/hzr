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
mod runtime;
mod snapshot;
mod supervisor;
mod types;

pub use circuit::{CircuitSnapshot, CircuitStatus};
pub use client::IcmClient;
pub use config::IcmConfig;
pub use error::{MemoryError, Result};
pub use installation::verify_installation;
pub use layout::IcmLayout;
pub use namespace::{
    GLOBAL_SCOPE_TOKEN, MAX_MEMORY_KIND_BYTES, MemoryNamespace, PROJECT_TOKEN_BYTES, global_topic,
    isolate_memories, isolate_project_memories, merge_memories, namespaced_topic,
    recall_candidate_limit, topic_belongs_to_project, topic_is_global, validate_memory_kind,
};
pub use release::{ICM_COMMIT, ICM_MCP_SERVER_VERSION, ICM_TAG, ICM_VERSION, IcmInstallation};
pub use runtime::is_managed_icm_process;
pub use snapshot::{
    MemoryContent, MemoryTopicEdge, MemoryTopicSnapshot, ProjectMemoryDetail,
    ProjectMemorySnapshot, ProjectTopicDetails, read_memory_by_id, read_project_snapshot,
    read_project_topic_details,
};
pub use supervisor::{IcmSupervisor, ServiceStatus, StartOutcome, StopOutcome};
pub use types::{
    IcmTransport, Importance, MemoryRecallResponse, MemoryRecord, MemoryScope, MemorySource,
    MemoryStats, MemoryTransport, RecallRequest, ServiceHealth, StoreReceipt, StoreRequest,
};
