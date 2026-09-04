use hzr_protocol::ErrorResponse;
use serde::{Deserialize, Serialize};

use crate::ExecutionOutcome;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExecJobDelivery {
    pub unchanged: bool,
    pub output_omitted: bool,
    pub required_bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecJobState {
    Running,
    Completed,
    Cancelled,
    Interrupted,
    Failed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExecJobSnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<ExecJobDelivery>,
    pub operation_id: String,
    pub state: ExecJobState,
    pub revision: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<ExecutionOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorResponse>,
}
