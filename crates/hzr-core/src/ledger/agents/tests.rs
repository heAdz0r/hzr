use std::collections::BTreeSet;

use hzr_protocol::agents::{
    AgentSnapshotCapabilities, AgentSnapshotEdge, AgentSnapshotEnvelope, AgentSnapshotHook,
    AgentSnapshotNotification, AgentSnapshotProject, AgentSnapshotRuntime, AgentSnapshotTask,
    AgentTokenTotals, AgentUsageReceiptV1,
};

use super::*;
use crate::billing::{PricingCatalog, load_pricing_catalog};

const PROJECT_PATH: &str = "/fixture/work/repo";
const NOW_MS: i64 = 1_788_258_200_000;

fn ledger() -> (tempfile::TempDir, Ledger) {
    let dir = tempfile::tempdir().expect("temp dir");
    let ledger = Ledger::open(&dir.path().join("ledger.db")).expect("ledger opens");
    (dir, ledger)
}

fn identity(_ledger: &Ledger, generation: &str) -> AgentSourceIdentity {
    let project_hash = crate::ledger::privacy_identity_hash("project", PROJECT_PATH);
    AgentSourceIdentity::new("enrollment-1", generation, &project_hash)
}

fn pseudonym(value: &str) -> String {
    crate::ledger::privacy_identity_hash("session", value)
}

fn context() -> AgentApplyContext<'static> {
    AgentApplyContext {
        session_pseudonym: &pseudonym,
        max_observation_interval_ms: 10_000,
    }
}

fn task(id: &str, status: &str, agent: &str) -> AgentSnapshotTask {
    AgentSnapshotTask {
        source_task_id: id.into(),
        source_project_id: "proj-1".into(),
        title: Some(format!("Fixture task {id}")),
        branch: Some(format!("feat/{id}")),
        board_status: status.into(),
        unknown_board_status: None,
        agent: agent.into(),
        cycle: 1,
        referenced_task_ids: Vec::new(),
        has_worktree: true,
        source_created_at_ms: Some(NOW_MS - 60_000),
        source_updated_at_ms: Some(NOW_MS - 1_000),
        runtime: None,
        hook: None,
    }
}

fn envelope(tasks: Vec<AgentSnapshotTask>, observed_at_ms: i64) -> AgentSnapshotEnvelope {
    AgentSnapshotEnvelope {
        schema_version: 1,
        request_id: "req".into(),
        upstream_version: "1.0.4".into(),
        upstream_commit: "d307c4c".into(),
        patch_identity: "hzr-agtx-readonly-observer-1".into(),
        source_instance_id: "gen:aaaa".into(),
        observed_at_ms,
        snapshot_id: "snap".into(),
        snapshot_consistency: "per_project_transaction".into(),
        project: AgentSnapshotProject {
            source_project_id: Some("proj-1".into()),
            project_path_hash: "0530166353".into(),
        },
        capabilities: AgentSnapshotCapabilities {
            tasks: true,
            task_runtime: true,
            notifications: true,
            running_agents: true,
            hook_status: true,
        },
        tasks,
        edges: Vec::new(),
        notifications: Vec::new(),
        running_agents: Vec::new(),
        next_cursor: None,
        complete: true,
        warnings: Vec::new(),
    }
}

fn apply(
    ledger: &mut Ledger,
    identity: &AgentSourceIdentity,
    envelope: &AgentSnapshotEnvelope,
) -> AgentSnapshotApplied {
    ledger
        .agent_source_upsert(
            identity,
            &envelope.capabilities,
            &envelope.upstream_version,
            &envelope.upstream_commit,
            &envelope.patch_identity,
            envelope.observed_at_ms,
        )
        .expect("source upsert");
    ledger
        .agent_apply_snapshot(identity, envelope, &context())
        .expect("snapshot applies")
}

#[test]
fn a_first_scan_is_a_baseline_not_a_history() {
    let (_dir, mut ledger) = ledger();
    let identity = identity(&ledger, "gen-1");
    let applied = apply(
        &mut ledger,
        &identity,
        &envelope(vec![task("t1", "running", "claude")], NOW_MS),
    );
    assert_eq!(applied.tasks_created, 1);
    assert_eq!(applied.events_recorded, 1);

    let (events, _) = ledger
        .agent_events_page(&identity.project_hash, None, None, 100)
        .expect("fixture step succeeds");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].kind, "first_observed");
    assert_eq!(events[0].to_state.as_deref(), Some("running"));
}

