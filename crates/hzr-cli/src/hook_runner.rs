use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use hzr_core::{Config, first_class_replacement};
use hzr_exec::{
    CanonicalCommand, ForkRuntimePaths, PinnedRtkAdapter, RewriteDecision, RtkAdapterConfig,
};
use hzr_protocol::{ContextPlanApiRequest, ExecApiRequest};
use serde_json::{Value, json};
use tokio::time::timeout;

use crate::client::DaemonClient;

const HOOK_TIMEOUT: Duration = Duration::from_secs(2);
const MAX_HOOK_INPUT_BYTES: u64 = 2 * 1024 * 1024;

pub async fn dispatch(config: &Config) -> Result<()> {
    let input = read_input()?;
    let tool_name = input
        .get("tool_name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match tool_name {
        "Bash" => rewrite(config, &input).await,
        "Agent" | "Task" => task(config, &input).await,
        _ => Ok(()),
    }
}

fn read_input() -> Result<Value> {
    let mut bytes = Vec::new();
    io::stdin()
        .take(MAX_HOOK_INPUT_BYTES + 1)
        .read_to_end(&mut bytes)
        .context("failed to read hook input")?;
    if bytes.len() as u64 > MAX_HOOK_INPUT_BYTES {
        bail!("hook input exceeds {MAX_HOOK_INPUT_BYTES} bytes");
    }
    serde_json::from_slice(&bytes).context("hook input is not valid JSON")
}

async fn rewrite(config: &Config, input: &Value) -> Result<()> {
    let Some(raw) = input.pointer("/tool_input/command").and_then(Value::as_str) else {
        return Ok(());
    };
    if raw.is_empty() {
        return Ok(());
    }
    let cwd = std::env::current_dir().context("failed to resolve hook working directory")?;
    let request = ExecApiRequest {
        cwd: cwd.to_string_lossy().into_owned(),
        command: raw.to_owned(),
        timeout_ms: Some(HOOK_TIMEOUT.as_millis() as u64),
    };
    let managed = if let Ok(client) = DaemonClient::from_config(config) {
        timeout(HOOK_TIMEOUT, client.exec_rewrite(&request))
            .await
            .ok()
            .and_then(Result::ok)
    } else {
        None
    };
    let decision = match managed {
        Some(decision) => {
            // The daemon answered, so any earlier gap is now behind us: close it instead of
            // leaving `hzr stats` pinned to INCOMPLETE for the rest of the installation.
            let _ = clear_reconciled_rewrites(config);
            decision
        }
        None => {
            let _ = record_degraded_rewrite(config);
            fallback_decision(config, raw, &cwd).await
        }
    };
    write_decision(input, steer_to_first_class(raw, decision))
}

/// Offer the first-class HZR command when the agent is about to reach the shell unfiltered.
///
/// This is the counterpart to the bypass panel in `hzr stats`: measurement alone changes
/// nothing if the agent never learns the replacement. The decision is deliberately `Ask`
/// and not `Deny` — raw is the correct tool for checksums, generated files and complete
/// logs, so the escape hatch stays one keystroke away. Nothing is proposed when the engine
/// has already rewritten the command, or when no equivalent exists.
fn steer_to_first_class(raw: &str, decision: RewriteDecision) -> RewriteDecision {
    if !matches!(decision, RewriteDecision::AllowRaw { .. }) {
        return decision;
    }
    let Some(replacement) = first_class_replacement(raw) else {
        return decision;
    };
    RewriteDecision::Ask {
        proposed: Some(CanonicalCommand::shell(replacement.suggestion.clone())),
        reason: format!(
            "`{}` reaches the shell unfiltered and is recorded as an optimizer bypass. \
             {}. Accept the proposed command, or re-run the original to keep raw output.",
            replacement.tool, replacement.rationale
        ),
    }
}

