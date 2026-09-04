use std::fs::File;
use std::io::{self, Write};
use std::process::ExitCode;

use hzr_agent::AgentRun;
use hzr_codec::Transform;
use hzr_core::EngineManifest;
use hzr_exec::{
    CanonicalCommand, CapturedContent, CapturedStream, ExecutionOutcome, NotStarted,
    RewriteDecision, TerminationCause,
};
use hzr_index::{IndexMigrationOutcome, IndexStatus, InitOutcome};
use hzr_memory::MemoryRecord;
use hzr_protocol::{ContextPlanApiResponse, EngineHealth, HealthResponse, SearchApiResponse};
use serde::Serialize;

use crate::diagnostics::{CheckStatus, DoctorCheck, DoctorReport};
use crate::migration::MigrationScan;
use crate::stats::StatsReport;

pub fn print_json(value: &impl Serialize) -> io::Result<()> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    serde_json::to_writer_pretty(&mut output, value).map_err(io::Error::other)?;
    output.write_all(b"\n")
}

pub fn print_health(health: &HealthResponse) -> io::Result<()> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    writeln!(
        output,
        "hzrd {} protocol={} state={:?}",
        health.hzr_version, health.protocol_version, health.state
    )?;
    for engine in &health.engines {
        write_engine(&mut output, engine)?;
    }
    Ok(())
}

pub fn print_engines(manifest: &EngineManifest) -> io::Result<()> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    for engine in &manifest.engine {
        writeln!(
            output,
            "{} {} {} {}",
            engine.name, engine.version, engine.tag, engine.commit
        )?;
    }
    Ok(())
}

pub fn print_index_status(status: &IndexStatus) -> io::Result<()> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    writeln!(output, "placement: {:?}", status.placement)?;
    writeln!(output, "initialized: {}", status.initialized)?;
    writeln!(output, "vectors: {}", status.vectors_present)?;
    writeln!(output, "symbols: {}", status.symbols_present)?;
    writeln!(
        output,
        "repository graph: {}",
        status.repository_graph_present
    )?;
    writeln!(output, "duplicates: {}", status.duplicate_index_dirs.len())?;
    if let Some(generation) = &status.generation {
        writeln!(output, "generation: {}", generation.generation)?;
    }
    Ok(())
}

pub fn print_index_init(outcome: InitOutcome, status: &IndexStatus) -> io::Result<()> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    writeln!(output, "index: {outcome:?}")?;
    writeln!(output, "placement: {:?}", status.placement)?;
    writeln!(output, "watcher: not started; hzrd owns watcher lifecycle")
}

pub fn render_search(response: &SearchApiResponse, verbose: bool) -> io::Result<Vec<u8>> {
    let mut output = Vec::new();
    write_search(&mut output, response, verbose)?;
    Ok(output)
}

fn write_search(
    output: &mut impl Write,
    response: &SearchApiResponse,
    verbose: bool,
) -> io::Result<()> {
    for hit in &response.hits {
        writeln!(
            output,
            "{} [{:?} score={:.4} matches={}]",
            hit.path, response.strategy, hit.score, hit.matched_lines
        )?;
        for line in hit.snippets.iter().flat_map(|snippet| &snippet.lines) {
            writeln!(output, "{}:{}:{}", hit.path, line.line, line.text)?;
        }
        for terms in hit
            .snippets
            .iter()
            .map(|snippet| &snippet.matched_terms)
            .filter(|terms| !terms.is_empty())
        {
            writeln!(output, "terms: {}", terms.join(","))?;
        }
    }
    if response.hits.is_empty() {
        writeln!(output, "no matches")?;
    }
    write!(
        output,
        "mode={:?} shown={}/{} scanned={} skipped_large={} skipped_binary={}",
        response.effective_mode,
        response.shown_hits,
        response.total_hits,
        response.scanned_files,
        response.skipped_large,
        response.skipped_binary
    )?;
    if verbose {
        if let Some(generation) = &response.index_generation {
            write!(output, " generation={generation}")?;
        }
    }
    if let Some(reason) = &response.fallback_reason {
        write!(output, " fallback={reason}")?;
    }
    writeln!(output)?;
    if let Some(next) = &response.next_step {
        writeln!(output, "next: {next}")?;
    }
    Ok(())
}

