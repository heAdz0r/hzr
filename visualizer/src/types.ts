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

export interface DashboardObservedUsage {
  tasks: number;
  accepted: number;
  actual_input_tokens: number;
  actual_output_tokens: number;
  estimated_input_tokens: number;
  cost_microusd: number;
}

export interface DashboardEstimatedEfficiency {
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
  registry_warnings: number;
  observed_usage: DashboardObservedUsage;
  estimated_efficiency: DashboardEstimatedEfficiency;
  help: DashboardHelpCommand[];
  notes: string[];
}