async fn fallback_decision(config: &Config, raw: &str, cwd: &Path) -> RewriteDecision {
    let adapter = PinnedRtkAdapter::detect(RtkAdapterConfig {
        binary: config.engines.binary("rtk"),
        runtime_paths: Some(ForkRuntimePaths::from_data_root(&config.data_dir)),
        probe_timeout_ms: HOOK_TIMEOUT.as_millis() as u64,
        rewrite_timeout_ms: HOOK_TIMEOUT.as_millis() as u64,
    })
    .await;
    adapter
        .decide_in(&CanonicalCommand::shell(raw), Some(cwd))
        .await
}

fn write_decision(input: &Value, decision: RewriteDecision) -> Result<()> {
    let output = match decision {
        RewriteDecision::AllowRaw { .. } => return Ok(()),
        RewriteDecision::AllowRewrite {
            command, reason, ..
        } => json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "allow",
                "permissionDecisionReason": reason,
                "updatedInput": updated_command(input, &command)?,
            }
        }),
        RewriteDecision::Ask { proposed, reason } => {
            let mut hook = json!({
                "hookEventName": "PreToolUse",
                "permissionDecision": "ask",
                "permissionDecisionReason": reason,
            });
            if let Some(command) = proposed {
                hook["updatedInput"] = updated_command(input, &command)?;
            }
            json!({"hookSpecificOutput": hook})
        }
        RewriteDecision::Deny { reason } => json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": reason,
            }
        }),
    };
    serde_json::to_writer(io::stdout().lock(), &output)?;
    io::stdout().lock().write_all(b"\n")?;
    Ok(())
}

fn updated_command(input: &Value, command: &CanonicalCommand) -> Result<Value> {
    let mut tool_input = input
        .get("tool_input")
        .cloned()
        .context("Bash hook input has no tool_input")?;
    tool_input["command"] = Value::String(render_command(command)?);
    Ok(tool_input)
}

fn render_command(command: &CanonicalCommand) -> Result<String> {
    match command {
        CanonicalCommand::Shell { command, .. } => Ok(command.clone()),
        CanonicalCommand::Argv { program, args } => {
            let mut words = Vec::with_capacity(args.len() + 1);
            words.push(shell_quote(program));
            words.extend(args.iter().map(|argument| shell_quote(argument)));
            Ok(words.join(" "))
        }
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

async fn task(config: &Config, input: &Value) -> Result<()> {
    let subagent = input
        .pointer("/tool_input/subagent_type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if subagent.eq_ignore_ascii_case("explore") {
        return write_hook_json(json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": "Native Explore is replaced by the unified HZR planner. Use `hzr context plan <intent>` or `hzr rtk -- memory explore <path>`.",
            }
        }));
    }
    let Some(prompt) = input.pointer("/tool_input/prompt").and_then(Value::as_str) else {
        return Ok(());
    };
    if prompt.is_empty() || prompt.contains("## HZR Unified Context") {
        return Ok(());
    }
    let workspace = std::env::current_dir()
        .context("failed to resolve hook working directory")?
        .canonicalize()
        .context("failed to canonicalize hook working directory")?;
    let client = match DaemonClient::from_config(config) {
        Ok(client) => client,
        Err(_) => return Ok(()),
    };
    let response = match timeout(
        HOOK_TIMEOUT,
        client.context_plan(&ContextPlanApiRequest {
            workspace: workspace.to_string_lossy().into_owned(),
            intent: prompt.chars().take(700).collect(),
            path: None,
            topic: None,
            search_limit: 10,
            memory_limit: 5,
        }),
    )
    .await
    {
        Ok(Ok(response)) => response,
        _ => return Ok(()),
    };
    let context = context_brief(&serde_json::to_value(&response)?);
    let mut tool_input = input
        .get("tool_input")
        .cloned()
        .context("agent hook input has no tool_input")?;
    tool_input["prompt"] = Value::String(format!(
        "## HZR Unified Context\n{context}\n\n---\n\n{prompt}"
    ));
    write_hook_json(json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "allow",
            "permissionDecisionReason": "HZR injected one bounded graph-first + ICM context plan",
            "updatedInput": tool_input,
        }
    }))
}

