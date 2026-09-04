export type DashboardState =
  | "ready"
  | "degraded"
  | "rebuilding"
  | "standby"
  | "stopped"
  | "unknown";

export type ProjectState =
  | "ready"
  | "warming"
  | "registered"
  | "unavailable"
  | "degraded";

export interface MemoryTopic {
  id: string;
  label: string;
  memory_count: number;
  average_weight: number;
  newest_at: string | null;
}

export interface MemoryEdge {
  source: string;
  target: string;
  relationship_count: number;
}

export type MemoryRetrieval = "hybrid" | "fts5" | "unavailable";

export interface DashboardMemoryObservatory {
  state: DashboardState;
  project: string | null;
  retrieval: MemoryRetrieval;
  observed_at_ms: number;
  latency_ms: number;
  transport: string;
  source: string;
  memory_count: number;
  visible_memory_count: number;
  hidden_memory_count: number;
  topics: MemoryTopic[];
  edges: MemoryEdge[];
  truncated: boolean;
  diagnostic_command: string;
  detail: string;
}

export interface DashboardMemoryTopicDetails {
  id: string;
  label: string;
  memory_count: number;
  visible_memory_count: number;
  hidden_memory_count: number;
  truncated: boolean;
  memories: DashboardMemoryDetail[];
}

export interface DashboardMemoryDetail {
  id: string;
  created_at: string;
  updated_at: string;
  last_accessed: string | null;
  access_count: number;
  weight: number;
  summary: string;
  raw_excerpt: string | null;
  keywords: string[];
  importance: string;
  source_type: string | null;
  source_data: string | null;
  related_ids: string[];
}

export interface DashboardIndexObservatory {
  state: DashboardState;
  project: string | null;
  observed_at_ms: number;
  generation: string | null;
  config_fingerprint: string | null;
  artifacts: {
    initialized: boolean;
    vectors_present: boolean;
    symbols_present: boolean;
    repository_graph_present: boolean;
    size_bytes: number;
    modified_at_ms: number | null;
  };
  watcher: {
    state: DashboardState;
    pid: number | null;
    uptime_ms: number | null;
    owned_by_hzr: boolean;
    ready_marker_observed: boolean;
    detail: string;
  };
  search_activity: {
    state: DashboardState;
    ledger_id: number | null;
    observed_at: string | null;
    operation: string | null;
    command_hash: string | null;
    project_hash: string | null;
    agent: string | null;
    session_hash: string | null;
    route: "optimized" | "raw" | "native_unaccounted" | null;
    execution_ms: number | null;
    detail: string;
  };
  diagnostic_command: string;
}

export interface DashboardDeliverySummary {
  operations: number;
  tokens_estimated: number | null;
  legacy_unknown_stage_operations: number;
  coverage: string;
  complete: boolean;
}

export interface DashboardLocalActivity {
  explicit_delivery?: DashboardDeliverySummary;
  project: string | null;
  accounting_policy_version: string;
  excluded_legacy_operations: number;
  operations: number;
  optimized_operations: number;
  raw_operations: number;
  native_unaccounted_operations: number;
  unmeasured_bypass_operations: number;
  baseline_tokens_estimated: number;
  delivered_tokens_estimated: number;
  gross_avoided_tokens_estimated: number;
  regression_tokens_estimated: number;
  net_avoided_tokens_estimated: number;
  total_execution_ms: number;
  first_record_at: string | null;
  last_record_at: string | null;
  unscoped_operations: number;
  measurement: string;
  recent_operations: DashboardLocalOperation[];
}

export interface DashboardLocalOperation {
  ledger_id: number;
  timestamp: string;
  operation: string;
  route: "optimized" | "raw" | "native_unaccounted";
  command_hash: string;
  project_hash: string | null;
  agent: string | null;
  session_hash: string | null;
  producer_version: string | null;
  policy_version: string | null;
  baseline_tokens_estimated: number;
  delivered_tokens_estimated: number;
  net_avoided_tokens_estimated: number;
  execution_ms: number;
  replacement: string | null;
  rationale: string | null;
}

export type TraceStage = "request" | "policy" | "engine" | "ledger";
export type TraceState = "completed" | "approval_required" | "denied" | "failed" | "skipped";

export interface DashboardTraceSpan {
  sequence: number;
  trace_hash: string;
  linked_trace_hash: string | null;
  span_id: number;
  parent_span_id: number | null;
  stage: TraceStage;
  state: TraceState;
  engine: string;
  observed_at_ms: number;
  duration_ms: number;
  project_hash: string | null;
  session_hash: string | null;
  route: string | null;
  error_code: string | null;
  producer_version: string;
  policy_version: string;
  generation: string | null;
}