pub fn print_context(response: &ContextPlanApiResponse) -> io::Result<()> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    writeln!(
        output,
        "context selected={} rejected={} tokens={}/{} coverage={:.2} confidence={:.2} ({}{})",
        response.pack.selected.len(),
        response.pack.rejected.len(),
        response.pack.used.value,
        response.pack.hard_limit,
        response.pack.coverage,
        response.pack.confidence,
        response.pack.ranking.method,
        if response.pack.ranking.calibrated {
            ""
        } else {
            ", uncalibrated"
        }
    )?;
    if let Some(planner) = &response.planner {
        writeln!(
            output,
            "fork-plan pipeline={} candidates={}/{} tokens={}/{}",
            planner.pipeline_version.as_deref().unwrap_or("unknown"),
            planner.candidates_selected,
            planner.candidates_total,
            planner.estimated_tokens_used,
            planner.token_budget
        )?;
    }
    for candidate in &response.pack.selected {
        write!(
            output,
            "{}",
            candidate.path.as_deref().unwrap_or(&candidate.content_ref)
        )?;
        if let Some(start) = candidate.line_start {
            write!(output, ":{start}")?;
            if let Some(end) = candidate.line_end.filter(|end| *end != start) {
                write!(output, "-{end}")?;
            }
        }
        if let Some(symbol) = &candidate.symbol {
            write!(output, " symbol={symbol}")?;
        }
        writeln!(
            output,
            " [{:?} relevance={:.4} tokens={}]",
            candidate.source, candidate.relevance, candidate.tokens.value
        )?;
        if let Some(content) = response.contents.get(&candidate.content_ref) {
            output.write_all(content.as_bytes())?;
            if !content.ends_with('\n') {
                output.write_all(b"\n")?;
            }
        }
    }
    if response.pack.selected.is_empty() {
        writeln!(output, "no context selected")?;
    }
    for warning in &response.warnings {
        writeln!(output, "warning {:?}: {}", warning.code, warning.message)?;
    }
    Ok(())
}

pub fn print_memories(memories: &[MemoryRecord], expanded: bool) -> io::Result<()> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    for memory in memories {
        write!(
            output,
            "{} [{} {:?}]",
            memory.id, memory.topic, memory.importance
        )?;
        if let Some(score) = memory.score {
            write!(output, " score={score:.4}")?;
        }
        writeln!(output)?;
        let summary = if expanded {
            memory.summary.clone()
        } else {
            compact_memory_summary(&memory.summary)
        };
        writeln!(output, "{summary}")?;
        if expanded {
            if let Some(raw) = &memory.raw_excerpt {
                writeln!(output, "raw: {raw}")?;
            }
        }
    }
    if memories.is_empty() {
        writeln!(output, "no memories")?;
    }
    Ok(())
}

fn compact_memory_summary(value: &str) -> String {
    let single_line = value.replace(['\r', '\n'], " ");
    if single_line.chars().count() <= 140 {
        return single_line;
    }
    let mut compact = single_line.chars().take(137).collect::<String>();
    compact.push_str("...");
    compact
}

pub fn print_memory_health(engine: &EngineHealth) -> io::Result<()> {
    let stdout = io::stdout();
    write_engine(&mut stdout.lock(), engine)
}

pub fn print_transform(transform: &Transform) -> io::Result<()> {
    io::stdout().lock().write_all(transform.content.as_bytes())
}

pub fn print_rewrite(decision: &RewriteDecision) -> io::Result<()> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    match decision {
        RewriteDecision::AllowRaw { reason } => writeln!(output, "allow_raw: {reason}"),
        RewriteDecision::AllowRewrite {
            command,
            source,
            reason,
        } => writeln!(
            output,
            "allow_rewrite: {} source={source:?} reason={reason}",
            render_command(command)
        ),
        RewriteDecision::Ask { proposed, reason } => writeln!(
            output,
            "ask: proposed={} reason={reason}",
            proposed.as_ref().map_or("none".into(), render_command)
        ),
        RewriteDecision::Deny { reason } => writeln!(output, "deny: {reason}"),
    }
}