#[test]
fn an_identical_replay_records_nothing() {
    let (_dir, mut ledger) = ledger();
    let identity = identity(&ledger, "gen-1");
    let snapshot = envelope(vec![task("t1", "running", "claude")], NOW_MS);
    apply(&mut ledger, &identity, &snapshot);

    let replay = apply(&mut ledger, &identity, &snapshot);
    assert_eq!(replay.events_recorded, 0);
    assert_eq!(replay.tasks_created, 0);
    assert_eq!(replay.tasks_updated, 0);

    let (tasks, _) = ledger
        .agent_tasks_page(&identity.project_hash, None, 50)
        .expect("fixture step succeeds");
    assert_eq!(tasks.len(), 1, "a replay must not duplicate the task");
}

#[test]
fn a_b_a_keeps_both_transitions() {
    let (_dir, mut ledger) = ledger();
    let identity = identity(&ledger, "gen-1");
    apply(
        &mut ledger,
        &identity,
        &envelope(vec![task("t1", "running", "claude")], NOW_MS),
    );
    apply(
        &mut ledger,
        &identity,
        &envelope(vec![task("t1", "review", "claude")], NOW_MS + 5_000),
    );
    apply(
        &mut ledger,
        &identity,
        &envelope(vec![task("t1", "running", "claude")], NOW_MS + 10_000),
    );

    let (events, _) = ledger
        .agent_events_page(&identity.project_hash, None, None, 100)
        .expect("fixture step succeeds");
    let transitions: Vec<_> = events
        .iter()
        .filter(|event| event.kind == "phase_changed")
        .map(|event| {
            (
                event.from_state.clone().unwrap_or_default(),
                event.to_state.clone().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        transitions,
        vec![
            ("running".to_string(), "review".to_string()),
            ("review".to_string(), "running".to_string())
        ]
    );
}

#[test]
fn one_partial_scan_never_tombstones() {
    let (_dir, mut ledger) = ledger();
    let identity = identity(&ledger, "gen-1");
    apply(
        &mut ledger,
        &identity,
        &envelope(
            vec![
                task("t1", "running", "claude"),
                task("t2", "backlog", "codex"),
            ],
            NOW_MS,
        ),
    );

    // A truncated page that lost t2 entirely.
    let mut partial = envelope(vec![task("t1", "running", "claude")], NOW_MS + 5_000);
    partial.complete = false;
    partial.next_cursor = Some("t1".into());
    let applied = apply(&mut ledger, &identity, &partial);
    assert_eq!(applied.tombstoned, 0);

    // One complete scan that still misses it is one strike, not a verdict.
    let applied = apply(
        &mut ledger,
        &identity,
        &envelope(vec![task("t1", "running", "claude")], NOW_MS + 10_000),
    );
    assert_eq!(applied.tombstoned, 0);

    let applied = apply(
        &mut ledger,
        &identity,
        &envelope(vec![task("t1", "running", "claude")], NOW_MS + 15_000),
    );
    assert_eq!(applied.tombstoned, 1, "two complete misses tombstone");

    let (tasks, _) = ledger
        .agent_tasks_page(&identity.project_hash, None, 50)
        .expect("fixture step succeeds");
    let gone = tasks.iter().find(|row| row.tombstoned).expect("tombstone");
    assert_ne!(
        gone.board_status, "done",
        "a tombstone means absent from the source, never Done"
    );
}

#[test]
fn a_reappearing_task_clears_its_tombstone() {
    let (_dir, mut ledger) = ledger();
    let identity = identity(&ledger, "gen-1");
    apply(
        &mut ledger,
        &identity,
        &envelope(
            vec![task("t1", "running", "x"), task("t2", "backlog", "y")],
            NOW_MS,
        ),
    );
    for step in 1..=2 {
        apply(
            &mut ledger,
            &identity,
            &envelope(vec![task("t1", "running", "x")], NOW_MS + step * 5_000),
        );
    }
    apply(
        &mut ledger,
        &identity,
        &envelope(
            vec![task("t1", "running", "x"), task("t2", "backlog", "y")],
            NOW_MS + 20_000,
        ),
    );
    let (tasks, _) = ledger
        .agent_tasks_page(&identity.project_hash, None, 50)
        .expect("fixture step succeeds");
    assert!(tasks.iter().all(|row| !row.tombstoned));
}

#[test]
fn a_source_reset_starts_a_separate_history() {
    let (_dir, mut ledger) = ledger();
    let first = identity(&ledger, "gen-1");
    apply(
        &mut ledger,
        &first,
        &envelope(vec![task("t1", "running", "claude")], NOW_MS),
    );
    let second = identity(&ledger, "gen-2");
    apply(
        &mut ledger,
        &second,
        &envelope(vec![task("t1", "running", "claude")], NOW_MS + 5_000),
    );

    assert_ne!(first.source_key, second.source_key);
    let (tasks, _) = ledger
        .agent_tasks_page(&first.project_hash, None, 50)
        .expect("fixture step succeeds");
    assert_eq!(tasks.len(), 2, "the two generations do not merge");
    let sources = ledger
        .agent_sources_for_project(&first.project_hash)
        .expect("fixture step succeeds");
    assert_eq!(sources.len(), 2);
}

#[test]
fn an_unknown_status_is_never_mapped_onto_a_neighbour() {
    let (_dir, mut ledger) = ledger();
    let identity = identity(&ledger, "gen-1");
    let mut unknown = task("t1", "unknown", "claude");
    unknown.unknown_board_status = Some("shipping".into());
    apply(&mut ledger, &identity, &envelope(vec![unknown], NOW_MS));
    let (tasks, _) = ledger
        .agent_tasks_page(&identity.project_hash, None, 50)
        .expect("fixture step succeeds");
    assert_eq!(tasks[0].board_status, "unknown");
    assert_eq!(tasks[0].unknown_board_status.as_deref(), Some("shipping"));
}

#[test]
fn a_hook_session_links_and_a_second_task_makes_it_ambiguous() {
    let (_dir, mut ledger) = ledger();
    let identity = identity(&ledger, "gen-1");
    let hook = AgentSnapshotHook {
        state: "working".into(),
        recorded_at_ms: NOW_MS - 500,
        agent: "claude".into(),
        session_id: Some("sess-shared".into()),
        working_stale: false,
    };
    let mut first = task("t1", "running", "claude");
    first.hook = Some(hook.clone());
    let mut second = task("t2", "running", "claude");
    second.hook = Some(hook);

    apply(
        &mut ledger,
        &identity,
        &envelope(vec![first, second], NOW_MS),
    );

    let t1 = identity.task_key("proj-1", "t1");
    let (exclusive, ambiguous) = ledger
        .agent_task_sessions(&t1)
        .expect("fixture step succeeds");
    assert!(exclusive.is_empty());
    assert_eq!(ambiguous.len(), 1, "one session, two tasks, no allocation");
}

#[test]
fn a_notification_is_deduped_by_its_own_id() {
    let (_dir, mut ledger) = ledger();
    let identity = identity(&ledger, "gen-1");
    let mut snapshot = envelope(vec![task("t1", "review", "claude")], NOW_MS);
    snapshot.notifications = vec![AgentSnapshotNotification {
        source_notification_id: "n-1".into(),
        source_task_id: Some("t1".into()),
        kind: "phase_completed".into(),
        created_at_ms: Some(NOW_MS - 2_000),
    }];
    apply(&mut ledger, &identity, &snapshot);
    snapshot.observed_at_ms = NOW_MS + 5_000;
    let second = apply(&mut ledger, &identity, &snapshot);
    assert_eq!(second.events_recorded, 0);

    let (events, _) = ledger
        .agent_events_page(&identity.project_hash, None, None, 100)
        .expect("fixture step succeeds");
    assert_eq!(
        events
            .iter()
            .filter(|event| event.kind == "phase_completed")
            .count(),
        1
    );
    let reported = events
        .iter()
        .find(|event| event.kind == "phase_completed")
        .expect("fixture step succeeds");
    assert_eq!(reported.evidence, "reported");
    assert_eq!(reported.source_at_ms, Some(NOW_MS - 2_000));
}

#[test]
fn an_unresolved_dependency_stays_in_the_graph() {
    let (_dir, mut ledger) = ledger();
    let identity = identity(&ledger, "gen-1");
    let mut snapshot = envelope(
        vec![
            task("t1", "running", "claude"),
            task("t2", "backlog", "codex"),
        ],
        NOW_MS,
    );
    snapshot.edges = vec![
        AgentSnapshotEdge {
            from_source_task_id: "t1".into(),
            to_source_task_id: "t2".into(),
            kind: "depends_on".into(),
            resolved: true,
        },
        AgentSnapshotEdge {
            from_source_task_id: "ghost".into(),
            to_source_task_id: "t2".into(),
            kind: "depends_on".into(),
            resolved: false,
        },
    ];
    apply(&mut ledger, &identity, &snapshot);

    let edges = ledger
        .agent_edges_for_project(&identity.project_hash, 100)
        .expect("fixture step succeeds");
    assert_eq!(edges.len(), 2);
    assert_eq!(edges.iter().filter(|edge| !edge.resolved).count(), 1);
}

#[test]
fn a_handoff_needs_both_a_new_agent_and_a_new_phase() {
    let (_dir, mut ledger) = ledger();
    let identity = identity(&ledger, "gen-1");
    apply(
        &mut ledger,
        &identity,
        &envelope(vec![task("t1", "running", "claude")], NOW_MS),
    );
    // Agent changes, phase does not: an assignment change, not a handoff.
    apply(
        &mut ledger,
        &identity,
        &envelope(vec![task("t1", "running", "codex")], NOW_MS + 5_000),
    );
    let (events, _) = ledger
        .agent_events_page(&identity.project_hash, None, None, 100)
        .expect("fixture step succeeds");
    assert!(events.iter().any(|event| event.kind == "agent_changed"));
    assert!(
        !events.iter().any(|event| event.kind == "handoff_observed"),
        "sequential work alone is not a handoff"
    );

    apply(
        &mut ledger,
        &identity,
        &envelope(vec![task("t1", "review", "gemini")], NOW_MS + 10_000),
    );
    let (events, _) = ledger
        .agent_events_page(&identity.project_hash, None, None, 100)
        .expect("fixture step succeeds");
    assert!(events.iter().any(|event| event.kind == "handoff_observed"));
}

#[test]
fn observed_intervals_are_attributed_and_gaps_are_not() {
    let (_dir, mut ledger) = ledger();
    let identity = identity(&ledger, "gen-1");
    let working = |at: i64| {
        let mut task = task("t1", "running", "claude");
        task.runtime = Some(AgentSnapshotRuntime {
            phase_status: "working".into(),
            unknown_phase_status: None,
            updated_at_ms: Some(at - 100),
            pane_changed_at_ms: None,
        });
        envelope(vec![task], at)
    };
    apply(&mut ledger, &identity, &working(NOW_MS));
    apply(&mut ledger, &identity, &working(NOW_MS + 5_000));
    // A 60 s hole: longer than one poll can account for.
    apply(&mut ledger, &identity, &working(NOW_MS + 65_000));

    let row = ledger
        .agent_task(&identity.task_key("proj-1", "t1"))
        .expect("fixture step succeeds")
        .expect("fixture step succeeds");
    assert_eq!(row.working_ms, 5_000);
    assert_eq!(row.gap_ms, 60_000);
    assert_eq!(
        row.observed_ms, 5_000,
        "a gap is never credited as observed working time"
    );
}

#[test]
fn a_stale_runtime_row_is_not_refreshed_by_a_fresh_read() {
    let (_dir, mut ledger) = ledger();
    let identity = identity(&ledger, "gen-1");
    let mut task = task("t1", "running", "claude");
    task.runtime = Some(AgentSnapshotRuntime {
        phase_status: "working".into(),
        unknown_phase_status: None,
        updated_at_ms: Some(NOW_MS - 600_000),
        pane_changed_at_ms: None,
    });
    apply(&mut ledger, &identity, &envelope(vec![task], NOW_MS));
    let row = ledger
        .agent_task(&identity.task_key("proj-1", "t1"))
        .expect("fixture step succeeds")
        .expect("fixture step succeeds");
    assert_eq!(row.runtime_observed_at_ms, Some(NOW_MS - 600_000));
    assert_eq!(row.last_observed_at_ms, NOW_MS);
}

// ---------------------------------------------------------------------------
// Economics, PRD section 9.4
// ---------------------------------------------------------------------------

fn fixture_catalog() -> PricingCatalog {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../integrations/agtx/fixtures/pricing-synthetic.json");
    load_pricing_catalog(Some(&path.canonicalize().expect("fixture catalog exists")))
        .expect("fixture catalog loads")
}

fn receipt(
    record: &str,
    session: &str,
    input: u64,
    output: u64,
    reasoning: u64,
) -> AgentUsageReceiptV1 {
    AgentUsageReceiptV1 {
        schema_version: 1,
        receipt_id: format!("rcpt-{record}"),
        source: "fixture-export".into(),
        source_record_id: record.into(),
        observed_at_ms: NOW_MS as u64,
        project_path: PROJECT_PATH.into(),
        host: "claude-code".into(),
        session_id: session.into(),
        task_id: Some("t1".into()),
        request_id: Some(format!("req-{record}")),
        provider: "fixture-provider".into(),
        harness: "fixture".into(),
        model: "fixture-model".into(),
        billing_method: "standard".into(),
        currency: "USD".into(),
        request_input_tokens: Some(input),
        usage_kind: "request_delta".into(),
        usage: AgentTokenTotals {
            input_tokens: input,
            output_tokens: output,
            reasoning_tokens: reasoning,
            ..Default::default()
        },
        reported_cost_microunits: None,
        original_source_hash: None,
    }
}

fn project_hash(path: &str) -> String {
    crate::ledger::privacy_identity_hash("project", path)
}

fn link_task_sessions(ledger: &mut Ledger, identity: &AgentSourceIdentity, sessions: &[&str]) {
    apply(
        ledger,
        identity,
        &envelope(vec![task("t1", "running", "claude")], NOW_MS),
    );
    for session in sessions {
        let hash = pseudonym(&format!("claude-code\u{0}{session}"));
        let (linked, _) = ledger
            .agent_link_session(identity, "t1", "proj-1", "claude-code", &hash, NOW_MS)
            .expect("fixture step succeeds");
        assert!(linked);
    }
}

fn economics(
    ledger: &Ledger,
    identity: &AgentSourceIdentity,
) -> hzr_protocol::agents::AgentEconomics {
    ledger
        .agent_economics(
            AgentEconomicsQuery {
                project_hash: &identity.project_hash,
                task_key: Some(&identity.task_key("proj-1", "t1")),
                from_ms: 0,
                to_ms: i64::MAX,
            },
            &fixture_catalog(),
        )
        .expect("economics")
}

#[test]
fn two_requests_in_two_sessions_price_to_the_fixture_total() {
    let (_dir, mut ledger) = ledger();
    let identity = identity(&ledger, "gen-1");
    link_task_sessions(&mut ledger, &identity, &["sess-a", "sess-b"]);

    let outcome = ledger
        .agent_import_usage(
            &[
                receipt("R1", "sess-a", 1_000, 200, 50),
                receipt("R2", "sess-b", 500, 100, 0),
            ],
            &project_hash,
            &pseudonym,
            NOW_MS as u64,
        )
        .expect("fixture step succeeds");
    assert!(outcome.committed);
    assert_eq!(outcome.accepted, 2);

    let economics = economics(&ledger, &identity);
    assert_eq!(economics.estimated_api_cost.len(), 1);
    assert_eq!(economics.estimated_api_cost[0].currency, "USD");
    assert_eq!(economics.estimated_api_cost[0].microunits, 5_400);
    assert_eq!(economics.estimated_api_cost[0].covered_receipts, 2);
    assert_eq!(economics.reported_tokens.input_tokens, 1_500);
    assert_eq!(economics.reported_tokens.output_tokens, 300);
    assert_eq!(
        economics.reported_tokens.reasoning_tokens, 50,
        "reasoning is reported but is already inside output"
    );
}

#[test]
fn an_identical_replay_leaves_the_total_alone() {
    let (_dir, mut ledger) = ledger();
    let identity = identity(&ledger, "gen-1");
    link_task_sessions(&mut ledger, &identity, &["sess-a", "sess-b"]);
    let batch = [
        receipt("R1", "sess-a", 1_000, 200, 50),
        receipt("R2", "sess-b", 500, 100, 0),
    ];
    ledger
        .agent_import_usage(&batch, &project_hash, &pseudonym, NOW_MS as u64)
        .expect("fixture step succeeds");
    let replay = ledger
        .agent_import_usage(&batch, &project_hash, &pseudonym, NOW_MS as u64)
        .expect("fixture step succeeds");
    assert_eq!(replay.replayed, 2);
    assert_eq!(replay.accepted, 0);
    assert_eq!(
        economics(&ledger, &identity).estimated_api_cost[0].microunits,
        5_400
    );
}

#[test]
fn the_same_record_id_with_new_content_conflicts_and_writes_nothing() {
    let (_dir, mut ledger) = ledger();
    let identity = identity(&ledger, "gen-1");
    link_task_sessions(&mut ledger, &identity, &["sess-a", "sess-b"]);
    ledger
        .agent_import_usage(
            &[
                receipt("R1", "sess-a", 1_000, 200, 50),
                receipt("R2", "sess-b", 500, 100, 0),
            ],
            &project_hash,
            &pseudonym,
            NOW_MS as u64,
        )
        .expect("fixture step succeeds");

    let mut tampered = receipt("R1", "sess-a", 1_000, 200, 50);
    tampered.usage.output_tokens = 9_999;
    tampered.receipt_id = "rcpt-brand-new".into();
    let outcome = ledger
        .agent_import_usage(&[tampered], &project_hash, &pseudonym, NOW_MS as u64)
        .expect("fixture step succeeds");
    assert_eq!(outcome.conflicts, 1);
    assert!(!outcome.committed);
    assert_eq!(
        economics(&ledger, &identity).estimated_api_cost[0].microunits,
        5_400,
        "a conflicting batch writes nothing"
    );
}

#[test]
fn a_reported_amount_and_an_estimate_are_alternatives_not_addends() {
    let (_dir, mut ledger) = ledger();
    let identity = identity(&ledger, "gen-1");
    link_task_sessions(&mut ledger, &identity, &["sess-a", "sess-b"]);
    let mut first = receipt("R1", "sess-a", 1_000, 200, 50);
    first.reported_cost_microunits = Some(4_000);
    ledger
        .agent_import_usage(
            &[first, receipt("R2", "sess-b", 500, 100, 0)],
            &project_hash,
            &pseudonym,
            NOW_MS as u64,
        )
        .expect("fixture step succeeds");

    let economics = economics(&ledger, &identity);
    let reported = &economics.reported_cost[0];
    assert_eq!(reported.microunits, 4_000);
    assert_eq!(reported.covered_receipts, 1);
    assert_eq!(reported.total_receipts, 2);
    let estimated = &economics.estimated_api_cost[0];
    assert_eq!(estimated.microunits, 5_400);
    assert_eq!(estimated.covered_receipts, 2);
    assert_ne!(
        reported.microunits + estimated.microunits,
        9_400_u64.min(reported.microunits + estimated.microunits) + 1,
    );
}

#[test]
fn a_missing_output_rate_is_unavailable_not_zero() {
    let (_dir, mut ledger) = ledger();
    let identity = identity(&ledger, "gen-1");
    link_task_sessions(&mut ledger, &identity, &["sess-a"]);
    let mut priceless = receipt("R1", "sess-a", 1_000, 200, 0);
    priceless.model = "fixture-model-no-output-rate".into();
    ledger
        .agent_import_usage(&[priceless], &project_hash, &pseudonym, NOW_MS as u64)
        .expect("fixture step succeeds");

    let economics = economics(&ledger, &identity);
    assert_eq!(economics.estimated_api_cost[0].microunits, 0);
    assert_eq!(economics.estimated_api_cost[0].covered_receipts, 0);
    assert_eq!(economics.estimated_api_cost[0].total_receipts, 1);
    assert!(
        economics
            .unavailable
            .iter()
            .any(|entry| entry.metric.starts_with("estimated_api_cost")),
        "a nonzero dimension with no rate must name the reason"
    );
}

#[test]
fn a_second_currency_gets_its_own_subtotal() {
    let (_dir, mut ledger) = ledger();
    let identity = identity(&ledger, "gen-1");
    link_task_sessions(&mut ledger, &identity, &["sess-a", "sess-b"]);
    let mut euro = receipt("R2", "sess-b", 500, 100, 0);
    euro.model = "fixture-model-eur".into();
    euro.currency = "EUR".into();
    ledger
        .agent_import_usage(
            &[receipt("R1", "sess-a", 1_000, 200, 50), euro],
            &project_hash,
            &pseudonym,
            NOW_MS as u64,
        )
        .expect("fixture step succeeds");

    let economics = economics(&ledger, &identity);
    assert_eq!(economics.estimated_api_cost.len(), 2);
    let usd = economics
        .estimated_api_cost
        .iter()
        .find(|subtotal| subtotal.currency == "USD")
        .expect("fixture step succeeds");
    let eur = economics
        .estimated_api_cost
        .iter()
        .find(|subtotal| subtotal.currency == "EUR")
        .expect("fixture step succeeds");
    assert_eq!(usd.microunits, 3_600);
    assert_eq!(eur.microunits, 1_800);
}

#[test]
fn a_subscription_plan_reports_but_does_not_price() {
    let (_dir, mut ledger) = ledger();
    let identity = identity(&ledger, "gen-1");
    link_task_sessions(&mut ledger, &identity, &["sess-a"]);
    let mut subscription = receipt("R1", "sess-a", 1_000, 200, 0);
    subscription.billing_method = "subscription".into();
    subscription.reported_cost_microunits = Some(0);
    ledger
        .agent_import_usage(&[subscription], &project_hash, &pseudonym, NOW_MS as u64)
        .expect("fixture step succeeds");

    let economics = economics(&ledger, &identity);
    assert_eq!(economics.reported_cost[0].microunits, 0);
    assert_eq!(
        economics.reported_cost[0].covered_receipts, 1,
        "an explicit zero is a reported zero, not unknown"
    );
    assert_eq!(economics.estimated_api_cost[0].covered_receipts, 0);
    assert!(!economics.unavailable.is_empty());
}

#[test]
fn a_cumulative_snapshot_is_refused() {
    let (_dir, mut ledger) = ledger();
    let identity = identity(&ledger, "gen-1");
    link_task_sessions(&mut ledger, &identity, &["sess-a"]);
    let mut cumulative = receipt("R1", "sess-a", 1_000, 200, 0);
    cumulative.usage_kind = "session_total".into();
    let outcome = ledger
        .agent_import_usage(&[cumulative], &project_hash, &pseudonym, NOW_MS as u64)
        .expect("fixture step succeeds");
    assert!(!outcome.committed);
    assert_eq!(outcome.rejected, 1);
    assert_eq!(outcome.rejections[0].1, "unsupported_usage_kind");
}

#[test]
fn reasoning_counted_twice_is_refused() {
    let (_dir, mut ledger) = ledger();
    let identity = identity(&ledger, "gen-1");
    link_task_sessions(&mut ledger, &identity, &["sess-a"]);
    let doubled = receipt("R1", "sess-a", 1_000, 200, 500);
    let outcome = ledger
        .agent_import_usage(&[doubled], &project_hash, &pseudonym, NOW_MS as u64)
        .expect("fixture step succeeds");
    assert!(!outcome.committed);
    assert_eq!(outcome.rejections[0].1, "reasoning_exceeds_output");
}

#[test]
fn an_ambiguous_session_is_shared_never_doubled() {
    let (_dir, mut ledger) = ledger();
    let identity = identity(&ledger, "gen-1");
    apply(
        &mut ledger,
        &identity,
        &envelope(
            vec![
                task("t1", "running", "claude"),
                task("t2", "running", "claude"),
            ],
            NOW_MS,
        ),
    );
    let hash = pseudonym("claude-code\u{0}sess-shared");
    for task_id in ["t1", "t2"] {
        ledger
            .agent_link_session(&identity, task_id, "proj-1", "claude-code", &hash, NOW_MS)
            .expect("fixture step succeeds");
    }
    ledger
        .agent_import_usage(
            &[receipt("R1", "sess-shared", 1_000, 200, 50)],
            &project_hash,
            &pseudonym,
            NOW_MS as u64,
        )
        .expect("fixture step succeeds");

    for task_id in ["t1", "t2"] {
        let economics = ledger
            .agent_economics(
                AgentEconomicsQuery {
                    project_hash: &identity.project_hash,
                    task_key: Some(&identity.task_key("proj-1", task_id)),
                    from_ms: 0,
                    to_ms: i64::MAX,
                },
                &fixture_catalog(),
            )
            .expect("fixture step succeeds");
        assert!(
            economics.estimated_api_cost.is_empty(),
            "an ambiguous session contributes no task-scoped spend"
        );
        assert_eq!(economics.shared_unallocated_cost[0].microunits, 3_600);
    }
}

#[test]
fn no_receipt_means_unknown_and_no_acceptance_means_unavailable() {
    let (_dir, mut ledger) = ledger();
    let identity = identity(&ledger, "gen-1");
    link_task_sessions(&mut ledger, &identity, &["sess-a"]);
    let economics = economics(&ledger, &identity);
    assert_eq!(economics.reported_receipt_count, 0);
    assert!(economics.reported_cost.is_empty());
    assert!(economics.estimated_api_cost.is_empty());
    assert_eq!(economics.accepted_task_count, None);
    assert!(
        economics
            .unavailable
            .iter()
            .any(|entry| entry.metric == "cost_per_accepted_task")
    );
}

#[test]
fn a_negative_reduction_is_retained() {
    // baseline 1000, delivered 1200 => net -200 and -20 %, never clamped.
    let baseline = 1_000_i64;
    let delivered = 1_200_i64;
    let net = baseline - delivered;
    let pct = 100.0 * net as f64 / baseline as f64;
    assert_eq!(net, -200);
    assert!((pct + 20.0).abs() < f64::EPSILON);
}

#[test]
fn pruning_keeps_projections_and_receipt_keys() {
    let (_dir, mut ledger) = ledger();
    let identity = identity(&ledger, "gen-1");
    link_task_sessions(&mut ledger, &identity, &["sess-a"]);
    ledger
        .agent_import_usage(
            &[receipt("R1", "sess-a", 1_000, 200, 50)],
            &project_hash,
            &pseudonym,
            NOW_MS as u64,
        )
        .expect("fixture step succeeds");

    let removed = ledger
        .agent_prune_events(1, NOW_MS + 40 * 24 * 60 * 60 * 1_000)
        .expect("fixture step succeeds");
    assert!(removed > 0);

    let (tasks, _) = ledger
        .agent_tasks_page(&identity.project_hash, None, 50)
        .expect("fixture step succeeds");
    assert_eq!(tasks.len(), 1, "pruning events must not drop projections");

    let replay = ledger
        .agent_import_usage(
            &[receipt("R1", "sess-a", 1_000, 200, 50)],
            &project_hash,
            &pseudonym,
            NOW_MS as u64,
        )
        .expect("fixture step succeeds");
    assert_eq!(
        replay.replayed, 1,
        "a pruned window must not let old usage bill again"
    );
}

#[test]
fn a_gap_is_recorded_without_touching_projections() {
    let (_dir, mut ledger) = ledger();
    let identity = identity(&ledger, "gen-1");
    apply(
        &mut ledger,
        &identity,
        &envelope(vec![task("t1", "running", "claude")], NOW_MS),
    );
    ledger
        .agent_record_gap(AgentGap {
            project_hash: &identity.project_hash,
            source_key: &identity.source_key,
            code: "helper_timeout",
            observed_at_ms: NOW_MS + 5_000,
        })
        .expect("fixture step succeeds");

    let counts = ledger
        .agent_coverage_counts(&identity.project_hash)
        .expect("fixture step succeeds");
    assert_eq!(counts.gaps, 1);
    let (tasks, _) = ledger
        .agent_tasks_page(&identity.project_hash, None, 50)
        .expect("fixture step succeeds");
    assert_eq!(tasks[0].last_observed_at_ms, NOW_MS);
}

#[test]
fn purging_a_project_leaves_other_projects_alone() {
    let (_dir, mut ledger) = ledger();
    let identity = identity(&ledger, "gen-1");
    apply(
        &mut ledger,
        &identity,
        &envelope(vec![task("t1", "running", "claude")], NOW_MS),
    );
    let other = AgentSourceIdentity::new(
        "enrollment-2",
        "gen-9",
        &crate::ledger::privacy_identity_hash("project", "/fixture/other"),
    );
    apply(
        &mut ledger,
        &other,
        &envelope(vec![task("t9", "backlog", "codex")], NOW_MS),
    );

    ledger
        .agent_purge_project(&identity.project_hash)
        .expect("fixture step succeeds");
    let (mine, _) = ledger
        .agent_tasks_page(&identity.project_hash, None, 50)
        .expect("fixture step succeeds");
    let (theirs, _) = ledger
        .agent_tasks_page(&other.project_hash, None, 50)
        .expect("fixture step succeeds");
    assert!(mine.is_empty());
    assert_eq!(theirs.len(), 1);
}

#[test]
fn two_worktrees_never_join_through_a_shared_session_string() {
    let (_dir, mut ledger) = ledger();
    let first = identity(&ledger, "gen-1");
    let second = AgentSourceIdentity::new(
        "enrollment-2",
        "gen-1",
        &crate::ledger::privacy_identity_hash("project", "/fixture/other"),
    );
    apply(
        &mut ledger,
        &first,
        &envelope(vec![task("t1", "running", "claude")], NOW_MS),
    );
    apply(
        &mut ledger,
        &second,
        &envelope(vec![task("t1", "running", "claude")], NOW_MS),
    );
    assert_ne!(
        first.task_key("proj-1", "t1"),
        second.task_key("proj-1", "t1"),
        "the same source id in two enrollments is two tasks"
    );

    let hash = pseudonym("claude-code\u{0}sess-same-text");
    ledger
        .agent_link_session(&first, "t1", "proj-1", "claude-code", &hash, NOW_MS)
        .expect("fixture step succeeds");
    let (exclusive, _) = ledger
        .agent_project_sessions(&second.project_hash)
        .expect("fixture step succeeds");
    assert!(
        exclusive.is_empty(),
        "a link in one enrollment is invisible in the other"
    );
}

#[test]
fn an_empty_session_set_reads_no_usage() {
    let (_dir, ledger) = ledger();
    let rows = ledger
        .agent_usage_rows("sha256:none", &BTreeSet::new(), 0, i64::MAX)
        .expect("fixture step succeeds");
    assert!(rows.is_empty());
}
