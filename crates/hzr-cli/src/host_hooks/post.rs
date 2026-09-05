//! Claude built-in output replacement. Native command execution is never repeated.
use hzr_core::Config;
use hzr_protocol::{CommandTermination, ForkRunApiRequest};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::HookHost;
use crate::client::DaemonClient;

const MARKER: &str = "[HZR post-tool cargo-test v1";
const MIN_STDOUT_BYTES: usize = 4096;
const MAX_STDOUT_BYTES: usize = 128 * 1024;

/// A deliberately narrow successful-test grammar. Diagnostics, custom test output,
/// images, errors, unsupported schemas, and managed commands pass through unchanged.
fn candidate(host: HookHost, input: &Value) -> Option<&str> {
    if host != HookHost::Claude
        || input["hook_event_name"] != "PostToolUse"
        || input["tool_name"] != "Bash"
    {
        return None;
    }
    let command = input["tool_input"]["command"].as_str()?;
    let mut words = command.split_whitespace();
    if words.next() != Some("cargo") || words.next() != Some("test") {
        return None;
    }
    // No guessed shell interpretation or command chains.
    if command
        .chars()
        .any(|c| !c.is_ascii_alphanumeric() && !" \t-_=./:,".contains(c))
    {
        return None;
    }
    let response = input.get("tool_response")?.as_object()?;
    if response.get("interrupted")?.as_bool()? || response.get("isImage")?.as_bool()? {
        return None;
    }
    response.get("stderr")?.as_str()?;
    if response
        .get("exit_code")
        .is_some_and(|value| value.as_i64() != Some(0))
    {
        return None;
    }
    let stdout = response.get("stdout")?.as_str()?;
    if !(MIN_STDOUT_BYTES..=MAX_STDOUT_BYTES).contains(&stdout.len()) || stdout.contains(MARKER) {
        return None;
    }
    let mut summaries = 0;
    for line in stdout.lines().map(str::trim) {
        if line.is_empty() {
            continue;
        }
        if line.starts_with("test result: ok.") && line.contains("; 0 failed;") {
            summaries += 1;
        } else if line.starts_with("test ")
            && (line.ends_with(" ... ok") || line.ends_with(" ... ignored"))
        {
            continue;
        } else if let Some(count) = line
            .strip_prefix("running ")
            .and_then(|s| s.strip_suffix(" tests").or_else(|| s.strip_suffix(" test")))
        {
            count.parse::<u64>().ok()?;
        } else {
            return None;
        }
    }
    (summaries > 0).then_some(stdout)
}

fn replacement(
    input: &Value,
    stdout: &str,
    filtered: &str,
    recovery: &str,
    digest: &str,
) -> Option<Value> {
    let summaries = stdout
        .lines()
        .filter(|line| line.trim_start().starts_with("test result:"))
        .collect::<Vec<_>>()
        .join("\n");
    let shown = format!(
        "{MARKER}; transform={digest}; original stdout: {recovery}]\n{filtered}\nOriginal suite summaries:\n{summaries}\n"
    );
    let mut response = input["tool_response"].clone();
    response["stdout"] = json!(shown);
    let output =
        json!({"hookSpecificOutput":{"hookEventName":"PostToolUse","updatedToolOutput":response}});
    // Include the replacement envelope and recovery instructions in the break-even check.
    let original_bytes = serde_json::to_vec(&input["tool_response"]).ok()?.len();
    let replacement_bytes = serde_json::to_vec(&output).ok()?.len();
    (replacement_bytes.saturating_add(256) < original_bytes).then_some(output)
}