/// Largest number of leads to put in front of a subagent. A plan is a starting point, not a
/// reading list; past a handful the brief competes with the task it was meant to serve.
const MAX_BRIEF_LEADS: usize = 12;

/// Render a context plan as a brief a subagent can act on.
///
/// The plan used to be prepended as a minified JSON envelope with no explanation — no
/// glossary, no statement of what the entries are, no instruction on what to do with them. A
/// subagent that cannot tell what the block is will either ignore it and re-derive everything,
/// or treat it as established fact. The second failure is the worse one: a plan candidate is a
/// ranked guess, so the brief has to say that out loud.
fn context_brief(plan: &Value) -> String {
    let leads: Vec<String> = plan
        .pointer("/pack/selected")
        .and_then(Value::as_array)
        .map(|selected| {
            selected
                .iter()
                .filter(|candidate| {
                    candidate.get("source").and_then(Value::as_str) != Some("memory")
                })
                .filter_map(|candidate| {
                    let path = candidate.get("path").and_then(Value::as_str)?;
                    let span = match (
                        candidate.get("line_start").and_then(Value::as_u64),
                        candidate.get("line_end").and_then(Value::as_u64),
                    ) {
                        (Some(start), Some(end)) => format!(":{start}-{end}"),
                        _ => String::new(),
                    };
                    let symbol = candidate
                        .get("symbol")
                        .and_then(Value::as_str)
                        .map(|symbol| format!(" ({symbol})"))
                        .unwrap_or_default();
                    Some(format!("- {path}{span}{symbol}"))
                })
                .take(MAX_BRIEF_LEADS)
                .collect()
        })
        .unwrap_or_default();

    if leads.is_empty() {
        return "HZR planned this task and found no code leads for it. Start from the task \
                itself; there is nothing here to confirm or rule out."
            .to_owned();
    }

    format!(
        "HZR ranked these as the most likely relevant places for the task below. They are \
         unverified leads, not findings: confirm each one before relying on it, and ignore any \
         that do not fit. Read them with `hzr rtk -- read <path> --from N --to M`, and search \
         with `hzr search \"<pattern>\" --mode exact` when a lead is wrong.\n\n{}",
        leads.join("\n")
    )
}

fn write_hook_json(value: Value) -> Result<()> {
    let stdout = io::stdout();
    let mut output = stdout.lock();
    serde_json::to_writer(&mut output, &value)?;
    output.write_all(b"\n")?;
    Ok(())
}

/// Rewrites the daemon never saw, and therefore never entered in the usage ledger.
///
/// The distinction between the two counts is the whole point. `unreconciled_rewrites` is
/// an *open* gap — the daemon has not served a rewrite since these happened, so the ledger
/// is still missing them and coverage is genuinely incomplete. `lifetime_rewrites` is the
/// historical total, kept so that closing a gap never looks like erasing it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize)]
pub struct AccountingCoverage {
    pub unreconciled_rewrites: usize,
    pub lifetime_rewrites: usize,
    pub complete: bool,
    pub last_degraded_at_unix: Option<u64>,
}

#[cfg(test)]
impl AccountingCoverage {
    /// The state of an installation that has never lost the daemon. `Default` cannot say
    /// this on its own, because a defaulted `complete: false` would read as a real gap.
    pub fn default_complete() -> Self {
        Self {
            complete: true,
            ..Self::default()
        }
    }
}

fn degraded_log_path(config: &Config) -> std::path::PathBuf {
    config.data_dir.join("ledger/degraded-rewrites.log")
}

fn degraded_total_path(config: &Config) -> std::path::PathBuf {
    config.data_dir.join("ledger/degraded-rewrites.total")
}