pub fn print_execution(outcome: &ExecutionOutcome, json: bool) -> io::Result<ExitCode> {
    if json {
        print_json(outcome)?;
    }
    match outcome {
        ExecutionOutcome::Completed { result } => {
            if !json {
                write_capture(&result.stdout, &mut io::stdout().lock())?;
                write_capture(&result.stderr, &mut io::stderr().lock())?;
            }
            Ok(exit_code(
                result.termination.cause,
                result.termination.exit_code,
                result.termination.signal,
            ))
        }
        ExecutionOutcome::ExecutedAccountingIncomplete { result, accounting } => {
            if !json {
                write_capture(&result.stdout, &mut io::stdout().lock())?;
                write_capture(&result.stderr, &mut io::stderr().lock())?;
                writeln!(
                    io::stderr().lock(),
                    "command executed, but fidelity accounting is incomplete (code={}, retryable=false, durable_incident={}); do not replay the command",
                    accounting.code,
                    accounting.incident_persisted
                )?;
            }
            Ok(ExitCode::from(70))
        }
        ExecutionOutcome::NotStarted { disposition } => {
            if !json {
                let stderr = io::stderr();
                let mut output = stderr.lock();
                match disposition {
                    NotStarted::ApprovalRequired {
                        decision_id,
                        reason,
                        ..
                    } => {
                        if let Some(decision_id) = decision_id {
                            writeln!(
                                output,
                                "approval required decision_id={decision_id}: {reason}"
                            )?;
                        } else {
                            writeln!(
                                output,
                                "approval required without executable proposal: {reason}"
                            )?;
                        }
                    }
                    NotStarted::Denied { reason, .. } => {
                        writeln!(output, "execution denied: {reason}")?;
                    }
                }
            }
            Ok(ExitCode::from(77))
        }
    }
}

pub fn print_agent(run: &AgentRun, json: bool) -> io::Result<()> {
    if json {
        let events = run
            .events
            .iter()
            .map(|event| {
                serde_json::json!({
                    "seq": event.seq,
                    "request_id": event.request_id,
                    "kind": event.kind,
                    "data": event.data,
                })
            })
            .collect::<Vec<_>>();
        return print_json(&serde_json::json!({
            "request_id": run.request_id,
            "text": run.text,
            "response": run.json,
            "events": events,
        }));
    }
    io::stdout().lock().write_all(run.text.as_bytes())
}

pub fn print_stats(report: &StatsReport) -> io::Result<()> {
    crate::stats_output::print(report)
}

pub fn print_index_archive(outcome: &hzr_index::IndexArchiveOutcome) -> io::Result<()> {
    let (state, manifest_path, manifest) = match outcome {
        hzr_index::IndexArchiveOutcome::Planned {
            manifest_path,
            manifest,
        } => ("planned", manifest_path, manifest),
        hzr_index::IndexArchiveOutcome::Applied {
            manifest_path,
            manifest,
        } => ("applied", manifest_path, manifest),
        hzr_index::IndexArchiveOutcome::AlreadyApplied {
            manifest_path,
            manifest,
        } => ("already-applied", manifest_path, manifest),
    };
    writeln!(
        io::stdout().lock(),
        "index archive {state}: source={} backup={} sha256={} manifest={}",
        manifest.source.display,
        manifest.backup.display,
        manifest.tree_sha256,
        manifest_path.display()
    )
}