/// All failures preserve the successful host result. A returned value is a replacement
/// proposal, not host-delivery acknowledgement and never a provider-savings receipt.
pub async fn replace_tool_output(config: &Config, input: &Value, host: HookHost) -> Option<Value> {
    let stdout = candidate(host, input)?;
    let cwd = input["cwd"].as_str()?;
    let client = DaemonClient::from_config(config).ok()?;
    let result = client
        .fork_run(&ForkRunApiRequest {
            cwd: cwd.to_owned(),
            args: vec!["pipe".into(), "--filter".into(), "cargo-test".into()],
            stdin: Some(stdout.to_owned()),
            timeout_ms: Some(1000),
            agent: Some("claude-post-tool".into()),
            session_id: input["session_id"].as_str().map(str::to_owned),
            managed_write: None,
        })
        .await
        .ok()?;
    if result.termination != CommandTermination::Exited
        || result.exit_code != Some(0)
        || result.stdout_truncated
        || result.stderr_truncated
        || !result.stderr.is_empty()
        || result.stdout.trim().is_empty()
    {
        return None;
    }
    let digest = hex::encode(Sha256::digest(stdout.as_bytes()));
    let path = config
        .data_dir
        .join("hook-output")
        .join(format!("cargo-test-v1-{digest}.txt"));
    let path_text = path.to_str()?;
    let recovery = format!(
        "hzr read '{}' --level none",
        path_text.replace('\'', "'\\''")
    );
    let output = replacement(input, stdout, &result.stdout, &recovery, &digest)?;
    // Versioned content-addressed exact recovery is durable before replacing anything.
    persist_original(&path, stdout.as_bytes(), 128 * 1024 * 1024, 2048).ok()?;
    Some(output)
}