fn record_degraded_rewrite(config: &Config) -> Result<()> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    record_degraded_rewrite_at(config, timestamp)
}

fn record_degraded_rewrite_at(config: &Config, timestamp: u64) -> Result<()> {
    let ledger = config.data_dir.join("ledger");
    fs::create_dir_all(&ledger)
        .with_context(|| format!("failed to create {}", ledger.display()))?;
    let path = degraded_log_path(config);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    writeln!(file, "{timestamp}")?;
    Ok(())
}

/// Fold the open gap into the lifetime total once the daemon proves it is serving again.
///
/// Called after every successful managed rewrite, so the common case — nothing pending —
/// must not touch the filesystem beyond one `metadata` probe.
fn clear_reconciled_rewrites(config: &Config) -> Result<()> {
    let path = degraded_log_path(config);
    let pending = match fs::metadata(&path) {
        Ok(metadata) if metadata.len() > 0 => read_degraded_log(config)?.len(),
        Ok(_) => 0,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(error).with_context(|| format!("failed to inspect {}", path.display()));
        }
    };
    if pending == 0 {
        return Ok(());
    }
    let total = read_lifetime_total(config)?.unwrap_or(0) + pending;
    let total_path = degraded_total_path(config);
    fs::write(&total_path, format!("{total}\n"))
        .with_context(|| format!("failed to write {}", total_path.display()))?;
    fs::write(&path, "").with_context(|| format!("failed to truncate {}", path.display()))?;
    Ok(())
}

