use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use hzr_core::PrivacyPseudonymizer;
use hzr_protocol::{
    DashboardLifecycleEvent, DashboardLifecycleKind, DashboardObservability, DashboardTraceSpan,
    DashboardTraceStage, DashboardTraceState, TraceId,
};

const EVENT_CAPACITY: usize = 512;
pub const DEFAULT_OBSERVABILITY_LIMIT: usize = 64;
pub const MAX_OBSERVABILITY_LIMIT: usize = 100;

#[derive(Clone)]
pub struct ObservabilityStore {
    inner: Arc<Inner>,
}

struct Inner {
    sequence: AtomicU64,
    span_sequence: AtomicU64,
    privacy: PrivacyPseudonymizer,
    events: Mutex<VecDeque<Event>>,
}

#[derive(Clone)]
enum Event {
    Trace(DashboardTraceSpan),
    Lifecycle(DashboardLifecycleEvent),
}

#[derive(Clone, Debug)]
pub struct TraceContext {
    pub hash: String,
    pub linked_trace_hash: Option<String>,
    pub project_hash: Option<String>,
    pub session_hash: Option<String>,
    root_span_id: u64,
}

#[derive(Clone, Debug)]
pub struct TraceSpanInput<'a> {
    pub stage: DashboardTraceStage,
    pub state: DashboardTraceState,
    pub engine: &'a str,
    pub duration_ms: u64,
    pub route: Option<&'a str>,
    pub error_code: Option<&'a str>,
    pub generation: Option<&'a str>,
}

impl ObservabilityStore {
    pub fn new(privacy: PrivacyPseudonymizer) -> Self {
        Self {
            inner: Arc::new(Inner {
                sequence: AtomicU64::new(0),
                span_sequence: AtomicU64::new(0),
                privacy,
                events: Mutex::new(VecDeque::with_capacity(EVENT_CAPACITY)),
            }),
        }
    }
    pub fn project_hash(&self, project: &str) -> String {
        self.pseudonym("project", project)
    }

    pub fn repository_hash(&self, repository: &str) -> String {
        self.pseudonym("repository", repository)
    }

    pub fn command_hash(&self, command_hash: &str) -> String {
        self.pseudonym("command", command_hash)
    }

    pub fn topic_hash(&self, topic: &str) -> String {
        self.pseudonym("topic", topic)
    }

    pub fn begin_trace(&self, project: &str, session: Option<&str>) -> TraceContext {
        let raw = TraceId::new().to_string();
        let root_span_id = self.next_span_id();
        TraceContext {
            hash: self.pseudonym("dashboard-trace", &raw),
            linked_trace_hash: None,
            project_hash: (!project.is_empty()).then(|| self.project_hash(project)),
            session_hash: session.map(|value| self.pseudonym("session", value)),
            root_span_id,
        }
    }

    pub fn begin_continuation(
        &self,
        project: &str,
        session: Option<&str>,
        linked_trace_hash: String,
    ) -> TraceContext {
        let mut trace = self.begin_trace(project, session);
        trace.linked_trace_hash = Some(linked_trace_hash);
        trace
    }

    pub fn record_span(&self, trace: &TraceContext, input: TraceSpanInput<'_>) {
        let sequence = self.next_sequence();
        let parent_span_id =
            (input.stage != DashboardTraceStage::Request).then_some(trace.root_span_id);
        let span_id = if input.stage == DashboardTraceStage::Request {
            trace.root_span_id
        } else {
            self.next_span_id()
        };
        self.push(Event::Trace(DashboardTraceSpan {
            sequence,
            trace_hash: trace.hash.clone(),
            linked_trace_hash: trace.linked_trace_hash.clone(),
            span_id,
            parent_span_id,
            stage: input.stage,
            state: input.state,
            engine: input.engine.to_owned(),
            observed_at_ms: now_ms().saturating_sub(input.duration_ms),
            duration_ms: input.duration_ms,
            project_hash: trace.project_hash.clone(),
            session_hash: trace.session_hash.clone(),
            route: input.route.map(str::to_owned),
            error_code: input.error_code.map(str::to_owned),
            producer_version: concat!("hzr-daemon/", env!("CARGO_PKG_VERSION")).into(),
            policy_version: hzr_core::CURRENT_ACCOUNTING_POLICY_VERSION.into(),
            generation: input.generation.map(str::to_owned),
        }));
    }