/// Refuse new replacements at capacity; advertised originals are never evicted.
fn persist_original(
    path: &std::path::Path,
    bytes: &[u8],
    max_bytes: u64,
    max_artifacts: usize,
) -> anyhow::Result<()> {
    use std::fs::{self, OpenOptions};
    use std::io::Write;

    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("recovery path has no parent"))?;
    let lock_path = parent.join(".quota.lock");
    crate::adoption::validate_lifecycle_target(&lock_path)?;
    fs::create_dir_all(parent)?;
    crate::adoption::validate_lifecycle_target(&lock_path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(nix::libc::O_NOFOLLOW);
    }
    let lock = options.open(&lock_path)?;
    anyhow::ensure!(
        lock.metadata()?.is_file(),
        "quota lock is not a regular file"
    );
    // 0.8.2: a sibling observer may hold the quota lock for a few milliseconds; wait briefly
    // instead of failing the recovery on the first EAGAIN. A lock held longer still refuses.
    let mut attempts = 0u32;
    loop {
        match fs2::FileExt::try_lock_exclusive(&lock) {
            Ok(()) => break,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock && attempts < 25 => {
                attempts += 1;
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(error) => return Err(error.into()),
        }
    }
    // Dropping the descriptor releases the process lock even on early return.
    crate::adoption::validate_lifecycle_target(path)?;
    match fs::symlink_metadata(path) {
        Ok(_) => {
            let stored = hzr_core::read_bounded_regular_file(path, MAX_STDOUT_BYTES as u64)?;
            anyhow::ensure!(stored == bytes, "content-addressed original is corrupted");
            return Ok(());
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let mut count = 0usize;
    let mut total = 0u64;
    for entry in fs::read_dir(parent)? {
        let entry = entry?;
        if entry.file_name() == ".quota.lock" {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path())?;
        anyhow::ensure!(
            metadata.is_file(),
            "recovery directory contains a non-regular entry"
        );
        count = count.saturating_add(1);
        total = total.saturating_add(metadata.len());
        anyhow::ensure!(
            count < max_artifacts && total.saturating_add(bytes.len() as u64) <= max_bytes,
            "recovery quota reached"
        );
    }
    anyhow::ensure!(
        count < max_artifacts && total.saturating_add(bytes.len() as u64) <= max_bytes,
        "recovery quota reached"
    );
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    temporary.as_file().sync_all()?;
    // Never overwrite an existing artifact, including a concurrently corrupted one.
    temporary
        .persist_noclobber(path)
        .map_err(|error| error.error)?;
    #[cfg(unix)]
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> Value {
        let stdout = format!(
            "running 180 tests\n{}\ntest result: ok. 180 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n",
            (0..180)
                .map(|n| format!("test module::case_{n} ... ok"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        json!({"hook_event_name":"PostToolUse","tool_name":"Bash","tool_input":{"command":"cargo test --lib"},
            "tool_response":{"stdout":stdout,"stderr":"compiler warning: preserve exactly\n","interrupted":false,"isImage":false,"extra":{"status":"preserved"}}})
    }

    #[test]
    fn replacement_preserves_shape_status_stderr_and_suite_totals() {
        let input = input();
        let raw = candidate(HookHost::Claude, &input).expect("supported fixture");
        let output = replacement(
            &input,
            raw,
            "cargo test: 180 passed",
            "hzr read '/private/recovery' --level none",
            "abc",
        )
        .expect("useful");
        let response = &output["hookSpecificOutput"]["updatedToolOutput"];
        for key in ["stderr", "interrupted", "isImage", "extra"] {
            assert_eq!(response[key], input["tool_response"][key]);
        }
        assert!(
            response["stdout"]
                .as_str()
                .expect("fixture value")
                .contains("test result: ok. 180 passed")
        );
        assert_eq!(output["hookSpecificOutput"]["hookEventName"], "PostToolUse");
        let mut replay = input.clone();
        replay["tool_response"] = response.clone();
        assert!(candidate(HookHost::Claude, &replay).is_none());
    }

    #[test]
    fn unsupported_or_sensitive_output_is_not_replaced() {
        let original = input();
        assert!(candidate(HookHost::Codex, &original).is_none());
        for (pointer, value) in [
            ("/tool_input/command", json!("hzr exec run 'cargo test'")),
            ("/tool_input/command", json!("cargo test; cat secret")),
            ("/hook_event_name", json!("PostToolUseFailure")),
            ("/tool_name", json!("Read")),
            ("/tool_response/interrupted", json!(true)),
            ("/tool_response/isImage", json!(true)),
            (
                "/tool_response/stdout",
                json!("test result: ok. 1 passed; 0 failed;"),
            ),
            ("/tool_response/stderr", json!({"changed":"shape"})),
        ] {
            let mut input = original.clone();
            *input.pointer_mut(pointer).expect("fixture value") = value;
            assert!(candidate(HookHost::Claude, &input).is_none(), "{pointer}");
        }
        let mut custom = original;
        let stdout = custom["tool_response"]["stdout"]
            .as_str()
            .expect("fixture value")
            .to_owned();
        custom["tool_response"]["stdout"] =
            json!(format!("{stdout}important custom test output\n"));
        assert!(candidate(HookHost::Claude, &custom).is_none());
    }

    #[test]
    fn original_recovery_is_quota_bounded_and_never_overwrites_corruption() {
        let directory = tempfile::tempdir().expect("directory");
        let first = directory.path().join("hook-output/first.txt");
        let second = directory.path().join("hook-output/second.txt");
        persist_original(&first, b"original", 8, 1).expect("first original");
        persist_original(&first, b"original", 8, 1).expect("content reuse at quota");
        assert!(persist_original(&second, b"new", 8, 1).is_err());
        assert!(!second.exists());
        assert!(
            persist_original(&second, b"new", 10, 4).is_err(),
            "byte quota"
        );
        std::fs::write(&first, b"corrupt").expect("corrupt fixture");
        assert!(persist_original(&first, b"original", 128, 4).is_err());
        assert_eq!(std::fs::read(&first).expect("original"), b"corrupt");
    }

    #[cfg(unix)]
    #[test]
    fn recovery_rejects_symlinks_and_uses_private_permissions() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let directory = tempfile::tempdir().expect("directory");
        let private = directory.path().join("private");
        let source = private.join("original.txt");
        persist_original(&source, b"exact", 100, 4).expect("private original");
        assert_eq!(
            std::fs::metadata(&private)
                .expect("directory")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&source)
                .expect("file")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let linked = directory.path().join("linked");
        symlink(&private, &linked).expect("directory symlink");
        assert!(persist_original(&linked.join("new.txt"), b"new", 100, 4).is_err());
        assert!(!private.join("new.txt").exists());
        symlink(&source, private.join("other.txt")).expect("file symlink");
        assert!(persist_original(&private.join("other.txt"), b"exact", 100, 4).is_err());
        assert_eq!(std::fs::read(source).expect("source"), b"exact");
    }

    #[test]
    fn busy_recovery_quota_lock_preserves_original_response() {
        let directory = tempfile::tempdir().expect("directory");
        let original = directory.path().join("original.txt");
        persist_original(&original, b"first", 100, 4).expect("original");
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(directory.path().join(".quota.lock"))
            .expect("lock");
        fs2::FileExt::lock_exclusive(&lock).expect("hold lock");
        let next = directory.path().join("next.txt");
        assert!(persist_original(&next, b"next", 100, 4).is_err());
        assert!(!next.exists());
    }

    #[test]
    fn envelope_overhead_must_break_even() {
        let input = input();
        let raw = candidate(HookHost::Claude, &input).expect("fixture value");
        assert!(replacement(&input, raw, raw, "recovery", "hash").is_none());
    }
}
