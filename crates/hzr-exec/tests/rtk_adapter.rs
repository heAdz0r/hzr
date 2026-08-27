#![cfg(unix)]

use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail};
use hzr_exec::{
    CanonicalCommand, CapturedContent, ExecutionEnvelope, ExecutionOutcome, ExecutionPipeline,
    ForkCoreInvocation, ForkRuntimePaths, PINNED_RTK_VERSION, PinnedRtkAdapter, RewriteDecision,
    RewriteSource, RtkAdapterConfig, RtkRewriteInterface, RtkRewriteRoute, StdinSpec,
    TerminationCause,
};
use hzr_protocol::{EnforcementTier, EvasionClass, EvasionPathForm, FidelityValidation};
use tempfile::TempDir;

static TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

struct FakeFork {
    _directory: TempDir,
    binary: PathBuf,
    runtime_paths: ForkRuntimePaths,
}

impl FakeFork {
    fn new(version: &str, rewrite_body: &str) -> Result<Self> {
        let directory = TempDir::new()?;
        let binary = directory.path().join("rtk");
        let metadata: toml::Value =
            include_str!("../../../fork-core/CURRENT_ENGINE.toml").parse()?;
        let contract = serde_json::json!({
            "contract_version": hzr_engine_contract::ENGINE_CONTRACT_VERSION,
            "engine_version": metadata["engine_version"].as_str().ok_or_else(|| anyhow!("missing engine version"))?,
            "manifest_sha256": metadata["manifest_sha256"].as_str().ok_or_else(|| anyhow!("missing manifest hash"))?,
            "content_manifest_sha256": metadata["content_manifest_sha256"].as_str().ok_or_else(|| anyhow!("missing content manifest hash"))?,
        });
        let script = format!(
            r#"#!/bin/sh
check_hzr_env() {{
  test -n "${{RTK_MEM_DB_PATH:-}}"
  test -z "${{RTK_DB_PATH:-}}"
  test -n "${{RTK_TEE_DIR:-}}"
  test -n "${{RTK_AUDIT_DIR:-}}"
  test -n "${{HZR_INTERNAL_ACCOUNTING_RECEIPT_JOURNAL:-}}"
  test -n "${{HZR_INTERNAL_ACCOUNTING_FAILURE_JOURNAL:-}}"
  test -n "${{HZR_INTERNAL_ACCOUNTING_CORRELATION:-}}"
  test "${{RTK_TEE:-}}" = 0
  test "${{RTK_TELEMETRY_DISABLED:-}}" = 1
}}
check_hzr_env
if test "${{1:-}}" = --version; then
  printf '%s\n' 'rtk {version}'
  exit 0
fi
if test "${{1:-}}" = contract && test "${{2:-}}" = --json; then
  printf '%s\n' '{contract}'
  exit 0
fi
if test "${{1:-}}" = rewrite && test "${{2:-}}" = --help; then
  printf '%s\n' 'Usage: rtk rewrite [ARGS]... Raw command to rewrite'
  exit 0
fi
if test "${{1:-}}" = proxy && test "${{2:-}}" = --help; then
  printf '%s\n' 'Usage: rtk proxy [ARGS]... Execute command without filtering'
  exit 0
fi
if test "${{1:-}}" = rewrite-plan; then
  {rewrite_body}
  exit $?
fi
if test "${{1:-}}" = proxy; then
  shift
  "$@"
  exit $?
fi
if test "${{1:-}}" = filtered; then
  printf 'fork-core'
  exit 0
fi
if test "${{1:-}}" = first; then
  printf 'A'
  exit 0
fi
if test "${{1:-}}" = second; then
  printf 'B'
  exit 0
fi
if test "${{1:-}}" = stdin; then
  cat
  exit 0
fi
if test "${{1:-}}" = accounting; then
  printf '%s' "${{RTK_TRACKING_DISABLED:-0}}"
  exit 0
fi
exit 64
"#
        );
        fs::write(&binary, script)?;
        let mut permissions = fs::metadata(&binary)?.permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&binary, permissions)?;
        let runtime_paths = ForkRuntimePaths::from_data_root(&directory.path().join("data"));
        Ok(Self {
            _directory: directory,
            binary,
            runtime_paths,
        })
    }

    fn config(&self) -> RtkAdapterConfig {
        RtkAdapterConfig {
            binary: self.binary.clone(),
            runtime_paths: Some(self.runtime_paths.clone()),
            ..RtkAdapterConfig::default()
        }
    }
}