    pub fn record_lifecycle(
        &self,
        engine: &str,
        kind: DashboardLifecycleKind,
        project: Option<&str>,
        detail_code: &str,
        generation: Option<&str>,
    ) {
        let sequence = self.next_sequence();
        self.push(Event::Lifecycle(DashboardLifecycleEvent {
            sequence,
            observed_at_ms: now_ms(),
            engine: engine.to_owned(),
            kind,
            project_hash: project.map(|value| self.pseudonym("project", value)),
            detail_code: detail_code.to_owned(),
            producer_version: concat!("hzr-daemon/", env!("CARGO_PKG_VERSION")).into(),
            generation: generation.map(str::to_owned),
        }));
    }

    pub fn snapshot(
        &self,
        project_hash: Option<&str>,
        after: Option<u64>,
        limit: usize,
    ) -> DashboardObservability {
        let events = self
            .inner
            .events
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let matches = events
            .iter()
            .filter(|event| event.sequence() > after.unwrap_or_default())
            .filter(|event| event.matches_project(project_hash))
            .cloned()
            .collect::<Vec<_>>();
        let truncated = matches.len() > limit;
        let matches = matches.into_iter().take(limit).collect::<Vec<_>>();
        let next_cursor = matches.last().map(Event::sequence);
        let mut snapshot = DashboardObservability {
            next_cursor,
            truncated,
            ..DashboardObservability::default()
        };
        for event in matches {
            match event {
                Event::Trace(span) => snapshot.trace_spans.push(span),
                Event::Lifecycle(event) => snapshot.lifecycle_events.push(event),
            }
        }
        snapshot
    }

    pub fn latest_snapshot(
        &self,
        project_hash: Option<&str>,
        limit: usize,
    ) -> DashboardObservability {
        let events = self
            .inner
            .events
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut matches = events
            .iter()
            .filter(|event| event.matches_project(project_hash))
            .rev()
            .take(limit)
            .cloned()
            .collect::<Vec<_>>();
        matches.reverse();
        let next_cursor = matches.last().map(Event::sequence);
        let truncated = events
            .iter()
            .filter(|event| event.matches_project(project_hash))
            .count()
            > matches.len();
        let mut snapshot = DashboardObservability {
            next_cursor,
            truncated,
            ..DashboardObservability::default()
        };
        for event in matches {
            match event {
                Event::Trace(span) => snapshot.trace_spans.push(span),
                Event::Lifecycle(event) => snapshot.lifecycle_events.push(event),
            }
        }
        snapshot
    }

    fn next_sequence(&self) -> u64 {
        self.inner.sequence.fetch_add(1, Ordering::AcqRel) + 1
    }

    fn next_span_id(&self) -> u64 {
        self.inner.span_sequence.fetch_add(1, Ordering::AcqRel) + 1
    }

    fn pseudonym(&self, domain: &str, value: &str) -> String {
        self.inner.privacy.hash(domain, value)
    }

    fn push(&self, event: Event) {
        let mut events = self
            .inner
            .events
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if events.len() == EVENT_CAPACITY {
            events.pop_front();
        }
        events.push_back(event);
    }
}

impl Event {
    fn sequence(&self) -> u64 {
        match self {
            Self::Trace(span) => span.sequence,
            Self::Lifecycle(event) => event.sequence,
        }
    }

