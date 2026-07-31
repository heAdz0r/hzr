use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use hzr_exec::CanonicalCommand;
use tokio::sync::Mutex;
use uuid::Uuid;

const APPROVAL_TTL: Duration = Duration::from_secs(5 * 60);
const MAX_PENDING_APPROVALS: usize = 128;

#[derive(Clone, Debug)]
pub struct PendingApproval {
    pub requested: CanonicalCommand,
    pub proposed: CanonicalCommand,
    pub cwd: PathBuf,
    pub timeout_ms: Option<u64>,
}

struct StoredApproval {
    created_at: Instant,
    pending: PendingApproval,
}

#[derive(Clone, Default)]
pub struct ApprovalStore {
    pending: Arc<Mutex<HashMap<String, StoredApproval>>>,
}

impl ApprovalStore {
    pub async fn insert(&self, pending: PendingApproval) -> String {
        let mut approvals = self.pending.lock().await;
        prune_expired(&mut approvals);
        if approvals.len() >= MAX_PENDING_APPROVALS
            && let Some(oldest) = approvals
                .iter()
                .min_by_key(|(_, approval)| approval.created_at)
                .map(|(id, _)| id.clone())
        {
            approvals.remove(&oldest);
        }
        let decision_id = Uuid::now_v7().to_string();
        approvals.insert(
            decision_id.clone(),
            StoredApproval {
                created_at: Instant::now(),
                pending,
            },
        );
        decision_id
    }

    pub async fn take(&self, decision_id: &str) -> Option<PendingApproval> {
        let mut approvals = self.pending.lock().await;
        prune_expired(&mut approvals);
        approvals.remove(decision_id).map(|stored| stored.pending)
    }
}

fn prune_expired(approvals: &mut HashMap<String, StoredApproval>) {
    approvals.retain(|_, approval| approval.created_at.elapsed() < APPROVAL_TTL);
}

#[cfg(test)]
mod tests {
    use hzr_exec::CanonicalCommand;

    use super::{ApprovalStore, PendingApproval};

    #[tokio::test]
    async fn test_pending_approval_is_single_use() {
        let store = ApprovalStore::default();
        let pending = PendingApproval {
            requested: CanonicalCommand::shell("cargo test"),
            proposed: CanonicalCommand::shell("rtk cargo test"),
            cwd: std::env::current_dir().expect("current directory"),
            timeout_ms: Some(1_000),
        };
        let decision_id = store.insert(pending).await;

        assert!(store.take(&decision_id).await.is_some());
        assert!(store.take(&decision_id).await.is_none());
    }
}