pub fn print_fleet_reconcile(report: &crate::diagnostics::FleetReconcileReport) -> io::Result<()> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    if let Some(refused) = &report.refused {
        return writeln!(output, "fleet contract: refused - {refused}");
    }
    let verb = if report.dry_run {
        "would refresh"
    } else {
        "refreshed"
    };
    let changed = report
        .rewritten
        .iter()
        .filter(|entry| entry.changed)
        .count();
    writeln!(
        output,
        "fleet contract: {verb} {changed} managed block(s) across {} registered workspace(s)",
        report.workspaces_scanned
    )?;
    for entry in report.rewritten.iter().filter(|entry| entry.changed) {
        writeln!(output, "  {} {}", entry.surface, entry.path.display())?;
    }
    for entry in &report.rewritten {
        if let Some(error) = &entry.error {
            writeln!(output, "  FAILED {}: {error}", entry.path.display())?;
        }
    }
    let mcp_changed = report
        .project_codex_mcp
        .iter()
        .filter(|entry| entry.changed)
        .count();
    writeln!(
        output,
        "fleet project Codex MCP: {verb} {mcp_changed} pin(s)"
    )?;
    writeln!(
        output,
        "fleet legacy index audit: {}",
        match report.legacy_index_audit {
            "full" => "full recursive audit requested",
            _ => "not requested (use --migrate-legacy-indexes for the recursive audit)",
        }
    )?;
    for entry in report
        .project_codex_mcp
        .iter()
        .filter(|entry| entry.changed)
    {
        writeln!(
            output,
            "  codex {} -> {}",
            entry.workspace.display(),
            entry.path.display()
        )?;
    }
    for entry in &report.project_codex_mcp {
        if let Some(error) = &entry.error {
            writeln!(output, "  FAILED {}: {error}", entry.path.display())?;
        }
    }
    for entry in &report.legacy_indexes {
        writeln!(
            output,
            "  grepai {}: {}{}",
            entry.workspace.display(),
            entry.state,
            entry
                .error
                .as_ref()
                .map(|error| format!(" - {error}"))
                .unwrap_or_default()
        )?;
        for command in &entry.resolution_commands {
            let argv = std::iter::once(command.program)
                .chain(command.arguments.iter().map(String::as_str))
                .collect::<Vec<_>>();
            let rendered = serde_json::to_string(&argv).map_err(io::Error::other)?;
            writeln!(output, "    next argv: {}", rendered)?;
        }
    }
    for entry in &report.workspace_errors {
        writeln!(
            output,
            "  FAILED workspace {}: {}",
            entry.workspace.display(),
            entry.error
        )?;
    }
    // A refreshed block next to a surviving user directive is still a finding. Name those
    // files so the refresh cannot read as "this workspace is now clean".
    for path in &report.conflicts_left_for_the_owner {
        writeln!(
            output,
            "  conflict left for the owner: {} (user-authored directives are never rewritten)",
            path.display()
        )?;
    }
    Ok(())
}

pub fn print_doctor(report: &DoctorReport) -> io::Result<()> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    write_doctor(&mut output, report)
}

fn write_doctor(output: &mut impl Write, report: &DoctorReport) -> io::Result<()> {
    writeln!(
        output,
        "HZR {} doctor: {}",
        report.hzr_version,
        if report.healthy { "healthy" } else { "failed" }
    )?;
    if let Some(receipt) = &report.fidelity_reconcile {
        writeln!(
            output,
            "fidelity reconcile: reservation={} resolution={:?} operation_recorded={} allowance_released={} cleanup_complete={} idempotent_replay={}",
            receipt.reservation_id,
            receipt.resolution,
            receipt.operation_recorded,
            receipt.allowance_released,
            receipt.cleanup_complete,
            receipt.idempotent_replay
        )?;
    }

    // Постоянные host limits (например global_codec) не смешиваем с actionable WARN/ERROR.
    let mut host_limits = Vec::new();
    let mut actionable = Vec::new();
    for check in &report.checks {
        if is_host_limit(check) {
            host_limits.push(check);
            continue;
        }
        let status = match check.status {
            CheckStatus::Pass => "PASS",
            CheckStatus::Warning => "WARN",
            CheckStatus::Error => "ERROR",
        };
        writeln!(output, "{status:<5} {}: {}", check.name, check.detail)?;
        if is_actionable_finding(check) {
            actionable.push(check);
        }
    }

    if !host_limits.is_empty() {
        writeln!(output)?;
        writeln!(output, "Host limits (permanent):")?;
        for check in &host_limits {
            writeln!(output, "NOTE  {}: {}", check.name, check.detail)?;
        }
    }

    if !actionable.is_empty() {
        writeln!(output)?;
        writeln!(output, "Next actions:")?;
        for check in actionable {
            writeln!(output, "- {}: {}", check.name, remediation_for(check))?;
        }
    }
    Ok(())
}

/// Постоянный предел хоста: нельзя исправить установкой HZR (см. README unintercepted).
fn is_host_limit(check: &DoctorCheck) -> bool {
    check.name.ends_with("_global_codec")
}

fn is_actionable_finding(check: &DoctorCheck) -> bool {
    !is_host_limit(check) && matches!(check.status, CheckStatus::Warning | CheckStatus::Error)
}