fn read_degraded_log(config: &Config) -> Result<Vec<u64>> {
    let path = degraded_log_path(config);
    match fs::read_to_string(&path) {
        Ok(content) => Ok(content
            .lines()
            .filter_map(|line| line.trim().parse::<u64>().ok())
            .collect()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
    }
}

fn read_lifetime_total(config: &Config) -> Result<Option<usize>> {
    let path = degraded_total_path(config);
    match fs::read_to_string(&path) {
        Ok(content) => Ok(content.trim().parse::<usize>().ok()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
    }
}

pub fn degraded_rewrite_coverage(config: &Config) -> Result<AccountingCoverage> {
    let pending = read_degraded_log(config)?;
    // A ledger written by an earlier HZR has no lifetime file; its open log *is* the
    // history, so fall back to it instead of reporting a zero total next to a non-zero gap.
    let lifetime = read_lifetime_total(config)?.unwrap_or(0) + pending.len();
    Ok(AccountingCoverage {
        unreconciled_rewrites: pending.len(),
        lifetime_rewrites: lifetime,
        complete: pending.is_empty(),
        last_degraded_at_unix: pending.last().copied(),
    })
}

#[cfg(test)]
mod tests {
    use hzr_core::Config;
    use tempfile::tempdir;

    use hzr_exec::{CanonicalCommand, RewriteDecision, RewriteSource};

    use super::{
        clear_reconciled_rewrites, context_brief, degraded_rewrite_coverage,
        record_degraded_rewrite_at, steer_to_first_class,
    };

    fn allow_raw() -> RewriteDecision {
        RewriteDecision::allow_raw("no rule matched")
    }

    /// The plan was prepended to a subagent's prompt as a minified JSON blob with no
    /// explanation: no glossary, no statement that the paths are unverified leads, and no
    /// instruction on what to do with them. A subagent that cannot tell what the block is will
    /// either ignore it and re-derive everything or trust it as fact — and the second failure
    /// is worse, because a plan candidate is a ranked guess, not a finding.
    #[test]
    fn test_the_injected_brief_tells_a_subagent_what_the_evidence_is() {
        let plan = serde_json::json!({
            "pack": {
                "selected": [{
                    "source": "context",
                    "path": "crates/hzr-cli/src/hook_runner.rs",
                    "line_start": 45,
                    "line_end": 79,
                    "relevance": 0.42,
                }],
            },
        });

        let brief = context_brief(&plan);

        assert!(
            brief.contains("crates/hzr-cli/src/hook_runner.rs:45-79"),
            "a lead must be citable as a path and span, got: {brief}"
        );
        assert!(
            brief.contains("unverified"),
            "the brief must say the leads are unverified, got: {brief}"
        );
        assert!(
            !brief.contains("\"pack\""),
            "the raw envelope is noise for a subagent, got: {brief}"
        );
    }

    /// An empty plan must say so rather than prepending an empty block that reads like a
    /// finding of "nothing relevant exists".
    #[test]
    fn test_an_empty_plan_states_that_it_found_nothing() {
        let brief = context_brief(&serde_json::json!({"pack": {"selected": []}}));

        assert!(
            brief.contains("no"),
            "an empty plan must be stated, got: {brief}"
        );
        assert!(!brief.contains("unverified leads:"));
    }

    fn proposed(decision: &RewriteDecision) -> Option<String> {
        match decision {
            RewriteDecision::Ask {
                proposed: Some(CanonicalCommand::Shell { command, .. }),
                ..
            } => Some(command.clone()),
            _ => None,
        }
    }

    /// The behaviour that stops the leak: an agent reaching for `sed -n A,Bp` is shown the
    /// `hzr read` that does the same job for a fraction of the tokens, with the span
    /// already filled in.
    #[test]
    fn test_a_bypassed_read_is_answered_with_the_equivalent_hzr_command() {
        let decision = steer_to_first_class(
            "hzr rtk -- raw sed -n 1030,1105p crates/hzr-core/src/ledger.rs",
            allow_raw(),
        );

        assert_eq!(
            proposed(&decision).as_deref(),
            Some("hzr rtk -- read crates/hzr-core/src/ledger.rs --from 1030 --to 1105")
        );
    }

    #[test]
    fn test_a_bypassed_search_is_answered_with_hzr_search() {
        let decision = steer_to_first_class("rg -n RewriteDecision crates/hzr-exec", allow_raw());

        assert_eq!(
            proposed(&decision).as_deref(),
            Some("hzr search 'RewriteDecision' --mode exact --path crates/hzr-exec")
        );
    }

    /// `Ask`, never `Deny`: raw stays reachable in one keystroke because it is the correct
    /// tool for checksums, generated files and complete logs.
    #[test]
    fn test_steering_asks_rather_than_denying() {
        let decision = steer_to_first_class("cat README.md", allow_raw());

        assert!(matches!(decision, RewriteDecision::Ask { .. }));
    }

    #[test]
    fn test_a_command_without_an_equivalent_is_passed_through_untouched() {
        let decision = steer_to_first_class("cargo clippy --workspace", allow_raw());

        assert!(matches!(decision, RewriteDecision::AllowRaw { .. }));
    }

    /// When the pinned engine already rewrote the command there is nothing to steer, and
    /// overriding its decision would replace a filtered command with a guess.
    #[test]
    fn test_an_already_optimized_decision_is_never_overridden() {
        let rewritten = RewriteDecision::AllowRewrite {
            command: CanonicalCommand::shell("rtk read README.md"),
            reason: "fork-core rule".into(),
            source: RewriteSource::HzrPolicy,
        };

        let decision = steer_to_first_class("cat README.md", rewritten);

        assert!(matches!(decision, RewriteDecision::AllowRewrite { .. }));
    }

    fn config(root: &std::path::Path) -> Config {
        Config {
            data_dir: root.to_path_buf(),
            ..Config::default()
        }
    }

    #[test]
    fn test_a_clean_installation_reports_complete_coverage() {
        let directory = tempdir().expect("temp directory");
        let config = config(directory.path());

        let coverage = degraded_rewrite_coverage(&config).expect("coverage");

        assert!(coverage.complete);
        assert_eq!(coverage.unreconciled_rewrites, 0);
        assert_eq!(coverage.lifetime_rewrites, 0);
    }

    #[test]
    fn test_a_daemon_free_rewrite_marks_coverage_incomplete() {
        let directory = tempdir().expect("temp directory");
        let config = config(directory.path());

        record_degraded_rewrite_at(&config, 1_785_531_432).expect("record");
        record_degraded_rewrite_at(&config, 1_785_531_500).expect("record");
        let coverage = degraded_rewrite_coverage(&config).expect("coverage");

        assert!(!coverage.complete);
        assert_eq!(coverage.unreconciled_rewrites, 2);
        assert_eq!(coverage.lifetime_rewrites, 2);
    }

    /// The decisive behaviour: once the daemon serves a rewrite again the earlier gap is
    /// reconciled and coverage returns to complete. The lifetime count is kept, because
    /// erasing the history would be the dishonest fix.
    #[test]
    fn test_a_healthy_daemon_reconciles_the_gap_but_keeps_the_lifetime_count() {
        let directory = tempdir().expect("temp directory");
        let config = config(directory.path());
        record_degraded_rewrite_at(&config, 1_785_531_432).expect("record");
        record_degraded_rewrite_at(&config, 1_785_531_500).expect("record");

        clear_reconciled_rewrites(&config).expect("reconcile");
        let coverage = degraded_rewrite_coverage(&config).expect("coverage");

        assert!(coverage.complete, "a healthy daemon closes the gap");
        assert_eq!(coverage.unreconciled_rewrites, 0);
        assert_eq!(
            coverage.lifetime_rewrites, 2,
            "history is retained, only the open gap is closed"
        );
    }

    #[test]
    fn test_reconciling_twice_does_not_double_count_the_lifetime_total() {
        let directory = tempdir().expect("temp directory");
        let config = config(directory.path());
        record_degraded_rewrite_at(&config, 1_785_531_432).expect("record");

        clear_reconciled_rewrites(&config).expect("reconcile");
        clear_reconciled_rewrites(&config).expect("reconcile again");
        record_degraded_rewrite_at(&config, 1_785_600_000).expect("record");
        let coverage = degraded_rewrite_coverage(&config).expect("coverage");

        assert_eq!(coverage.unreconciled_rewrites, 1);
        assert_eq!(coverage.lifetime_rewrites, 2);
    }

    /// Reconciling when nothing is pending must not touch the filesystem: this runs on
    /// every successful managed rewrite, the hottest path HZR has.
    #[test]
    fn test_reconciling_a_clean_ledger_writes_nothing() {
        let directory = tempdir().expect("temp directory");
        let config = config(directory.path());

        clear_reconciled_rewrites(&config).expect("reconcile");

        assert!(
            !directory.path().join("ledger").exists(),
            "no gap means no bookkeeping"
        );
    }

    #[test]
    fn test_coverage_reports_when_the_last_gap_occurred() {
        let directory = tempdir().expect("temp directory");
        let config = config(directory.path());
        record_degraded_rewrite_at(&config, 1_785_531_432).expect("record");

        let coverage = degraded_rewrite_coverage(&config).expect("coverage");

        assert_eq!(coverage.last_degraded_at_unix, Some(1_785_531_432));
    }

    /// A log left behind by an earlier HZR contains bare timestamps and no lifetime file.
    /// It must still be readable rather than reported as a corrupt ledger.
    #[test]
    fn test_a_legacy_log_without_a_lifetime_file_is_still_counted() {
        let directory = tempdir().expect("temp directory");
        let config = config(directory.path());
        std::fs::create_dir_all(directory.path().join("ledger")).expect("ledger directory");
        std::fs::write(
            directory.path().join("ledger/degraded-rewrites.log"),
            "1785531432\n1785531500\n1785531600\n",
        )
        .expect("legacy log");

        let coverage = degraded_rewrite_coverage(&config).expect("coverage");

        assert_eq!(coverage.unreconciled_rewrites, 3);
        assert_eq!(coverage.lifetime_rewrites, 3);
    }
}