export interface DashboardLifecycleEvent {
  sequence: number;
  observed_at_ms: number;
  engine: string;
  kind: "starting" | "ready" | "degraded" | "restart_scheduled" | "reaped" | "stopped";
  project_hash: string | null;
  detail_code: string;
  producer_version: string;
  generation: string | null;
}

export interface DashboardObservability {
  trace_spans: DashboardTraceSpan[];
  lifecycle_events: DashboardLifecycleEvent[];
  next_cursor: number | null;
  truncated: boolean;
}

export interface DashboardProviderReceipts {
  state: "available" | "no_receipts";
  records: number;
  accepted: number;
  actual_input_tokens: number;
  actual_output_tokens: number;
  cost_microusd: number;
  detail: string;
}

export interface DashboardEconomicAmount {
  currency: string;
  baseline_microunits: number;
  delivered_microunits: number;
  savings_microunits: number;
}

export interface DashboardRawPublicEstimate {
  currency: string;
  savings_microunits: number;
  avoided_input_tokens_estimated: number;
  pricing_basis: string;
  catalog_identity: string;
  entry_version: string;
  preliminary: boolean;
  disclaimer: string;
}

export interface DashboardSessionCommand {
  command_family: string;
  executions: number;
  net_avoided_tokens_estimated: number;
}

export interface DashboardSessionRoi {
  explicit_delivery?: DashboardDeliverySummary;
  session_hash: string | null;
  operations: number;
  baseline_tokens_estimated: number;
  delivered_tokens_estimated: number;
  net_avoided_tokens_estimated: number;
  top_commands: DashboardSessionCommand[];
  selected_harness: string;
  selected_provider: string;
  selected_model: string;
  selected_method: string;
  selected_request_input_tokens: number | null;
  selected_pricing_basis: string;
  catalog_identity: string | null;
  raw_public_estimate: DashboardRawPublicEstimate | null;
  raw_public_estimate_unavailable_reason: string | null;
  imported_claim_records: number;
  reported_actual: DashboardEconomicAmount | null;
  receipt_provenance: string | null;
  receipt_externally_verified: boolean;
  detail: string;
}

export interface DashboardService {
  id: string;
  name: string;
  version: string | null;
  state: DashboardState;
  detail: string;
  command: string | null;
}

export interface DashboardProjectArtifacts {
  config_present: boolean;
  vectors_present: boolean;
  symbols_present: boolean;
  repository_graph_present: boolean;
  size_bytes: number;
  modified_at_ms: number | null;
}

export interface DashboardProject {
  name: string;
  root: string;
  repository_id: string;
  worktree_id: string;
  git_backed: boolean;
  linked_worktree: boolean;
  state: ProjectState;
  registered_at_ms: number;
  last_seen_at_ms: number;
  artifacts: DashboardProjectArtifacts;
  command: string;
}

export interface DashboardProjectPage {
  projects: DashboardProject[];
  total: number;
  offset: number;
  limit: number;
  next_offset: number | null;
}

export interface DashboardObservedUsage {
  tasks: number;
  accepted: number;
  actual_input_tokens: number;
  actual_output_tokens: number;
  estimated_input_tokens: number;
  cost_microusd: number;
}

export interface DashboardEstimatedEfficiency {
  accounting_policy_version: string;
  excluded_legacy_operations: number;
  operations: number;
  baseline_tokens_estimated: number;
  delivered_tokens_estimated: number;
  gross_avoided_tokens_estimated: number;
  regression_tokens_estimated: number;
  net_avoided_tokens_estimated: number;
  reduction_pct: number;
  total_execution_ms: number;
  measurement: string;
}

export interface DashboardHelpCommand {
  label: string;
  description: string;
  command: string;
}

export interface DashboardResponse {
  protocol_version: number;
  hzr_version: string;
  visualizer_version: string;
  generated_at_ms: number;
  uptime_ms: number;
  daemon_endpoint: string;
  overall_state: DashboardState;
  services: DashboardService[];
  projects: DashboardProject[];
  projects_total: number;
  projects_next_offset: number | null;
  selected_worktree_id: string | null;
  registry_warnings: number;
  observed_usage: DashboardObservedUsage;
  estimated_efficiency: DashboardEstimatedEfficiency;
  memory_observatory: DashboardMemoryObservatory;
  index_observatory: DashboardIndexObservatory;
  local_activity: DashboardLocalActivity;
  observability: DashboardObservability;
  provider_receipts: DashboardProviderReceipts;
  session_roi: DashboardSessionRoi;
  help: DashboardHelpCommand[];
  notes: string[];
}