/// Короткая remediation для футера: предпочитаем явную команду/инструкцию из detail.
fn remediation_for(check: &DoctorCheck) -> &str {
    for marker in [
        "; run `",
        "; Re-register ",
        ". Re-register ",
        "; add ",
        "; stop ",
        "; restart ",
    ] {
        if let Some(pos) = check.detail.find(marker) {
            // Пропускаем "; " или ". " — оставляем императив целиком.
            return check.detail[pos + 2..].trim();
        }
    }
    match check.name.as_str() {
        "client_mcp_ownership" => {
            "replace direct ICM MCP registration with HZR (`hzr install --force` or client-specific add)"
        }
        "hzr_on_path" if check.detail.contains("not on PATH") => {
            "add the HZR install prefix to PATH"
        }
        "degraded_rewrites" => "run a managed rewrite; the next one reconciles the ledger",
        _ => check.detail.as_str(),
    }
}

pub fn print_migration(scan: &MigrationScan) -> io::Result<()> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    writeln!(
        output,
        "read-only migration scan: {}",
        scan.workspace.display()
    )?;
    if let Some(placement) = &scan.index_placement {
        writeln!(output, "grepai placement: {placement:?}")?;
    }
    for duplicate in &scan.duplicate_indexes {
        writeln!(output, "duplicate grepai index: {}", duplicate.display())?;
    }
    for artifact in &scan.artifacts {
        writeln!(output, "{}: {}", artifact.kind, artifact.path.display())?;
    }
    for warning in &scan.warnings {
        writeln!(output, "warning: {warning}")?;
    }
    Ok(())
}

pub fn print_migration_apply(outcome: &IndexMigrationOutcome) -> io::Result<()> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    let (label, manifest_path, manifest) = match outcome {
        IndexMigrationOutcome::Applied {
            manifest_path,
            manifest,
        } => ("applied", manifest_path, manifest),
        IndexMigrationOutcome::AlreadyApplied {
            manifest_path,
            manifest,
        } => ("already_applied", manifest_path, manifest),
    };
    writeln!(output, "migration: {label}")?;
    writeln!(output, "id: {}", manifest.migration_id)?;
    writeln!(output, "source: {}", manifest.source.display)?;
    writeln!(output, "target: {}", manifest.target.display)?;
    writeln!(output, "backup retained: {}", manifest.backup.display)?;
    writeln!(output, "manifest: {}", manifest_path.display())
}

fn write_engine(output: &mut impl Write, engine: &EngineHealth) -> io::Result<()> {
    write!(output, "{} {:?}", engine.name, engine.state)?;
    if let Some(version) = &engine.version {
        write!(output, " version={version}")?;
    }
    if let Some(detail) = &engine.detail {
        write!(output, " — {detail}")?;
    }
    writeln!(output)
}

fn write_capture(stream: &CapturedStream, output: &mut impl Write) -> io::Result<()> {
    match &stream.content {
        CapturedContent::Inline { bytes } => output.write_all(bytes),
        CapturedContent::Spilled { path } => {
            let mut file = File::open(path)?;
            io::copy(&mut file, output).map(|_| ())
        }
    }
}

fn render_command(command: &CanonicalCommand) -> String {
    match command {
        CanonicalCommand::Argv { program, args } => std::iter::once(program.as_str())
            .chain(args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" "),
        CanonicalCommand::Shell { shell, command } => format!("{shell} -c {command}"),
    }
}

