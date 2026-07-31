use std::fs;

use anyhow::{Result, bail};
use hzr_exec::{
    CanonicalCommand, CaptureConfig, CaptureOverflow, CapturedContent, ExecutionEnvelope,
    ExecutionEvent, ExecutionOutcome, ExecutionPipeline, ExecutionResult, RewriteDecision,
    RewriteSource, ShellSafety, TerminationCause, analyze_shell,
};
use tempfile::TempDir;

fn completed(outcome: ExecutionOutcome) -> Result<ExecutionResult> {
    match outcome {
        ExecutionOutcome::Completed { result } => Ok(*result),
        ExecutionOutcome::NotStarted { disposition } => {
            bail!("execution did not start: {disposition:?}")
        }
    }
}

fn inline(content: CapturedContent) -> Result<Vec<u8>> {
    match content {
        CapturedContent::Inline { bytes } => Ok(bytes),
        CapturedContent::Spilled { path } => Ok(fs::read(path)?),
    }
}

async fn run_shell(command: &str) -> Result<ExecutionResult> {
    let envelope = ExecutionEnvelope::allow_raw(CanonicalCommand::shell(command));
    completed(ExecutionPipeline.execute(envelope).await?)
}

#[tokio::test]
async fn test_pipeline_preserves_pipe_semantics_without_rewrite() -> Result<()> {
    assert!(matches!(
        analyze_shell("printf 'one\\ntwo\\n' | tail -n 1"),
        ShellSafety::RawRequired { .. }
    ));
    let result = run_shell("printf 'one\\ntwo\\n' | tail -n 1").await?;

    assert_eq!(inline(result.stdout.content)?, b"two\n");
    assert_eq!(result.termination.exit_code, Some(0));
    Ok(())
}

#[tokio::test]
async fn test_pipeline_preserves_and_or_semantics_without_rewrite() -> Result<()> {
    for command in ["true && printf success", "false || printf recovered"] {
        assert!(matches!(
            analyze_shell(command),
            ShellSafety::RawRequired { .. }
        ));
    }

    let success = run_shell("true && printf success").await?;
    let recovered = run_shell("false || printf recovered").await?;
    assert_eq!(inline(success.stdout.content)?, b"success");
    assert_eq!(inline(recovered.stdout.content)?, b"recovered");
    Ok(())
}

#[tokio::test]
async fn test_pipeline_preserves_shell_quoting() -> Result<()> {
    let result = run_shell("printf '%s' 'a b \"c\"'").await?;

    assert_eq!(inline(result.stdout.content)?, b"a b \"c\"");
    Ok(())
}

#[tokio::test]
async fn test_pipeline_keeps_xargs_raw() -> Result<()> {
    assert!(matches!(
        analyze_shell("xargs -n1 printf"),
        ShellSafety::RawRequired { .. }
    ));
    let result = run_shell("printf 'a\\nb\\n' | xargs -n1 printf '[%s]'").await?;

    assert_eq!(inline(result.stdout.content)?, b"[a][b]");
    Ok(())
}