fn completed(outcome: ExecutionOutcome) -> Result<hzr_exec::ExecutionResult> {
    match outcome {
        ExecutionOutcome::Completed { result } => Ok(*result),
        ExecutionOutcome::ExecutedAccountingIncomplete { accounting, .. } => Err(
            std::io::Error::other(format!("unexpected accounting failure: {accounting:?}")).into(),
        ),
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

async fn execute_decision(
    requested: CanonicalCommand,
    decision: RewriteDecision,
) -> Result<hzr_exec::ExecutionResult> {
    let mut envelope = ExecutionEnvelope::allow_raw(requested);
    envelope.decision = decision;
    completed(ExecutionPipeline.execute(envelope).await?)
}

#[tokio::test]
async fn test_adapter_exit_zero_executes_exact_pinned_fork() -> Result<()> {
    let _guard = TEST_LOCK.lock().await;
    let fork = FakeFork::new(
        PINNED_RTK_VERSION,
        r#"printf '{"decision":"rewrite","proposed":"rtk filtered"}'"#,
    )?;
    let decoy_directory = TempDir::new()?;
    let decoy = decoy_directory.path().join("rtk");
    fs::write(&decoy, "#!/bin/sh\nprintf 'wrong-rtk'\n")?;
    fs::set_permissions(&decoy, fs::Permissions::from_mode(0o700))?;
    let adapter = PinnedRtkAdapter::detect(fork.config()).await;

    assert_eq!(adapter.capabilities().rewrite, RtkRewriteInterface::ForkCli);
    let requested = CanonicalCommand::shell("git status");
    let decision = adapter.decide(&requested).await;
    let mut envelope = ExecutionEnvelope::allow_raw(requested);
    envelope.decision = decision;
    envelope.environment.set.insert(
        "PATH".to_owned(),
        decoy_directory.path().to_string_lossy().into_owned(),
    );
    let result = completed(ExecutionPipeline.execute(envelope).await?)?;

    assert_eq!(inline(result.stdout.content)?, b"fork-core");
    assert!(!result.raw_fallback);
    Ok(())
}

#[tokio::test]
async fn test_adapter_consumes_typed_payload_free_evasion_plan() -> Result<()> {
    let _guard = TEST_LOCK.lock().await;
    let fork = FakeFork::new(
        PINNED_RTK_VERSION,
        r#"printf '{"decision":"rewrite","proposed":"rtk read README.md","attribution":{"class":"e2_shell_wrapper","wrapper_depth":1,"path_form":"bare","stage_count":1,"hatch_marker":false,"avoidable":true,"tier":"t1_named_correction","fidelity_validation":"not_requested"}}'"#,
    )?;
    let adapter = PinnedRtkAdapter::detect(fork.config()).await;
    let outcome = adapter
        .decide_with_plan_in(&CanonicalCommand::shell("sh -c 'cat README.md'"), None)
        .await;
    assert!(
        outcome.evasion.is_some(),
        "missing attribution: {outcome:?}"
    );
    let evasion = outcome.evasion.expect("typed evasion attribution");

    assert_eq!(evasion.class, EvasionClass::E2ShellWrapper);
    assert_eq!(evasion.wrapper_depth, 1);
    assert_eq!(evasion.path_form, EvasionPathForm::Bare);
    assert_eq!(evasion.tier, EnforcementTier::T1NamedCorrection);
    assert_eq!(
        evasion.fidelity_validation,
        FidelityValidation::NotRequested
    );
    assert!(matches!(
        outcome.decision,
        RewriteDecision::AllowRewrite { .. }
    ));
    let mut environment = hzr_exec::Environment::default();
    outcome.apply_evasion_environment(&mut environment)?;
    let internal = environment
        .set
        .get(hzr_exec::INTERNAL_EVASION_ENV)
        .expect("internal attribution environment");
    assert!(!internal.contains("README.md"));
    assert!(!internal.contains("sh -c"));
    Ok(())
}

#[tokio::test]
async fn test_adapter_exit_one_uses_proxy_and_propagates_child_exit() -> Result<()> {
    let _guard = TEST_LOCK.lock().await;
    let fork = FakeFork::new(PINNED_RTK_VERSION, r#"printf '{"decision":"proxy"}'"#)?;
    let adapter = PinnedRtkAdapter::detect(fork.config()).await;
    let requested = CanonicalCommand::argv("/bin/sh", vec!["-c".to_owned(), "exit 7".to_owned()])?;
    let decision = adapter.decide(&requested).await;

    assert!(matches!(decision, RewriteDecision::AllowRewrite { .. }));
    let result = execute_decision(requested, decision).await?;
    assert_eq!(result.termination.cause, TerminationCause::Exited);
    assert_eq!(result.termination.exit_code, Some(7));
    assert!(!result.raw_fallback);
    Ok(())
}

#[tokio::test]
async fn test_adapter_exit_two_returns_typed_deny() -> Result<()> {
    let _guard = TEST_LOCK.lock().await;
    let fork = FakeFork::new(
        PINNED_RTK_VERSION,
        r#"printf '{"decision":"deny","reason":"permission_policy"}'"#,
    )?;
    let adapter = PinnedRtkAdapter::detect(fork.config()).await;

    assert!(matches!(
        adapter.decide(&CanonicalCommand::shell("git push")).await,
        RewriteDecision::Deny { .. }
    ));
    Ok(())
}

#[tokio::test]
async fn test_adapter_exit_three_preserves_fork_approval() -> Result<()> {
    let _guard = TEST_LOCK.lock().await;
    let fork = FakeFork::new(
        PINNED_RTK_VERSION,
        r#"printf '{"decision":"ask","proposed":"rtk filtered","reason":"permission_policy"}'"#,
    )?;
    let adapter = PinnedRtkAdapter::detect(fork.config()).await;

    assert!(matches!(
        adapter.decide(&CanonicalCommand::shell("git status")).await,
        RewriteDecision::Ask {
            proposed: Some(_),
            ..
        }
    ));
    Ok(())
}

#[tokio::test]
async fn test_adapter_exit_four_asks_without_reconstructing_opaque_shell() -> Result<()> {
    let _guard = TEST_LOCK.lock().await;
    let fork = FakeFork::new(
        PINNED_RTK_VERSION,
        r#"printf '{"decision":"ask","reason":"canonical_policy"}'"#,
    )?;
    let adapter = PinnedRtkAdapter::detect(fork.config()).await;

    assert!(matches!(
        adapter
            .decide(&CanonicalCommand::shell("sh -c 'git status"))
            .await,
        RewriteDecision::Ask { proposed: None, .. }
    ));
    Ok(())
}

#[tokio::test]
async fn test_byte_fidelity_mode_requires_the_fork_to_select_proxy() -> Result<()> {
    let _guard = TEST_LOCK.lock().await;
    let fork = FakeFork::new(
        PINNED_RTK_VERSION,
        r#"if test "${HZR_INTERNAL_BYTE_FIDELITY:-}" = 1; then printf '{"decision":"proxy"}'; else printf '{"decision":"rewrite","proposed":"rtk filtered"}'; fi"#,
    )?;
    let adapter = PinnedRtkAdapter::detect(fork.config()).await;
    let command = CanonicalCommand::shell("rg -n needle src");

    assert!(matches!(
        adapter.decide_byte_fidelity_in(&command, None).await,
        RewriteDecision::AllowRewrite {
            source: RewriteSource::Rtk {
                route: RtkRewriteRoute::Proxy,
                ..
            },
            ..
        }
    ));
    assert!(matches!(
        adapter.decide_in(&command, None).await,
        RewriteDecision::AllowRewrite {
            source: RewriteSource::Rtk {
                route: RtkRewriteRoute::Optimized,
                ..
            },
            ..
        }
    ));
    Ok(())
}

#[tokio::test]
async fn test_adapter_preserves_compound_fork_rewrite() -> Result<()> {
    let _guard = TEST_LOCK.lock().await;
    let fork = FakeFork::new(
        PINNED_RTK_VERSION,
        r#"printf '{"decision":"rewrite","proposed":"rtk first && rtk second"}'"#,
    )?;
    let adapter = PinnedRtkAdapter::detect(fork.config()).await;
    let requested = CanonicalCommand::shell("git status && cargo test");
    let decision = adapter.decide(&requested).await;
    let result = execute_decision(requested, decision).await?;

    assert_eq!(inline(result.stdout.content)?, b"AB");
    Ok(())
}

#[tokio::test]
async fn test_adapter_passes_complete_shell_program_to_fork_rewrite() -> Result<()> {
    let _guard = TEST_LOCK.lock().await;
    let directory = TempDir::new()?;
    let marker = directory.path().join("raw-command");
    let marker_text = marker.to_string_lossy().replace('\'', "'\\''");
    let body = format!(
        "printf '%s' \"$2\" > '{marker_text}'; printf '{{\"decision\":\"deny\",\"reason\":\"permission_policy\"}}'"
    );
    let fork = FakeFork::new(PINNED_RTK_VERSION, &body)?;
    let adapter = PinnedRtkAdapter::detect(fork.config()).await;
    let cases = [
        "printf 'a b' | sed 's/ /_/'",
        "printf '%s\\n' x > ./result.txt",
        "cat <<'EOF'\nalpha $HOME\nEOF",
        "git status && cargo test || printf 'failed\\n'",
        "printf '%s\\0' a b | xargs -0 -n1 printf '<%s>\\n'",
    ];

    for raw in cases {
        let decision = adapter.decide(&CanonicalCommand::shell(raw)).await;
        assert!(matches!(decision, RewriteDecision::Deny { .. }));
        assert_eq!(fs::read_to_string(&marker)?, raw);
    }
    Ok(())
}

#[tokio::test]
async fn test_adapter_version_mismatch_fails_closed() -> Result<()> {
    let _guard = TEST_LOCK.lock().await;
    let fork = FakeFork::new("9.9.9", "exit 1")?;
    let adapter = PinnedRtkAdapter::detect(fork.config()).await;

    assert!(matches!(
        adapter.capabilities().rewrite,
        RtkRewriteInterface::Unavailable { .. }
    ));
    assert!(matches!(
        adapter.decide(&CanonicalCommand::shell("printf raw")).await,
        RewriteDecision::Deny { .. }
    ));
    Ok(())
}

#[tokio::test]
async fn test_adapter_rejects_matching_help_and_version_with_wrong_contract_identity() -> Result<()>
{
    let _guard = TEST_LOCK.lock().await;
    let fork = FakeFork::new(PINNED_RTK_VERSION, "exit 1")?;
    let script = fs::read_to_string(&fork.binary)?;
    let metadata: toml::Value = include_str!("../../../fork-core/CURRENT_ENGINE.toml").parse()?;
    let expected = metadata["manifest_sha256"]
        .as_str()
        .ok_or_else(|| anyhow!("missing manifest hash"))?;
    fs::write(&fork.binary, script.replace(expected, &"0".repeat(64)))?;
    let adapter = PinnedRtkAdapter::detect(fork.config()).await;

    assert!(matches!(
        adapter.capabilities().rewrite,
        RtkRewriteInterface::Unavailable { ref reason }
            if reason.contains("contract identity")
    ));
    Ok(())
}

#[tokio::test]
async fn test_adapter_missing_binary_fails_closed() -> Result<()> {
    let _guard = TEST_LOCK.lock().await;
    let directory = TempDir::new()?;
    let config = RtkAdapterConfig {
        binary: directory.path().join("missing/rtk"),
        runtime_paths: Some(ForkRuntimePaths::from_data_root(
            &directory.path().join("data"),
        )),
        ..RtkAdapterConfig::default()
    };
    let adapter = PinnedRtkAdapter::detect(config).await;

    assert!(matches!(
        adapter.decide(&CanonicalCommand::shell("printf raw")).await,
        RewriteDecision::Deny { .. }
    ));
    Ok(())
}

#[tokio::test]
async fn test_adapter_rewrite_timeout_fails_closed() -> Result<()> {
    let _guard = TEST_LOCK.lock().await;
    let fork = FakeFork::new(PINNED_RTK_VERSION, "while :; do :; done")?;
    let mut config = fork.config();
    config.rewrite_timeout_ms = 10;
    let adapter = PinnedRtkAdapter::detect(config).await;

    assert!(matches!(
        adapter.decide(&CanonicalCommand::shell("printf raw")).await,
        RewriteDecision::Deny { .. }
    ));
    Ok(())
}

#[tokio::test]
async fn test_adapter_probe_timeout_fails_closed() -> Result<()> {
    let _guard = TEST_LOCK.lock().await;
    let directory = TempDir::new()?;
    let binary = directory.path().join("rtk");
    fs::write(&binary, "#!/bin/sh\nwhile :; do :; done\n")?;
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700))?;
    let config = RtkAdapterConfig {
        binary,
        runtime_paths: Some(ForkRuntimePaths::from_data_root(
            &directory.path().join("data"),
        )),
        probe_timeout_ms: 10,
        ..RtkAdapterConfig::default()
    };
    let adapter = PinnedRtkAdapter::detect(config).await;

    assert!(matches!(
        adapter.capabilities().rewrite,
        RtkRewriteInterface::Unavailable { .. }
    ));
    assert!(matches!(
        adapter.decide(&CanonicalCommand::shell("printf raw")).await,
        RewriteDecision::Deny { .. }
    ));
    Ok(())
}

#[tokio::test]
async fn test_runner_executes_exact_argv_with_centralized_runtime_and_stdin() -> Result<()> {
    let _guard = TEST_LOCK.lock().await;
    let fork = FakeFork::new(PINNED_RTK_VERSION, "exit 1")?;
    let adapter = PinnedRtkAdapter::detect(fork.config()).await;
    let runner = adapter.runner()?;
    let mut invocation = ForkCoreInvocation::new(vec!["stdin".to_owned()]);
    invocation.stdin = StdinSpec::Bytes {
        data: b"exact bytes".to_vec(),
    };
    let result = completed(runner.execute(invocation).await?)?;

    assert!(matches!(
        result.executed,
        CanonicalCommand::Argv { ref program, ref args }
            if Path::new(program) == fs::canonicalize(&fork.binary)?
                && args == &["stdin".to_owned()]
    ));
    assert_eq!(inline(result.stdout.content)?, b"exact bytes");
    assert!(!result.raw_fallback);
    assert!(fork.runtime_paths.tee_dir.is_dir());
    assert!(fork.runtime_paths.audit_dir.is_dir());
    assert_eq!(
        fs::metadata(
            fork.runtime_paths
                .tee_dir
                .parent()
                .ok_or_else(|| anyhow!("tee directory must have a parent"))?,
        )?
        .permissions()
        .mode()
            & 0o777,
        0o700
    );
    Ok(())
}

#[tokio::test]
async fn test_runner_can_disable_tracking_for_observability_canaries() -> Result<()> {
    let _guard = TEST_LOCK.lock().await;
    let fork = FakeFork::new(PINNED_RTK_VERSION, "exit 1")?;
    let adapter = PinnedRtkAdapter::detect(fork.config()).await;
    let runner = adapter.runner()?;
    let invocation = ForkCoreInvocation::new(vec!["accounting".to_owned()]).without_accounting();

    let result = completed(runner.execute(invocation).await?)?;

    assert_eq!(inline(result.stdout.content)?, b"1");
    Ok(())
}

#[tokio::test]
async fn test_decide_in_runs_fork_policy_from_requested_cwd() -> Result<()> {
    let _guard = TEST_LOCK.lock().await;
    let directory = TempDir::new()?;
    let marker = directory.path().join("rewrite-cwd");
    let body = format!(
        "pwd > {}; printf '{{\"decision\":\"proxy\"}}'",
        marker.display()
    );
    let fork = FakeFork::new(PINNED_RTK_VERSION, &body)?;
    let adapter = PinnedRtkAdapter::detect(fork.config()).await;

    let decision = adapter
        .decide_in(&CanonicalCommand::shell("unknown"), Some(directory.path()))
        .await;

    assert!(matches!(decision, RewriteDecision::AllowRewrite { .. }));
    assert_eq!(
        Path::new(fs::read_to_string(marker)?.trim()),
        fs::canonicalize(directory.path())?
    );
    Ok(())
}

#[tokio::test]
async fn test_std_command_uses_exact_binary_and_centralized_environment() -> Result<()> {
    let _guard = TEST_LOCK.lock().await;
    let fork = FakeFork::new(PINNED_RTK_VERSION, "exit 1")?;
    let adapter = PinnedRtkAdapter::detect(fork.config()).await;
    let runner = adapter.runner()?;
    let command = runner.std_command(&["--version".to_owned()])?;

    assert_eq!(command.get_program(), fs::canonicalize(&fork.binary)?);
    assert!(command.get_envs().any(|(key, value)| {
        key == OsStr::new("RTK_MEM_DB_PATH")
            && value == Some(fork.runtime_paths.memory_db.as_os_str())
    }));
    assert!(
        command
            .get_envs()
            .any(|(key, value)| { key == OsStr::new("RTK_DB_PATH") && value.is_none() })
    );
    assert!(
        command
            .get_envs()
            .any(|(key, value)| { key == OsStr::new("RTK_TEE") && value == Some(OsStr::new("0")) })
    );
    assert!(command.get_envs().any(|(key, value)| {
        key == OsStr::new("RTK_TELEMETRY_DISABLED") && value == Some(OsStr::new("1"))
    }));
    let non_utf8 = OsString::from_vec(vec![b'x', 0xff]);
    let os_command = runner.std_command_os(std::slice::from_ref(&non_utf8))?;
    assert_eq!(os_command.get_args().next(), Some(non_utf8.as_os_str()));
    Ok(())
}