fn exit_code(cause: TerminationCause, code: Option<i32>, signal: Option<i32>) -> ExitCode {
    match cause {
        TerminationCause::Exited => code
            .and_then(|code| u8::try_from(code).ok())
            .map_or(ExitCode::FAILURE, ExitCode::from),
        TerminationCause::Signaled => signal
            .and_then(|signal| u8::try_from(128_i32.saturating_add(signal)).ok())
            .map_or(ExitCode::FAILURE, ExitCode::from),
        TerminationCause::TimedOut => ExitCode::from(124),
        TerminationCause::Cancelled => ExitCode::from(130),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::process::ExitCode;

    use hzr_exec::TerminationCause;

    use super::exit_code;
    use super::write_doctor;
    use crate::diagnostics::{CheckStatus, DoctorCheck, DoctorReport};

    #[test]
    fn test_exit_code_preserves_completed_process_status() {
        assert_eq!(
            exit_code(TerminationCause::Exited, Some(42), None),
            ExitCode::from(42)
        );
    }

    #[test]
    fn test_exit_code_uses_timeout_convention() {
        assert_eq!(
            exit_code(TerminationCause::TimedOut, None, None),
            ExitCode::from(124)
        );
    }

    fn render_doctor(report: &DoctorReport) -> String {
        let mut output = Vec::new();
        write_doctor(&mut output, report).expect("doctor render");
        String::from_utf8(output).expect("UTF-8 doctor output")
    }

    fn sample_report() -> DoctorReport {
        DoctorReport {
            hzr_version: "0.3.8".into(),
            config_path: PathBuf::from("/tmp/hzr.toml"),
            data_dir: PathBuf::from("/tmp/hzr-data"),
            workspace: PathBuf::from("/tmp/project"),
            healthy: false,
            checks: vec![
                DoctorCheck {
                    name: "hook_ownership".into(),
                    status: CheckStatus::Pass,
                    detail: "HZR owns hooks".into(),
                },
                DoctorCheck {
                    name: "client_mcp_workspace".into(),
                    status: CheckStatus::Warning,
                    detail: "claude-desktop registered without `--workspace`, so the memory \
                             namespace comes from the directory the client launches from; \
                             Re-register with `hzr mcp config --client <client> --workspace <dir>`"
                        .into(),
                },
                DoctorCheck {
                    name: "claude_global_codec".into(),
                    status: CheckStatus::Warning,
                    detail: "unintercepted: the host exposes no global request/response hook; \
                             HZR records no codec savings for this path"
                        .into(),
                },
                DoctorCheck {
                    name: "codex_global_codec".into(),
                    status: CheckStatus::Warning,
                    detail: "unintercepted: the host exposes no global request/response hook; \
                             HZR records no codec savings for this path"
                        .into(),
                },
                DoctorCheck {
                    name: "hzr_on_path".into(),
                    status: CheckStatus::Error,
                    detail: "no durable `hzr` in /tmp/bin; run `hzr install --prefix /tmp/bin`"
                        .into(),
                },
            ],
            client_workspace_bindings: Vec::new(),
            response_codec_coverage: Vec::new(),
            repair: None,
            fidelity_reconcile: None,
            fleet_reconcile: None,
        }
    }

    #[test]
    fn test_doctor_labels_permanent_host_limits_separately_from_actionable_warns() {
        let rendered = render_doctor(&sample_report());

        assert!(
            rendered.contains("Host limits (permanent):"),
            "permanent codec host limits must not share the WARN stream with actionable findings"
        );
        assert!(
            rendered.contains("NOTE  claude_global_codec:"),
            "claude_global_codec is a permanent host limit, not an actionable WARN"
        );
        assert!(
            rendered.contains("NOTE  codex_global_codec:"),
            "codex_global_codec is a permanent host limit, not an actionable WARN"
        );
        assert!(
            !rendered.contains("WARN  claude_global_codec:"),
            "host limits must not render as WARN"
        );
        assert!(
            !rendered.contains("WARN  codex_global_codec:"),
            "host limits must not render as WARN"
        );
        assert!(
            rendered.contains("WARN  client_mcp_workspace:"),
            "actionable warnings stay in the main check list"
        );
    }

    #[test]
    fn test_doctor_lists_next_actions_for_actionable_findings_only() {
        let rendered = render_doctor(&sample_report());

        assert!(
            rendered.contains("Next actions:"),
            "human doctor output must end with remediations for actionable findings"
        );
        assert!(
            rendered.contains("client_mcp_workspace:"),
            "workspace pinning warn must appear in Next actions"
        );
        assert!(
            rendered.contains("hzr mcp config --client <client> --workspace <dir>"),
            "next action should carry the remediation command"
        );
        assert!(
            rendered.contains("hzr_on_path:"),
            "path error must appear in Next actions"
        );
        assert!(
            rendered.contains("hzr install --prefix /tmp/bin"),
            "next action should carry the install remediation"
        );
        assert!(
            !rendered
                .split("Next actions:")
                .nth(1)
                .expect("footer")
                .contains("global_codec"),
            "host limits must not appear under Next actions"
        );
    }

    #[test]
    fn test_doctor_omits_next_actions_when_only_host_limits_warn() {
        let mut report = sample_report();
        report.healthy = true;
        report.checks.retain(|check| {
            check.status == CheckStatus::Pass || check.name.ends_with("_global_codec")
        });
        let rendered = render_doctor(&report);

        assert!(rendered.contains("Host limits (permanent):"));
        assert!(
            !rendered.contains("Next actions:"),
            "a report with only permanent host limits has nothing actionable to list"
        );
    }
}