#[tokio::test]
async fn test_pipeline_reports_exact_exit_and_stderr_channels() -> Result<()> {
    let result = run_shell("printf stdout; printf stderr >&2; exit 7").await?;

    assert_eq!(inline(result.stdout.content)?, b"stdout");
    assert_eq!(inline(result.stderr.content)?, b"stderr");
    assert_eq!(result.termination.cause, TerminationCause::Exited);
    assert_eq!(result.termination.exit_code, Some(7));
    assert_eq!(result.termination.signal, None);
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn test_pipeline_reports_exact_unix_signal() -> Result<()> {
    let result = run_shell("kill -TERM $$").await?;

    assert_eq!(result.termination.cause, TerminationCause::Signaled);
    assert_eq!(result.termination.exit_code, None);
    assert_eq!(result.termination.signal, Some(15));
    Ok(())
}

#[tokio::test]
async fn test_pipeline_timeout_terminates_process_group() -> Result<()> {
    let mut envelope = ExecutionEnvelope::allow_raw(CanonicalCommand::shell("sleep 5"));
    envelope.timeout_ms = Some(30);
    envelope.termination_grace_ms = 20;
    let result = completed(ExecutionPipeline.execute(envelope).await?)?;

    assert_eq!(result.termination.cause, TerminationCause::TimedOut);
    assert!(result.duration_ms < 2_000);
    Ok(())
}

#[tokio::test]
async fn test_pipeline_cancellation_is_typed() -> Result<()> {
    let envelope = ExecutionEnvelope::allow_raw(CanonicalCommand::shell("sleep 5"));
    let handle = ExecutionPipeline.start(envelope)?;
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    handle.cancel();
    let result = completed(handle.wait().await?)?;

    assert_eq!(result.termination.cause, TerminationCause::Cancelled);
    assert!(result.duration_ms < 2_000);
    Ok(())
}

#[tokio::test]
async fn test_pipeline_streams_events_without_affecting_capture() -> Result<()> {
    let envelope =
        ExecutionEnvelope::allow_raw(CanonicalCommand::shell("printf first; printf second >&2"));
    let mut handle = ExecutionPipeline.start(envelope)?;
    let mut saw_stdout = false;
    let mut saw_stderr = false;
    while let Some(event) = handle.next_event().await {
        match event {
            ExecutionEvent::Output {
                stream: hzr_exec::ExecutionStream::Stdout,
                ..
            } => saw_stdout = true,
            ExecutionEvent::Output {
                stream: hzr_exec::ExecutionStream::Stderr,
                ..
            } => saw_stderr = true,
            ExecutionEvent::Started { .. } | ExecutionEvent::Finished { .. } => {}
        }
    }
    let result = completed(handle.wait().await?)?;

    assert!(saw_stdout);
    assert!(saw_stderr);
    assert_eq!(inline(result.stdout.content)?, b"first");
    assert_eq!(inline(result.stderr.content)?, b"second");
    Ok(())
}

#[tokio::test]
async fn test_pipeline_spills_exact_output_after_memory_limit() -> Result<()> {
    let directory = TempDir::new()?;
    let mut envelope = ExecutionEnvelope::allow_raw(CanonicalCommand::shell("printf 1234567890"));
    envelope.capture = CaptureConfig {
        memory_limit_bytes: 4,
        max_capture_bytes: 32,
        overflow: CaptureOverflow::Spill {
            directory: directory.path().to_owned(),
        },
        event_buffer: 4,
    };
    let result = completed(ExecutionPipeline.execute(envelope).await?)?;

    assert!(result.stdout.is_exact());
    assert_eq!(result.stdout.total_bytes, 10);
    assert!(matches!(
        &result.stdout.content,
        CapturedContent::Spilled { .. }
    ));
    assert_eq!(inline(result.stdout.content)?, b"1234567890");
    Ok(())
}

#[tokio::test]
async fn test_pipeline_allows_hzr_policy_raw_fallback_before_child_start() -> Result<()> {
    let requested =
        CanonicalCommand::argv("/bin/sh", vec!["-c".to_owned(), "printf raw".to_owned()])?;
    let mut envelope = ExecutionEnvelope::allow_raw(requested.clone());
    envelope.decision = RewriteDecision::AllowRewrite {
        command: CanonicalCommand::argv(
            "/definitely/missing/hzr-rtk",
            vec!["git".to_owned(), "status".to_owned()],
        )?,
        source: RewriteSource::HzrPolicy,
        reason: "test adapter".to_owned(),
    };
    let result = completed(ExecutionPipeline.execute(envelope).await?)?;

    assert!(result.raw_fallback);
    assert_eq!(result.executed, requested);
    assert_eq!(inline(result.stdout.content)?, b"raw");
    Ok(())
}

#[tokio::test]
async fn test_pipeline_never_bypasses_fork_when_managed_spawn_fails() -> Result<()> {
    let directory = TempDir::new()?;
    let marker = directory.path().join("raw-bypass");
    let requested = CanonicalCommand::shell(format!("touch {}", marker.display()));
    let mut envelope = ExecutionEnvelope::allow_raw(requested);
    envelope.decision = RewriteDecision::AllowRewrite {
        command: CanonicalCommand::argv(
            "/definitely/missing/hzr-fork-core",
            vec!["git".to_owned(), "status".to_owned()],
        )?,
        source: RewriteSource::Rtk {
            version: hzr_exec::PINNED_RTK_VERSION.to_owned(),
        },
        reason: "managed fork-core".to_owned(),
    };

    assert!(ExecutionPipeline.execute(envelope).await.is_err());
    assert!(!marker.exists());
    Ok(())
}

#[tokio::test]
async fn test_pipeline_ask_never_executes_proposed_command() -> Result<()> {
    let directory = TempDir::new()?;
    let marker = directory.path().join("marker");
    let requested = CanonicalCommand::shell(format!("touch {}", marker.display()));
    let mut envelope = ExecutionEnvelope::allow_raw(requested.clone());
    envelope.decision = RewriteDecision::Ask {
        proposed: Some(requested),
        reason: "approval required".to_owned(),
    };
    let outcome = ExecutionPipeline.execute(envelope).await?;

    assert!(matches!(outcome, ExecutionOutcome::NotStarted { .. }));
    assert!(!marker.exists());
    Ok(())
}
