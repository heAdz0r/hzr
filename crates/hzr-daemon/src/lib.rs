mod accounting_sweeper;
// 0.8.7: optional agtx Agent Observatory component lifecycle.
mod agent_observer;
mod api;
mod approval;
mod auth;
mod error;
mod exec_jobs;
mod ledger_writer;
mod lock;
mod observability;
mod read_cost;
mod search_pages;
mod server;
mod shutdown;
mod state;
mod visualizer;

pub use agent_observer::{ComponentInfo, probe_agent_component};
pub use auth::{AuthToken, load_or_create_token};
pub use error::DaemonError;
pub use ledger_writer::{FidelityDurabilityStatus, inspect_fidelity_pending};
pub use lock::DaemonLockError;
pub use server::{router, serve};
pub use shutdown::shutdown_signal;
pub use state::AppState;
