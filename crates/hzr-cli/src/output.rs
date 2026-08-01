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

use crate::diagnostics::{CheckStatus, DoctorReport};
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

pub fn print_search(response: &SearchApiResponse) -> io::Result<()> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
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
    } else {
        write!(
            output,
            "shown={}/{} scanned={} skipped={}",
            response.shown_hits,
            response.total_hits,
            response.scanned_files,
            response.skipped_large + response.skipped_binary
        )?;
        if let Some(generation) = &response.index_generation {
            write!(output, " generation={generation}")?;
        }
        writeln!(output)?;
    }
    Ok(())
}

pub fn print_context(response: &ContextPlanApiResponse) -> io::Result<()> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    writeln!(
        output,
        "context selected={} rejected={} tokens={}/{} coverage={:.2} confidence={:.2}",
        response.pack.selected.len(),
        response.pack.rejected.len(),
        response.pack.used.value,
        response.pack.hard_limit,
        response.pack.coverage,
        response.pack.confidence
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

pub fn print_memories(memories: &[MemoryRecord]) -> io::Result<()> {
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
        writeln!(output, "{}", memory.summary)?;
        if let Some(raw) = &memory.raw_excerpt {
            writeln!(output, "raw: {raw}")?;
        }
    }
    if memories.is_empty() {
        writeln!(output, "no memories")?;
    }
    Ok(())
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

pub fn print_doctor(report: &DoctorReport) -> io::Result<()> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    writeln!(
        output,
        "HZR {} doctor: {}",
        report.hzr_version,
        if report.healthy { "healthy" } else { "failed" }
    )?;
    for check in &report.checks {
        let status = match check.status {
            CheckStatus::Pass => "PASS",
            CheckStatus::Warning => "WARN",
            CheckStatus::Error => "ERROR",
        };
        writeln!(output, "{status:<5} {}: {}", check.name, check.detail)?;
    }
    Ok(())
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
    use std::process::ExitCode;

    use hzr_exec::TerminationCause;

    use super::exit_code;

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
}