    fn matches_project(&self, project_hash: Option<&str>) -> bool {
        let Some(project_hash) = project_hash else {
            return true;
        };
        match self {
            // Pure global operations such as codec transforms are visible in every
            // selected project without being falsely attributed to that project.
            Self::Trace(span) => span
                .project_hash
                .as_deref()
                .is_none_or(|value| value == project_hash),
            // Daemon-global lifecycle transitions intentionally apply to every selected
            // project; project-owned transitions remain isolated to their keyed pseudonym.
            Self::Lifecycle(event) => event
                .project_hash
                .as_deref()
                .is_none_or(|value| value == project_hash),
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|duration| u64::try_from(duration.as_millis()).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use hzr_core::{Ledger, PrivacyPseudonymizer};
    use hzr_protocol::{DashboardLifecycleKind, DashboardTraceStage, DashboardTraceState};

    use super::{ObservabilityStore, TraceSpanInput};

    #[test]
    fn snapshot_is_bounded_cursor_based_and_project_scoped() {
        let store = ObservabilityStore::new(
            PrivacyPseudonymizer::from_key("33".repeat(32)).expect("valid test privacy key"),
        );
        let alpha = store.begin_trace("/alpha", Some("secret-session"));
        let beta = store.begin_trace("/beta", None);
        store.record_span(
            &alpha,
            TraceSpanInput {
                stage: DashboardTraceStage::Request,
                state: DashboardTraceState::Completed,
                engine: "hzrd",
                duration_ms: 1,
                route: None,
                error_code: None,
                generation: None,
            },
        );
        store.record_span(
            &beta,
            TraceSpanInput {
                stage: DashboardTraceStage::Engine,
                state: DashboardTraceState::Failed,
                engine: "grepai",
                duration_ms: 2,
                route: Some("semantic"),
                error_code: Some("index_unavailable"),
                generation: Some("g1"),
            },
        );
        let global = store.begin_trace("", None);
        store.record_span(
            &global,
            TraceSpanInput {
                stage: DashboardTraceStage::Engine,
                state: DashboardTraceState::Completed,
                engine: "codec",
                duration_ms: 1,
                route: Some("codec_compile"),
                error_code: None,
                generation: None,
            },
        );
        store.record_lifecycle(
            "icm",
            DashboardLifecycleKind::RestartScheduled,
            Some("/alpha"),
            "health_failed",
            None,
        );

        let alpha_hash = alpha.project_hash.as_deref().expect("project hash");
        let alpha_snapshot = store.snapshot(Some(alpha_hash), None, 10);
        assert_eq!(alpha_snapshot.trace_spans.len(), 2);
        assert_eq!(alpha_snapshot.lifecycle_events.len(), 1);
        let beta_hash = beta.project_hash.as_deref().expect("project hash");
        let beta_snapshot = store.snapshot(Some(beta_hash), None, 10);
        assert_eq!(beta_snapshot.trace_spans.len(), 2);
        assert!(
            alpha_snapshot
                .trace_spans
                .iter()
                .any(|span| { span.engine == "codec" && span.project_hash.is_none() })
        );
        assert!(
            beta_snapshot
                .trace_spans
                .iter()
                .any(|span| { span.engine == "codec" && span.project_hash.is_none() })
        );
        let encoded = serde_json::to_string(&alpha_snapshot).expect("JSON");
        assert!(!encoded.contains("/alpha"));
        assert!(!encoded.contains("secret-session"));

        let cursor = alpha_snapshot.next_cursor.expect("cursor");
        assert!(
            store
                .snapshot(Some(alpha_hash), Some(cursor), 10)
                .trace_spans
                .is_empty()
        );
        let bounded = store.snapshot(None, None, 1);
        assert!(bounded.truncated);
        assert_eq!(
            bounded.trace_spans.len() + bounded.lifecycle_events.len(),
            1
        );
    }

    #[test]
    fn persisted_ledger_key_keeps_public_identities_stable_across_restart() {
        let directory = tempfile::tempdir().expect("temporary ledger directory");
        let path = directory.path().join("ledger.sqlite");
        let first = Ledger::open(&path).expect("first ledger");
        let first_privacy = first.privacy_pseudonymizer().expect("first privacy key");
        let first_project = first_privacy.hash("project", "/private/workspace");
        let first_session = first_privacy.hash("session", "/private/workspace");
        drop(first);

        let second = Ledger::open(&path).expect("reopened ledger");
        let second_privacy = second
            .privacy_pseudonymizer()
            .expect("persisted privacy key");
        assert_eq!(
            first_project,
            second_privacy.hash("project", "/private/workspace")
        );
        assert_ne!(first_project, first_session, "domains must not join");
        assert_ne!(
            first_project,
            second_privacy.hash("project", "/private/other-workspace")
        );
    }
}
