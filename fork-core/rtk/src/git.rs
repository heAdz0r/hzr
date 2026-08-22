use crate::args_utils;
use crate::fidelity::{self, FidelityReason};
use crate::tracking;
use anyhow::{Context, Result};
use chrono::{FixedOffset, Utc};
use std::collections::HashMap;
use std::ffi::OsString;
use std::process::Command;

const BLAME_EXACT_REASONS: &[FidelityReason] = &[FidelityReason::VerbatimSource];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitCommandClass {
    ReadOnly,
    Mutating,
}

#[derive(Debug, Clone)]
pub enum GitCommand {
    Diff,
    Log,
    Status,
    Show,
    Add,
    Commit { message: String },
    Checkout,
    Push,
    Pull,
    Branch,
    Fetch,
    Stash { subcommand: Option<String> },
    Worktree,
    Blame,
}

/// Create a git Command with global options prepended before subcommand args.
/// This ensures -C, -c, --git-dir, --work-tree, --no-pager etc. apply to every git invocation.
fn git_cmd(global_args: &[String]) -> Command {
    let mut cmd = Command::new("git");
    for arg in global_args {
        cmd.arg(arg);
    }
    cmd
}

/// Flags that pin git's diff rendering to a plain unified diff.
///
/// `git diff` output is user-configurable and we parse it. A `diff.external`
/// setting, `GIT_EXTERNAL_DIFF`, or a `.gitattributes` `diff=<driver>` attribute
/// replaces the diff wholesale (difftastic, delta, …); `textconv` drivers rewrite
/// file contents before diffing; `diff.noprefix` / `diff.mnemonicPrefix` change
/// the `a/` `b/` prefixes the filenames are read from. Any of them makes the
/// parse silently yield nothing while the surrounding stat summary still looks
/// authoritative.
///
/// These are subcommand flags, not git globals — `git --no-ext-diff status` is an
/// error — so they are applied per-subcommand and never in [`git_cmd`].
const DIFF_HARDENING: &[&str] = &[
    "--no-ext-diff",
    "--no-textconv",
    "--src-prefix=a/",
    "--dst-prefix=b/",
];

/// [`git_cmd`] plus a diff-producing subcommand and [`DIFF_HARDENING`].
///
/// Only for paths whose output we parse. Passthrough paths deliberately do not
/// use this: they print git's bytes verbatim and must keep honouring the user's
/// diff configuration. Hardening goes before the caller's own args, so an
/// explicit `--ext-diff` still wins — git is last-flag-wins.
fn git_diff_cmd(global_args: &[String], subcommand: &[&str]) -> Command {
    let mut cmd = git_cmd(global_args);
    cmd.args(subcommand);
    cmd.args(DIFF_HARDENING);
    cmd
}

pub fn run(
    cmd: GitCommand,
    args: &[String],
    max_lines: Option<usize>,
    verbose: u8,
    global_args: &[String],
) -> Result<()> {
    let command_class = classify_git_command(&cmd, args);
    if verbose > 2 {
        eprintln!("git command class: {:?}", command_class);
    }

    match cmd {
        GitCommand::Diff => run_diff(args, max_lines, verbose, global_args),
        GitCommand::Log => run_log(args, max_lines, verbose, global_args),
        GitCommand::Status => run_status(args, verbose, global_args),
        GitCommand::Show => run_show(args, max_lines, verbose, global_args),
        GitCommand::Add => run_add(args, verbose, global_args),
        GitCommand::Commit { message } => run_commit(&message, verbose, global_args),
        GitCommand::Checkout => run_checkout(args, verbose, global_args),
        GitCommand::Push => run_push(args, verbose, global_args),
        GitCommand::Pull => run_pull(args, verbose, global_args),
        GitCommand::Branch => run_branch(args, verbose, global_args),
        GitCommand::Fetch => run_fetch(args, verbose, global_args),
        GitCommand::Stash { subcommand } => {
            run_stash(subcommand.as_deref(), args, verbose, global_args)
        }
        GitCommand::Worktree => run_worktree(args, verbose, global_args),
        GitCommand::Blame => run_blame(args, verbose, global_args),
    }
}

fn is_branch_action(args: &[String]) -> bool {
    args.iter()
        .any(|a| a == "-d" || a == "-D" || a == "-m" || a == "-M" || a == "-c" || a == "-C")
}

fn is_stash_mutating(subcommand: Option<&str>) -> bool {
    match subcommand {
        Some("list") | Some("show") => false,
        Some("pop") | Some("apply") | Some("drop") | Some("push") | None => true,
        Some(_) => true,
    }
}

fn is_worktree_action(args: &[String]) -> bool {
    args.iter().any(|a| {
        a == "add" || a == "remove" || a == "prune" || a == "lock" || a == "unlock" || a == "move"
    })
}

pub(crate) fn classify_git_command(cmd: &GitCommand, args: &[String]) -> GitCommandClass {
    match cmd {
        GitCommand::Diff
        | GitCommand::Log
        | GitCommand::Status
        | GitCommand::Show
        | GitCommand::Blame => GitCommandClass::ReadOnly,
        GitCommand::Add
        | GitCommand::Commit { .. }
        | GitCommand::Checkout
        | GitCommand::Push
        | GitCommand::Pull
        | GitCommand::Fetch => GitCommandClass::Mutating,
        GitCommand::Branch => {
            if is_branch_action(args) {
                GitCommandClass::Mutating
            } else {
                GitCommandClass::ReadOnly
            }
        }
        GitCommand::Stash { subcommand } => {
            if is_stash_mutating(subcommand.as_deref()) {
                GitCommandClass::Mutating
            } else {
                GitCommandClass::ReadOnly
            }
        }
        GitCommand::Worktree => {
            if is_worktree_action(args) {
                GitCommandClass::Mutating
            } else {
                GitCommandClass::ReadOnly
            }
        }
    }
}

fn run_checkout(args: &[String], verbose: u8, global_args: &[String]) -> Result<()> {
    let timer = tracking::TimedExecution::start();
    if verbose > 0 {
        eprintln!("git checkout");
    }

    let mut cmd = git_cmd(global_args);
    cmd.arg("checkout").args(args);
    let output = cmd.output().context("Failed to run git checkout")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}{}", stdout, stderr);
    let exit_code = crate::stream::status_to_exit_code(output.status);
    let filtered = format_checkout_output(args, &raw, exit_code);
    let shown = crate::guard::never_worse(&raw, &filtered);
    println!("{}", shown);
    timer.track(
        &format!("git checkout {}", args.join(" ")),
        &format!("rtk git checkout {}", args.join(" ")),
        &raw,
        shown,
    );
    if exit_code != 0 {
        std::process::exit(exit_code);
    }
    Ok(())
}

fn format_checkout_output(args: &[String], raw: &str, exit_code: i32) -> String {
    if exit_code == 0 {
        format_checkout_success(args, raw)
    } else {
        filter_checkout_failure(raw)
    }
}

fn format_checkout_success(args: &[String], raw: &str) -> String {
    if let Some(restored) = checkout_restored_count(args) {
        return format!(
            "ok {} {}",
            restored,
            if restored == 1 {
                "file restored"
            } else {
                "files restored"
            }
        );
    }
    if let Some(branch) = checkout_reset_branch_arg(args) {
        return format!("ok {}", branch);
    }

    for line in raw.lines().map(str::trim) {
        if let Some(branch) = quoted_suffix(line, "Switched to a new branch ") {
            return format!("ok {} (new)", branch);
        }
        if let Some(branch) = quoted_suffix(line, "Switched to branch ") {
            return format!("ok {}", branch);
        }
        if let Some(branch) = quoted_suffix(line, "Already on ") {
            return format!("ok {}", branch);
        }
        if let Some(rest) = line.strip_prefix("HEAD is now at ") {
            return format!(
                "ok HEAD {}",
                rest.split_whitespace().next().unwrap_or("HEAD")
            );
        }
        if line.starts_with("Updated ") && line.contains(" path") {
            return format!("ok {}", line.to_ascii_lowercase());
        }
    }

    if let Some(branch) = checkout_new_branch_arg(args) {
        return format!("ok {} (new)", branch);
    }
    if let Some(branch) = checkout_branch_arg(args) {
        return format!("ok {}", branch);
    }
    "ok".to_string()
}

fn checkout_restored_count(args: &[String]) -> Option<usize> {
    let separator = args.iter().position(|arg| arg == "--")?;
    let count = args[separator + 1..]
        .iter()
        .filter(|arg| !arg.is_empty())
        .count();
    (count > 0).then_some(count)
}

fn checkout_new_branch_arg(args: &[String]) -> Option<&str> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-b" | "--orphan" => return iter.next().map(String::as_str),
            "-B" => {
                iter.next();
            }
            _ => {
                if let Some(branch) = arg.strip_prefix("--orphan=") {
                    return Some(branch);
                }
            }
        }
    }
    None
}

fn checkout_reset_branch_arg(args: &[String]) -> Option<&str> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "-B" {
            return iter.next().map(String::as_str);
        }
    }
    None
}

fn checkout_branch_arg(args: &[String]) -> Option<&str> {
    if args.iter().any(|arg| arg == "--") {
        return None;
    }
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "-b" | "-B" | "--orphan" => {
                iter.next();
            }
            "-t" | "--track" | "--detach" => {}
            _ if arg.starts_with('-') => {}
            _ => return Some(arg),
        }
    }
    None
}

fn quoted_suffix<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    line.strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix('\''))
        .and_then(|rest| rest.strip_suffix('\''))
}

fn filter_checkout_failure(raw: &str) -> String {
    let mut important = Vec::new();
    let mut in_file_list = false;
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let is_header = trimmed.starts_with("error:")
            || trimmed.starts_with("fatal:")
            || trimmed.starts_with("CONFLICT");
        if is_header {
            in_file_list = trimmed.contains("following")
                && trimmed.contains("files")
                && trimmed.ends_with(':');
            important.push(trimmed.to_string());
            continue;
        }
        if in_file_list {
            if trimmed.starts_with("Please ") || trimmed.starts_with("Aborting") {
                in_file_list = false;
            } else if line.starts_with(char::is_whitespace) {
                important.push(line.to_string());
                continue;
            }
        }
        if trimmed.starts_with("Aborting") {
            important.push(trimmed.to_string());
        }
    }
    if important.is_empty() {
        raw.trim().to_string()
    } else {
        important.join("\n")
    }
}

fn exit_with_git_failure(
    label: &str,
    stdout: &str,
    stderr: &str,
    status: std::process::ExitStatus,
) -> ! {
    let exit_code = crate::stream::status_to_exit_code(status);
    eprintln!("FAILED: {}", label);
    let echoed = !stderr.trim().is_empty() || !stdout.trim().is_empty();
    if !stderr.trim().is_empty() {
        eprintln!("{}", stderr);
    }
    if !stdout.trim().is_empty() {
        eprintln!("{}", stdout);
    }
    // Restate the verdict after the child's own output. `pre-commit` prints a
    // result line per hook, so an aborted commit routinely *ended* on
    // "...............Passed" — a reader taking the last line concluded the
    // commit succeeded.
    if echoed {
        eprintln!("{}: failed (exit {})", label, exit_code);
    }
    std::process::exit(exit_code);
}

fn run_diff(
    args: &[String],
    max_lines: Option<usize>,
    verbose: u8,
    global_args: &[String],
) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    // Check if user wants stat output
    let wants_stat = args
        .iter()
        .any(|arg| arg == "--stat" || arg == "--numstat" || arg == "--shortstat");

    // Check if user wants compact diff (default RTK behavior)
    let wants_compact = !args.iter().any(|arg| arg == "--no-compact");

    if wants_stat || !wants_compact {
        // User wants stat or explicitly no compacting - pass through directly
        let mut cmd = git_cmd(global_args);
        cmd.arg("diff");
        for arg in args {
            cmd.arg(arg);
        }

        let output = cmd.output().context("Failed to run git diff")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!("{}", stderr);
            std::process::exit(output.status.code().unwrap_or(1));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        println!("{}", stdout.trim());

        timer.track(
            &format!("git diff {}", args.join(" ")),
            &format!("rtk git diff {} (passthrough)", args.join(" ")),
            &stdout,
            &stdout,
        );

        return Ok(());
    }

    // Default RTK behavior: stat first, then compacted diff
    let mut cmd = git_diff_cmd(global_args, &["diff", "--stat"]);

    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run git diff")?;
    let stat_stdout = String::from_utf8_lossy(&output.stdout);

    if verbose > 0 {
        eprintln!("Git diff summary:");
    }

    // Now get actual diff but compact it
    let mut diff_cmd = git_diff_cmd(global_args, &["diff"]);
    for arg in args {
        diff_cmd.arg(arg);
    }

    let diff_output = diff_cmd.output().context("Failed to run git diff")?;
    let diff_stdout = String::from_utf8_lossy(&diff_output.stdout);

    let printed = if !diff_stdout.is_empty() {
        let compacted = compact_diff(&diff_stdout, max_lines.unwrap_or(100));
        format!("{}\n\n--- Changes ---\n{}", stat_stdout.trim(), compacted)
    } else {
        stat_stdout.trim().to_string()
    };

    let raw = format!("{}\n{}", stat_stdout, diff_stdout);
    let shown = crate::guard::never_worse(&raw, &printed);
    println!("{}", shown);

    timer.track(
        &format!("git diff {}", args.join(" ")),
        &format!("rtk git diff {}", args.join(" ")),
        &raw,
        shown,
    );

    Ok(())
}

fn run_show(
    args: &[String],
    max_lines: Option<usize>,
    verbose: u8,
    global_args: &[String],
) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    // If user wants --stat or --format only, pass through
    let wants_stat_only = args
        .iter()
        .any(|arg| arg == "--stat" || arg == "--numstat" || arg == "--shortstat");

    let wants_format = args
        .iter()
        .any(|arg| arg.starts_with("--pretty") || arg.starts_with("--format"));

    // fix #248: `git show rev:path` prints a blob, not a commit diff — pass through directly
    let wants_blob_show = args.iter().any(|arg| is_blob_show_arg(arg));

    if wants_stat_only || wants_format || wants_blob_show {
        let mut cmd = git_cmd(global_args);
        cmd.arg("show");
        for arg in args {
            cmd.arg(arg);
        }
        let output = cmd.output().context("Failed to run git show")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!("{}", stderr);
            std::process::exit(output.status.code().unwrap_or(1));
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        if wants_blob_show {
            print!("{}", stdout); // fix #248: no trim — preserve trailing newlines exactly
        } else {
            println!("{}", stdout.trim());
        }

        timer.track(
            &format!("git show {}", args.join(" ")),
            &format!("rtk git show {} (passthrough)", args.join(" ")),
            &stdout,
            &stdout,
        );

        return Ok(());
    }

    // Get raw output for tracking. Hardened as well, so the tracked savings
    // baseline is the diff we actually parse rather than an external driver's
    // rendering of it.
    let mut raw_cmd = git_diff_cmd(global_args, &["show"]);
    for arg in args {
        raw_cmd.arg(arg);
    }
    let raw_output = raw_cmd
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();

    // Step 1: one-line commit summary
    let mut summary_cmd = git_cmd(global_args);
    summary_cmd.args(["show", "--no-patch", "--pretty=format:%h %s (%ar) <%an>"]);
    for arg in args {
        summary_cmd.arg(arg);
    }
    let summary_output = summary_cmd.output().context("Failed to run git show")?;
    if !summary_output.status.success() {
        let stderr = String::from_utf8_lossy(&summary_output.stderr);
        eprintln!("{}", stderr);
        std::process::exit(summary_output.status.code().unwrap_or(1));
    }
    let summary = String::from_utf8_lossy(&summary_output.stdout);
    let mut printed = summary.trim().to_string();

    // Step 2: --stat summary
    let mut stat_cmd = git_cmd(global_args);
    stat_cmd.args(["show", "--stat", "--pretty=format:"]);
    for arg in args {
        stat_cmd.arg(arg);
    }
    let stat_output = stat_cmd.output().context("Failed to run git show --stat")?;
    let stat_stdout = String::from_utf8_lossy(&stat_output.stdout);
    let stat_text = stat_stdout.trim();
    if !stat_text.is_empty() {
        printed.push('\n');
        printed.push_str(stat_text);
    }

    // Step 3: compacted diff
    let mut diff_cmd = git_cmd(global_args);
    diff_cmd.args(["show", "--pretty=format:"]);
    for arg in args {
        diff_cmd.arg(arg);
    }
    let diff_output = diff_cmd.output().context("Failed to run git show (diff)")?;
    let diff_stdout = String::from_utf8_lossy(&diff_output.stdout);
    let diff_text = diff_stdout.trim();

    if !diff_text.is_empty() {
        if verbose > 0 {
            printed.push_str("\n\n--- Changes ---");
        }
        let compacted = compact_diff(diff_text, max_lines.unwrap_or(100));
        printed.push('\n');
        printed.push_str(&compacted);
    }

    let shown = crate::guard::never_worse(&raw_output, &printed);
    println!("{}", shown);

    timer.track(
        &format!("git show {}", args.join(" ")),
        &format!("rtk git show {}", args.join(" ")),
        &raw_output,
        shown,
    );

    Ok(())
}

/// fix #248: detect `rev:path` style arguments while ignoring flags like `--pretty=format:...`
fn is_blob_show_arg(arg: &str) -> bool {
    !arg.starts_with('-') && arg.contains(':')
}

pub(crate) fn compact_diff(diff: &str, max_lines: usize) -> String {
    let mut result = Vec::new();
    let mut current_file = String::new();
    let mut added = 0;
    let mut removed = 0;
    let mut in_hunk = false;
    let mut hunk_lines = 0;
    let max_hunk_lines = 10;

    for line in diff.lines() {
        if line.starts_with("diff --git") {
            // New file
            if !current_file.is_empty() && (added > 0 || removed > 0) {
                result.push(format!("  +{} -{}", added, removed));
            }
            current_file = line.split(" b/").nth(1).unwrap_or("unknown").to_string();
            result.push(format!("\n📄 {}", current_file));
            added = 0;
            removed = 0;
            in_hunk = false;
        } else if line.starts_with("@@") {
            // New hunk
            in_hunk = true;
            hunk_lines = 0;
            let hunk_info = line.split("@@").nth(1).unwrap_or("").trim();
            result.push(format!("  @@ {} @@", hunk_info));
        } else if in_hunk {
            if line.starts_with('+') && !line.starts_with("+++") {
                added += 1;
                if hunk_lines < max_hunk_lines {
                    result.push(format!("  {}", line));
                    hunk_lines += 1;
                }
            } else if line.starts_with('-') && !line.starts_with("---") {
                removed += 1;
                if hunk_lines < max_hunk_lines {
                    result.push(format!("  {}", line));
                    hunk_lines += 1;
                }
            } else if hunk_lines < max_hunk_lines && !line.starts_with("\\") {
                // Context line
                if hunk_lines > 0 {
                    result.push(format!("  {}", line));
                    hunk_lines += 1;
                }
            }

            if hunk_lines == max_hunk_lines {
                result.push("  ... (truncated)".to_string());
                hunk_lines += 1;
            }
        }

        if result.len() >= max_lines {
            result.push("\n... (more changes truncated)".to_string());
            break;
        }
    }

    if !current_file.is_empty() && (added > 0 || removed > 0) {
        result.push(format!("  +{} -{}", added, removed));
    }

    // Nothing was ever pushed — no `diff --git`, no `@@` — so this is not a
    // unified diff. Returning the empty join would drop the whole diff while the
    // stat summary around it still reads as authoritative. Fall back to the raw
    // text and let `never_worse` at the call site pick the cheaper of the two.
    if result.is_empty() && !diff.trim().is_empty() {
        return diff.to_string();
    }

    result.join("\n")
}

fn run_log(
    args: &[String],
    _max_lines: Option<usize>,
    verbose: u8,
    global_args: &[String],
) -> Result<()> {
    // Clap's trailing_var_arg swallows the literal `--`. Without it back,
    // `git log -- -p` loses its pathspec separator and `-p` is misread as the
    // patch flag rather than as a path literally named `-p`.
    let args = &args_utils::restore_double_dash(args);

    // A request for patch/diffstat/name-list output changes the *shape* of git's
    // own output, which the injected `--pretty=format` and the line cap below
    // cannot coexist with. Hand the whole invocation to git untouched instead of
    // reformatting a diff into a commit list.
    if requests_raw_log_output(args) {
        let passthrough: Vec<OsString> = std::iter::once(OsString::from("log"))
            .chain(args.iter().map(OsString::from))
            .collect();
        return run_passthrough(&passthrough, verbose, global_args);
    }

    let timer = tracking::TimedExecution::start();

    let mut cmd = git_cmd(global_args);
    cmd.arg("log");

    // Tokenize once and share it: a value belonging to --grep/--author/etc.
    // (`git log --grep --pretty`) must not be read as one of the flags below,
    // and the limit parser needs the same classification.
    let tokens = log_arg_tokens(args);
    let flag_args = flag_args_from_tokens(&tokens);

    // Check if user provided format flags
    let has_format_flag = flag_args.iter().any(|arg| {
        arg.starts_with("--oneline") || arg.starts_with("--pretty") || arg.starts_with("--format")
    });

    // Check if user provided a limit flag (-N, -n N, --max-count=N, --max-count N)
    let has_limit_flag = flag_args.iter().any(|arg| {
        (arg.starts_with('-') && arg.chars().nth(1).is_some_and(|c| c.is_ascii_digit()))
            || *arg == "-n"
            || arg.starts_with("--max-count")
    });

    // Apply RTK defaults only if user didn't specify them
    if !has_format_flag {
        cmd.args(["--pretty=format:%h %s (%ar) <%an>"]);
    }

    let limit = if !has_limit_flag {
        cmd.arg("-10");
        10
    } else {
        parse_limit_from_tokens(&tokens).unwrap_or(10)
    };

    // Only add --no-merges if user didn't explicitly request merge commits
    let wants_merges = flag_args
        .iter()
        .any(|arg| *arg == "--merges" || *arg == "--min-parents=2" || *arg == "--no-merges");
    if !wants_merges {
        cmd.arg("--no-merges");
    }

    // Pass all user arguments
    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run git log")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("{}", stderr);
        // Propagate git's exit code
        std::process::exit(output.status.code().unwrap_or(1));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    if verbose > 0 {
        eprintln!("Git log output:");
    }

    // Post-process: truncate long messages, cap lines
    let filtered = filter_log_output(&stdout, limit);
    let filtered = crate::guard::never_worse(&stdout, &filtered).to_string();
    println!("{}", filtered);

    timer.track(
        &format!("git log {}", args.join(" ")),
        &format!("rtk git log {}", args.join(" ")),
        &stdout,
        &filtered,
    );

    Ok(())
}

/// True for git log/diff options that take their value as a separate,
/// space-delimited token. `git log --grep -p` searches messages for the literal
/// string "-p"; it does not request patch output. Consuming that value token
/// keeps flag-lookalike values from being read as the flag they resemble.
///
/// Options whose value is *attached* (`-U5`, `--expand-tabs=4`, `--max-parents=1`)
/// are deliberately absent: they consume nothing that follows them.
fn consumes_next_token_as_value(arg: &str) -> bool {
    matches!(
        arg,
        "--after"
            | "--anchored"
            | "--author"
            | "--before"
            | "--color-moved-ws"
            | "--committer"
            | "--date"
            | "--decorate-refs"
            | "--decorate-refs-exclude"
            | "--diff-algorithm"
            | "--diff-filter"
            | "--diff-merges"
            | "--dst-prefix"
            | "--encoding"
            | "--exclude"
            | "--find-object"
            | "--glob"
            | "--grep"
            | "--grep-reflog"
            | "--inter-hunk-context"
            | "--line-prefix"
            | "--max-depth"
            | "--output"
            | "--output-indicator-context"
            | "--output-indicator-new"
            | "--output-indicator-old"
            | "--rotate-to"
            | "--since"
            | "--since-as-filter"
            | "--skip"
            | "--skip-to"
            | "--src-prefix"
            | "--stat-count"
            | "--stat-name-width"
            | "--stat-width"
            | "--until"
            | "--word-diff-regex"
            | "--ws-error-highlight"
            | "-G"
            | "-I"
            | "-L"
            | "-O"
            | "-S"
            | "-l"
            | "-n"
    )
}

/// A git log argument, classified as either a flag or the value consumed by the
/// preceding flag.
enum LogArg<'a> {
    Flag(&'a str),
    Value { flag: &'a str, value: &'a str },
}

/// Tokenize git log `args`, stopping at the `--` pathspec separator: tokens
/// after it are paths, never flags — `git log -- -5` means "history for the path
/// literally named -5". Shared by every flag, limit and shape check in
/// [`run_log`] so `--` handling and option-value handling live in one place.
fn log_arg_tokens(args: &[String]) -> Vec<LogArg<'_>> {
    let mut tokens = Vec::with_capacity(args.len());
    let mut iter = args.iter().take_while(|arg| *arg != "--");
    while let Some(arg) = iter.next() {
        let arg_str = arg.as_str();
        if arg_str == "--max-count" || consumes_next_token_as_value(arg_str) {
            if let Some(value) = iter.next() {
                tokens.push(LogArg::Value {
                    flag: arg_str,
                    value: value.as_str(),
                });
                continue;
            }
        }
        tokens.push(LogArg::Flag(arg_str));
    }
    tokens
}

/// Filter `tokens` down to the flags themselves, dropping every value consumed
/// by the preceding option.
fn flag_args_from_tokens<'a>(tokens: &[LogArg<'a>]) -> Vec<&'a str> {
    tokens
        .iter()
        .map(|token| match token {
            LogArg::Flag(flag) | LogArg::Value { flag, .. } => *flag,
        })
        .collect()
}

/// Convenience wrapper over [`log_arg_tokens`] for tests that only care which
/// tokens are real flags; `run_log` shares one tokenization instead.
#[cfg(test)]
fn real_flag_args(args: &[String]) -> Vec<&str> {
    flag_args_from_tokens(&log_arg_tokens(args))
}

/// True for flags that change the *shape* of git's raw output — patch text,
/// diffstat, name lists. The injected `--pretty=format` and the line cap in
/// [`filter_log_output`] cannot coexist with any of them, so matching one must
/// take the untouched passthrough path.
fn requests_raw_diff_shape(flag: &str) -> bool {
    matches!(
        flag,
        "-p" | "-u"
            | "--dirstat"
            | "--name-only"
            | "--name-status"
            | "--numstat"
            | "--patch"
            | "--patch-with-raw"
            | "--patch-with-stat"
            | "--raw"
            | "--shortstat"
            | "--stat"
            | "--summary"
    ) || flag.starts_with("--stat=")
        || flag.starts_with("--dirstat=")
}

fn requests_raw_log_output(args: &[String]) -> bool {
    log_arg_tokens(args)
        .iter()
        .any(|token| matches!(token, LogArg::Flag(flag) if requests_raw_diff_shape(flag)))
}

/// Parse the user-specified limit: `-20`, `-n 20`, `--max-count=20`,
/// `--max-count 20`.
#[cfg(test)]
fn parse_user_limit(args: &[String]) -> Option<usize> {
    parse_limit_from_tokens(&log_arg_tokens(args))
}

fn parse_limit_from_tokens(tokens: &[LogArg<'_>]) -> Option<usize> {
    for token in tokens {
        match token {
            // -20 (combined digit form)
            LogArg::Flag(flag)
                if flag.starts_with('-')
                    && flag.len() > 1
                    && flag.chars().nth(1).is_some_and(|c| c.is_ascii_digit()) =>
            {
                if let Ok(n) = flag[1..].parse::<usize>() {
                    return Some(n);
                }
            }
            // -n 20 / --max-count 20 (two-token form)
            LogArg::Value {
                flag: "-n" | "--max-count",
                value,
            } => {
                if let Ok(n) = value.parse::<usize>() {
                    return Some(n);
                }
            }
            // --max-count=20
            LogArg::Flag(flag) => {
                if let Some(rest) = flag.strip_prefix("--max-count=") {
                    if let Ok(n) = rest.parse::<usize>() {
                        return Some(n);
                    }
                }
            }
            LogArg::Value { .. } => {}
        }
    }
    None
}

/// Filter git log output: truncate long messages, cap lines
pub(crate) fn filter_log_output(output: &str, limit: usize) -> String {
    let lines: Vec<&str> = output.lines().collect();
    let capped: Vec<String> = lines
        .iter()
        .take(limit)
        .map(|line| {
            if line.len() > 80 {
                let truncated: String = line.chars().take(77).collect();
                format!("{}...", truncated)
            } else {
                line.to_string()
            }
        })
        .collect();

    capped.join("\n").trim().to_string()
}

// upstream v0.39: extract in-progress state from plain git status output
fn extract_state_header(raw: &str) -> Option<String> {
    const ANCHORS: &[&str] = &[
        "rebase in progress",
        "You are currently rebasing",
        "You are currently editing",
        "You are currently splitting",
        "You are currently cherry-picking",
        "You are currently reverting",
        "You are currently bisecting",
        "You are in the middle of",
        "You are in a sparse checkout",
        "All conflicts fixed but you are still merging",
        "You have unmerged paths",
        "Last command done",
        "Next command to do",
        "No commands remaining",
    ];

    const STOPPERS: &[&str] = &[
        "Changes to be committed:",
        "Changes not staged for commit:",
        "Untracked files:",
        "Unmerged paths:",
        "no changes added to commit",
        "nothing to commit",
        "nothing added to commit",
    ];

    let mut found = false;
    let mut out: Vec<String> = Vec::new();

    for line in raw.lines() {
        let trimmed = line.trim_end();
        let stripped = trimmed.trim_start();

        if STOPPERS.iter().any(|s| stripped.starts_with(s)) {
            break;
        }

        if stripped.starts_with("On branch ")
            || stripped.starts_with("HEAD detached")
            || stripped.starts_with("Your branch ")
        {
            continue;
        }

        if stripped.starts_with("(use \"git add") || stripped.starts_with("(use \"git restore") {
            continue;
        }

        if !found && ANCHORS.iter().any(|a| stripped.contains(a)) {
            found = true;
        }

        if found {
            out.push(trimmed.to_string());
        }
    }

    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }

    if out.is_empty() {
        None
    } else {
        Some(format!("⚡ {}", out.join("\n⚡ ")))
    }
}

/// Format porcelain output into compact RTK status display
pub(crate) fn format_status_output(porcelain: &str) -> String {
    let lines: Vec<&str> = porcelain.lines().collect();

    if lines.is_empty() {
        return "Clean working tree".to_string();
    }

    let mut output = String::new();

    // Parse branch info
    if let Some(branch_line) = lines.first() {
        if branch_line.starts_with("##") {
            let branch = branch_line.trim_start_matches("## ");
            output.push_str(&format!("📌 {}\n", branch));
        }
    }

    // Count changes by type
    let mut staged = 0;
    let mut modified = 0;
    let mut untracked = 0;
    let mut conflicts = 0;

    let mut staged_files = Vec::new();
    let mut modified_files = Vec::new();
    let mut untracked_files = Vec::new();

    for line in lines.iter().skip(1) {
        if line.len() < 3 {
            continue;
        }
        let status = line.get(0..2).unwrap_or("  ");
        let file = line.get(3..).unwrap_or("");

        match status.chars().next().unwrap_or(' ') {
            'M' | 'A' | 'D' | 'R' | 'C' => {
                staged += 1;
                staged_files.push(file);
            }
            'U' => conflicts += 1,
            _ => {}
        }

        match status.chars().nth(1).unwrap_or(' ') {
            'M' | 'D' => {
                modified += 1;
                modified_files.push(file);
            }
            _ => {}
        }

        if status == "??" {
            untracked += 1;
            untracked_files.push(file);
        }
    }

    // Build summary
    if staged > 0 {
        output.push_str(&format!("✅ Staged: {} files\n", staged));
        for f in staged_files.iter().take(5) {
            output.push_str(&format!("   {}\n", f));
        }
        if staged_files.len() > 5 {
            output.push_str(&format!("   ... +{} more\n", staged_files.len() - 5));
        }
    }

    if modified > 0 {
        output.push_str(&format!("📝 Modified: {} files\n", modified));
        for f in modified_files.iter().take(5) {
            output.push_str(&format!("   {}\n", f));
        }
        if modified_files.len() > 5 {
            output.push_str(&format!("   ... +{} more\n", modified_files.len() - 5));
        }
    }

    if untracked > 0 {
        output.push_str(&format!("❓ Untracked: {} files\n", untracked));
        for f in untracked_files.iter().take(3) {
            output.push_str(&format!("   {}\n", f));
        }
        if untracked_files.len() > 3 {
            output.push_str(&format!("   ... +{} more\n", untracked_files.len() - 3));
        }
    }

    if conflicts > 0 {
        output.push_str(&format!("⚠️  Conflicts: {} files\n", conflicts));
    }

    output.trim_end().to_string()
}

/// Minimal filtering for git status with user-provided args
fn filter_status_with_args(output: &str) -> String {
    let mut result = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();

        // Skip empty lines
        if trimmed.is_empty() {
            continue;
        }

        // Skip git hints - can appear at start or within line
        if trimmed.starts_with("(use \"git")
            || trimmed.starts_with("(create/copy files")
            || trimmed.contains("(use \"git add")
            || trimmed.contains("(use \"git restore")
        {
            continue;
        }

        // Special case: clean working tree
        if trimmed.contains("nothing to commit") && trimmed.contains("working tree clean") {
            result.push(trimmed.to_string());
            break;
        }

        result.push(line.to_string());
    }

    if result.is_empty() {
        "ok ✓".to_string()
    } else {
        result.join("\n")
    }
}

fn run_status(args: &[String], verbose: u8, global_args: &[String]) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    // If user provided flags, apply minimal filtering
    if !args.is_empty() {
        let output = git_cmd(global_args)
            .arg("status")
            .args(args)
            .output()
            .context("Failed to run git status")?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if !output.status.success() {
            timer.track(
                &format!("git status {}", args.join(" ")),
                &format!("rtk git status {}", args.join(" ")),
                &format!("{}{}", stdout, stderr),
                "FAILED",
            );
            exit_with_git_failure("git status", &stdout, &stderr, output.status);
        }

        if verbose > 0 || !stderr.is_empty() {
            eprint!("{}", stderr);
        }

        // Apply minimal filtering: strip ANSI, remove hints, empty lines
        let filtered = filter_status_with_args(&stdout);
        let filtered = crate::guard::never_worse(&stdout, &filtered).to_string();
        print!("{}", filtered);

        timer.track(
            &format!("git status {}", args.join(" ")),
            &format!("rtk git status {}", args.join(" ")),
            &stdout,
            &filtered,
        );

        return Ok(());
    }

    // Default RTK compact mode (no args provided)
    // Get raw git status for tracking
    let raw_output = git_cmd(global_args)
        .args(["status"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();

    let output = git_cmd(global_args)
        .args(["status", "--porcelain", "-b"])
        .output()
        .context("Failed to run git status")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        let message = if stderr.contains("not a git repository") {
            "Not a git repository"
        } else {
            stderr.trim()
        };
        if !message.is_empty() {
            eprintln!("{}", message);
        }
        timer.track("git status", "rtk git status", &raw_output, message);
        std::process::exit(output.status.code().unwrap_or(1));
    }

    let mut formatted = format_status_output(&stdout);

    // upstream v0.39: surface in-progress state (rebase/merge/cherry-pick/bisect)
    if let Some(state) = extract_state_header(&raw_output) {
        formatted = format!("{}\n{}", state, formatted);
    }

    let shown = crate::guard::never_worse(&raw_output, &formatted);
    println!("{}", shown);

    // Track for statistics
    timer.track("git status", "rtk git status", &raw_output, shown);

    return Ok(());
}

fn run_add(args: &[String], verbose: u8, global_args: &[String]) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = git_cmd(global_args);
    cmd.arg("add");

    // Pass all arguments directly to git (flags like -A, -p, --all, etc.)
    if args.is_empty() {
        cmd.arg(".");
    } else {
        for arg in args {
            cmd.arg(arg);
        }
    }

    let output = cmd.output().context("Failed to run git add")?;

    if verbose > 0 {
        eprintln!("git add executed");
    }

    let raw_output = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    if output.status.success() {
        // Count what was added
        let status_output = git_cmd(global_args)
            .args(["diff", "--cached", "--stat", "--shortstat"])
            .output()
            .context("Failed to check staged files")?;

        let stat = String::from_utf8_lossy(&status_output.stdout);
        let compact = if stat.trim().is_empty() {
            "ok (nothing to add)".to_string()
        } else {
            // Parse "1 file changed, 5 insertions(+)" format
            let short = stat.lines().last().unwrap_or("").trim();
            if short.is_empty() {
                "ok ✓".to_string()
            } else {
                format!("ok ✓ {}", short)
            }
        };

        println!("{}", compact);

        timer.track(
            &format!("git add {}", args.join(" ")),
            &format!("rtk git add {}", args.join(" ")),
            &raw_output,
            &compact,
        );
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        eprintln!("FAILED: git add");
        if !stderr.trim().is_empty() {
            eprintln!("{}", stderr);
        }
        if !stdout.trim().is_empty() {
            eprintln!("{}", stdout);
        }
        // Propagate git's exit code
        std::process::exit(output.status.code().unwrap_or(1));
    }

    Ok(())
}

fn run_commit(message: &str, verbose: u8, global_args: &[String]) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("git commit -m \"{}\"", message);
    }

    let output = git_cmd(global_args)
        .args(["commit", "-m", message])
        .output()
        .context("Failed to run git commit")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw_output = format!("{}\n{}", stdout, stderr);

    if output.status.success() {
        // The hash is the last whitespace-separated token inside the brackets.
        let compact = if let Some(line) = stdout.lines().next() {
            if let Some(bracket_end) = line.find(']') {
                let bracket_content = &line[1..bracket_end];
                let hash = bracket_content.split_whitespace().next_back().unwrap_or("");
                if !hash.is_empty() && hash.len() >= 7 {
                    let short_hash: String = hash.chars().take(7).collect();
                    format!("ok ✓ {}", short_hash)
                } else {
                    "ok ✓".to_string()
                }
            } else {
                "ok ✓".to_string()
            }
        } else {
            "ok ✓".to_string()
        };

        println!("{}", compact);

        timer.track(
            &format!("git commit -m \"{}\"", message),
            "rtk git commit",
            &raw_output,
            &compact,
        );
    } else {
        timer.track(
            &format!("git commit -m \"{}\"", message),
            "rtk git commit",
            &raw_output,
            "FAILED",
        );
        exit_with_git_failure("git commit", &stdout, &stderr, output.status);
    }

    Ok(())
}

fn run_push(args: &[String], verbose: u8, global_args: &[String]) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("git push");
    }

    let mut cmd = git_cmd(global_args);
    cmd.arg("push");
    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run git push")?;

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let raw = format!("{}{}", stdout, stderr);

    // upstream v0.41: noise-skipping prefixes for git push progress lines
    const GIT_PUSH_NOISE_PREFIXES: &[&str] = &[
        "Enumerating objects:",
        "Counting objects:",
        "Compressing objects:",
        "Writing objects:",
        "Delta compression using",
        "Total ",
    ];

    if output.status.success() {
        let compact = if stderr.contains("Everything up-to-date") {
            "ok (up-to-date)".to_string()
        } else {
            let mut result = String::new();
            // upstream v0.41: filter noise lines before extracting ref info
            for line in stderr.lines() {
                let trimmed = line.trim();
                if GIT_PUSH_NOISE_PREFIXES
                    .iter()
                    .any(|p| trimmed.starts_with(p))
                {
                    continue;
                }
                if line.contains("->") {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 3 {
                        result = format!("ok {}", parts[parts.len() - 1]);
                        break;
                    }
                }
            }
            if !result.is_empty() {
                result
            } else {
                "ok".to_string()
            }
        };

        println!("{}", compact);

        timer.track(
            &format!("git push {}", args.join(" ")),
            &format!("rtk git push {}", args.join(" ")),
            &raw,
            &compact,
        );
    } else {
        timer.track(
            &format!("git push {}", args.join(" ")),
            &format!("rtk git push {}", args.join(" ")),
            &raw,
            "FAILED",
        );
        exit_with_git_failure("git push", &stdout, &stderr, output.status);
    }

    Ok(())
}

fn run_pull(args: &[String], verbose: u8, global_args: &[String]) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("git pull");
    }

    let mut cmd = git_cmd(global_args);
    cmd.arg("pull");
    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run git pull")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw_output = format!("{}\n{}", stdout, stderr);

    if output.status.success() {
        let compact =
            if stdout.contains("Already up to date") || stdout.contains("Already up-to-date") {
                "ok (up-to-date)".to_string()
            } else {
                // Count files changed
                let mut files = 0;
                let mut insertions = 0;
                let mut deletions = 0;

                for line in stdout.lines() {
                    if line.contains("file") && line.contains("changed") {
                        // Parse "3 files changed, 10 insertions(+), 2 deletions(-)"
                        for part in line.split(',') {
                            let part = part.trim();
                            if part.contains("file") {
                                files = part
                                    .split_whitespace()
                                    .next()
                                    .and_then(|n| n.parse().ok())
                                    .unwrap_or(0);
                            } else if part.contains("insertion") {
                                insertions = part
                                    .split_whitespace()
                                    .next()
                                    .and_then(|n| n.parse().ok())
                                    .unwrap_or(0);
                            } else if part.contains("deletion") {
                                deletions = part
                                    .split_whitespace()
                                    .next()
                                    .and_then(|n| n.parse().ok())
                                    .unwrap_or(0);
                            }
                        }
                    }
                }

                if files > 0 {
                    format!("ok ✓ {} files +{} -{}", files, insertions, deletions)
                } else {
                    "ok ✓".to_string()
                }
            };

        println!("{}", compact);

        timer.track(
            &format!("git pull {}", args.join(" ")),
            &format!("rtk git pull {}", args.join(" ")),
            &raw_output,
            &compact,
        );
    } else {
        timer.track(
            &format!("git pull {}", args.join(" ")),
            &format!("rtk git pull {}", args.join(" ")),
            &raw_output,
            "FAILED",
        );
        exit_with_git_failure("git pull", &stdout, &stderr, output.status);
    }

    Ok(())
}

fn run_branch(args: &[String], verbose: u8, global_args: &[String]) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("git branch");
    }

    let mut cmd = git_cmd(global_args);
    cmd.arg("branch");

    // If user passes flags like -d, -D, -m, pass through directly
    let has_action_flag = is_branch_action(args);

    if has_action_flag {
        for arg in args {
            cmd.arg(arg);
        }
        let output = cmd.output().context("Failed to run git branch")?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = format!("{}{}", stdout, stderr);

        let msg = if output.status.success() {
            "ok ✓"
        } else {
            &combined
        };

        timer.track(
            &format!("git branch {}", args.join(" ")),
            &format!("rtk git branch {}", args.join(" ")),
            &combined,
            msg,
        );

        if output.status.success() {
            println!("ok ✓");
        } else {
            exit_with_git_failure("git branch", &stdout, &stderr, output.status);
        }
        return Ok(());
    }

    // List mode: show compact branch list
    cmd.arg("-a").arg("--no-color");
    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run git branch")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let raw = stdout.to_string();

    let filtered = filter_branch_output(&stdout);
    let filtered = crate::guard::never_worse(&raw, &filtered).to_string();
    println!("{}", filtered);

    timer.track(
        &format!("git branch {}", args.join(" ")),
        &format!("rtk git branch {}", args.join(" ")),
        &raw,
        &filtered,
    );

    Ok(())
}

fn filter_branch_output(output: &str) -> String {
    let mut current = String::new();
    let mut local: Vec<String> = Vec::new();
    let mut remote: Vec<String> = Vec::new();
    let mut seen_remote: std::collections::HashSet<String> = std::collections::HashSet::new();

    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Some(branch) = line.strip_prefix("* ") {
            current = branch.to_string();
        } else if let Some(rest) = line.strip_prefix("remotes/") {
            // fork: upstream v0.42.4 parity — handle ANY remote name, not just origin
            if let Some(slash_pos) = rest.find('/') {
                let branch = &rest[slash_pos + 1..];
                if branch.starts_with("HEAD ") {
                    continue;
                }
                if seen_remote.insert(branch.to_string()) {
                    remote.push(branch.to_string());
                }
            }
        } else {
            local.push(line.to_string());
        }
    }

    let mut result = Vec::new();
    result.push(format!("* {}", current));

    if !local.is_empty() {
        for b in &local {
            result.push(format!("  {}", b));
        }
    }

    if !remote.is_empty() {
        // Filter out remotes that already exist locally
        let remote_only: Vec<&String> = remote
            .iter()
            .filter(|r| *r != &current && !local.contains(r))
            .collect();
        if !remote_only.is_empty() {
            result.push(format!("  remote-only ({}):", remote_only.len()));
            for b in remote_only.iter().take(10) {
                result.push(format!("    {}", b));
            }
            if remote_only.len() > 10 {
                result.push(format!("    ... +{} more", remote_only.len() - 10));
            }
        }
    }

    result.join("\n")
}

fn run_fetch(args: &[String], verbose: u8, global_args: &[String]) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("git fetch");
    }

    let mut cmd = git_cmd(global_args);
    cmd.arg("fetch");
    for arg in args {
        cmd.arg(arg);
    }

    let output = cmd.output().context("Failed to run git fetch")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}{}", stdout, stderr);

    if !output.status.success() {
        timer.track("git fetch", "rtk git fetch", &raw, "FAILED");
        exit_with_git_failure("git fetch", &stdout, &stderr, output.status);
    }

    // Count new refs from stderr (git fetch outputs to stderr)
    let new_refs: usize = stderr
        .lines()
        .filter(|l| l.contains("->") || l.contains("[new"))
        .count();

    let msg = if new_refs > 0 {
        format!("ok fetched ({} new refs)", new_refs)
    } else {
        "ok fetched".to_string()
    };

    println!("{}", msg);
    timer.track("git fetch", "rtk git fetch", &raw, &msg);

    Ok(())
}

fn run_stash(
    subcommand: Option<&str>,
    args: &[String],
    verbose: u8,
    global_args: &[String],
) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("git stash {:?}", subcommand);
    }

    match subcommand {
        Some("list") => {
            let output = git_cmd(global_args)
                .args(["stash", "list"])
                .output()
                .context("Failed to run git stash list")?;
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let raw = stdout.to_string();

            if !output.status.success() {
                timer.track("git stash list", "rtk git stash list", &raw, "FAILED");
                exit_with_git_failure("git stash list", &stdout, &stderr, output.status);
            }

            if stdout.trim().is_empty() {
                timer.track("git stash list", "rtk git stash list", &raw, "");
                return Ok(());
            }

            let filtered = filter_stash_list(&stdout);
            let shown = crate::guard::never_worse(&raw, &filtered);
            println!("{}", shown);
            timer.track("git stash list", "rtk git stash list", &raw, shown);
        }
        Some("show") => {
            let patch_mode = args.iter().any(|arg| arg == "-p" || arg == "--patch");
            let mut cmd = git_cmd(global_args);
            cmd.args(["stash", "show"]);
            for arg in args {
                cmd.arg(arg);
            }
            let output = cmd.output().context("Failed to run git stash show")?;
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let raw = stdout.to_string();

            if !output.status.success() {
                timer.track("git stash show", "rtk git stash show", &raw, "FAILED");
                exit_with_git_failure("git stash show", &stdout, &stderr, output.status);
            }

            let filtered = if stdout.trim().is_empty() {
                let msg = "Empty stash";
                println!("{}", msg);
                msg.to_string()
            } else if patch_mode {
                let compacted = compact_diff(&stdout, 100);
                let shown = crate::guard::never_worse(&raw, &compacted).to_string();
                println!("{}", shown);
                shown
            } else {
                let compacted = compact_stash_stat(&stdout);
                let shown = crate::guard::never_worse(&raw, &compacted).to_string();
                println!("{}", shown);
                shown
            };

            timer.track("git stash show", "rtk git stash show", &raw, &filtered);
        }
        Some("pop") | Some("apply") | Some("drop") | Some("push") => {
            let sub = subcommand.unwrap();
            let mut cmd = git_cmd(global_args);
            cmd.args(["stash", sub]);
            for arg in args {
                cmd.arg(arg);
            }
            let output = cmd.output().context("Failed to run git stash")?;
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let combined = format!("{}{}", stdout, stderr);

            // P1-3: track + exit in each branch to avoid unreachable code after -> !
            if output.status.success() {
                let msg = format!("ok stash {}", sub);
                println!("{}", msg);
                timer.track(
                    &format!("git stash {}", sub),
                    &format!("rtk git stash {}", sub),
                    &combined,
                    &msg,
                );
            } else {
                timer.track(
                    &format!("git stash {}", sub),
                    &format!("rtk git stash {}", sub),
                    &combined,
                    "FAILED",
                );
                exit_with_git_failure(
                    &format!("git stash {}", sub),
                    &stdout,
                    &stderr,
                    output.status,
                );
            }
        }
        _ => {
            // Default: git stash (push)
            let mut cmd = git_cmd(global_args);
            cmd.arg("stash");
            for arg in args {
                cmd.arg(arg);
            }
            let output = cmd.output().context("Failed to run git stash")?;
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let combined = format!("{}{}", stdout, stderr);

            // P1-3: track + exit in each branch to avoid unreachable code after -> !
            if output.status.success() {
                let msg = if stdout.contains("No local changes") {
                    "ok (nothing to stash)"
                } else {
                    "ok stashed"
                };
                println!("{}", msg);
                timer.track("git stash", "rtk git stash", &combined, msg);
            } else {
                timer.track("git stash", "rtk git stash", &combined, "FAILED");
                exit_with_git_failure("git stash", &stdout, &stderr, output.status);
            }
        }
    }

    Ok(())
}

fn compact_stash_stat(raw: &str) -> String {
    use crate::truncate::CAP_LIST;

    let (files, summary) = parse_stash_stat(raw);
    if files.is_empty() {
        return raw.trim_end().to_string();
    }
    let total = files.len();
    let mut output = files[..total.min(CAP_LIST)].join("\n");
    if total > CAP_LIST {
        output.push_str(&format!("\n... +{} more files", total - CAP_LIST));
        if let Some(hint) =
            crate::tee::force_tee_tail_hint(&files.join("\n"), "git-stash-show", CAP_LIST + 1)
        {
            output.push(' ');
            output.push_str(&hint);
        }
    }
    if !summary.is_empty() {
        output.push('\n');
        output.push_str(&compress_stat_summary(&summary));
    }
    output
}

fn compress_stat_summary(summary: &str) -> String {
    summary
        .replace("insertions(+)", "+")
        .replace("insertion(+)", "+")
        .replace("deletions(-)", "-")
        .replace("deletion(-)", "-")
        .replace("files changed", "changed")
        .replace("file changed", "changed")
        .replace(',', "")
}

fn parse_stash_stat(stat: &str) -> (Vec<String>, String) {
    let stat = crate::utils::strip_ansi(stat);
    let mut files = Vec::new();
    let mut summary = String::new();
    for line in stat.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match diffstat_row(line) {
            Some(row) => files.push(row),
            None => summary = line.to_string(),
        }
    }
    (files, summary)
}

fn diffstat_row(line: &str) -> Option<String> {
    let bar = line.rfind('|')?;
    let path = line[..bar].trim();
    let rhs = line[bar + 1..].trim();
    let is_diffstat_row = rhs.starts_with("Bin") || rhs.starts_with(|c: char| c.is_ascii_digit());
    if path.is_empty() || !is_diffstat_row {
        return None;
    }
    if rhs.starts_with("Bin") {
        return Some(format!("{} (binary)", path));
    }
    let count = rhs.split_whitespace().next().unwrap_or("");
    let sign = match (rhs.contains('+'), rhs.contains('-')) {
        (true, true) => " +-",
        (true, false) => " +",
        (false, true) => " -",
        (false, false) => "",
    };
    Some(format!("{} {}{}", path, count, sign))
}

fn filter_stash_list(output: &str) -> String {
    // Format: "stash@{0}: WIP on main: abc1234 commit message"
    let mut result = Vec::new();
    for line in output.lines() {
        if let Some(colon_pos) = line.find(": ") {
            let index = &line[..colon_pos];
            let rest = &line[colon_pos + 2..];
            // Compact: strip "WIP on branch:" prefix if present
            let message = if let Some(second_colon) = rest.find(": ") {
                rest[second_colon + 2..].trim()
            } else {
                rest.trim()
            };
            result.push(format!("{}: {}", index, message));
        } else {
            result.push(line.to_string());
        }
    }
    result.join("\n")
}

fn run_worktree(args: &[String], verbose: u8, global_args: &[String]) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("git worktree list");
    }

    // If args contain "add", "remove", "prune" etc., pass through
    let has_action = is_worktree_action(args);

    if has_action {
        let mut cmd = git_cmd(global_args);
        cmd.arg("worktree");
        for arg in args {
            cmd.arg(arg);
        }
        let output = cmd.output().context("Failed to run git worktree")?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let combined = format!("{}{}", stdout, stderr);

        let msg = if output.status.success() {
            "ok ✓"
        } else {
            &combined
        };

        timer.track(
            &format!("git worktree {}", args.join(" ")),
            &format!("rtk git worktree {}", args.join(" ")),
            &combined,
            msg,
        );

        if output.status.success() {
            println!("ok ✓");
        } else {
            exit_with_git_failure(
                &format!("git worktree {}", args.join(" ")),
                &stdout,
                &stderr,
                output.status,
            );
        }
        return Ok(());
    }

    // Default: list mode
    let output = git_cmd(global_args)
        .args(["worktree", "list"])
        .output()
        .context("Failed to run git worktree list")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = stdout.to_string();

    if !output.status.success() {
        timer.track("git worktree list", "rtk git worktree", &raw, "FAILED");
        exit_with_git_failure("git worktree list", &stdout, &stderr, output.status);
    }

    let filtered = filter_worktree_list(&stdout);
    let filtered = crate::guard::never_worse(&raw, &filtered).to_string();
    println!("{}", filtered);
    timer.track("git worktree list", "rtk git worktree", &raw, &filtered);

    Ok(())
}

fn filter_worktree_list(output: &str) -> String {
    let home = dirs::home_dir()
        .map(|h| h.to_string_lossy().to_string())
        .unwrap_or_default();

    let mut result = Vec::new();
    for line in output.lines() {
        if line.trim().is_empty() {
            continue;
        }
        // Format: "/path/to/worktree  abc1234 [branch]"
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 3 {
            let mut path = parts[0].to_string();
            if !home.is_empty() && path.starts_with(&home) {
                path = format!("~{}", &path[home.len()..]);
            }
            let hash = parts[1];
            let branch = parts[2..].join(" ");
            result.push(format!("{} {} {}", path, hash, branch));
        } else {
            result.push(line.to_string());
        }
    }
    result.join("\n")
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct BlameMetadata {
    author: String,
    date: String,
    summary: String,
}

#[derive(Debug, PartialEq, Eq)]
struct BlameRange {
    start: usize,
    end: usize,
    commit: String,
    metadata: BlameMetadata,
}

fn run_blame(args: &[String], verbose: u8, global_args: &[String]) -> Result<()> {
    let timer = tracking::TimedExecution::start();
    let porcelain_requested = args
        .iter()
        .any(|arg| arg == "--line-porcelain" || arg == "--porcelain" || arg == "-p");
    let incremental_requested = args.iter().any(|arg| arg == "--incremental");
    let exact_output = fidelity::exact_requested(BLAME_EXACT_REASONS)?;
    if incremental_requested && !exact_output {
        anyhow::bail!(
            "git blame --incremental requires exact fidelity with the verbatim_source reason"
        );
    }
    if exact_output && !(porcelain_requested || incremental_requested) {
        anyhow::bail!(
            "exact git blame fidelity requires an explicit porcelain or incremental mode"
        );
    }

    if verbose > 0 {
        eprintln!("git blame");
    }

    let mut cmd = git_cmd(global_args);
    cmd.arg("blame");
    if !exact_output {
        cmd.arg("--line-porcelain");
    }
    cmd.args(args);
    let output = cmd.output().context("Failed to run git blame")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = stdout.to_string();

    if !output.status.success() {
        timer.track("git blame", "rtk git blame", &raw, "FAILED");
        exit_with_git_failure("git blame", &stdout, &stderr, output.status);
    }

    let shown = if exact_output {
        raw.clone()
    } else {
        let compact = compact_blame(&raw);
        if compact.is_empty() && !raw.trim().is_empty() {
            raw.clone()
        } else {
            crate::guard::never_worse(&raw, &compact).to_string()
        }
    };
    print!("{shown}");
    if !shown.ends_with('\n') {
        println!();
    }
    timer.track("git blame", "rtk git blame", &raw, &shown);
    Ok(())
}

fn compact_blame(porcelain: &str) -> String {
    let mut metadata_by_commit = HashMap::<String, BlameMetadata>::new();
    let mut ranges = Vec::<BlameRange>::new();
    let mut current: Option<(String, usize)> = None;
    let mut author = None;
    let mut author_time = None;
    let mut author_tz = None;
    let mut summary = None;

    for line in porcelain.lines() {
        if let Some((commit, final_line)) = parse_blame_header(line) {
            current = Some((commit, final_line));
            author = None;
            author_time = None;
            author_tz = None;
            summary = None;
            continue;
        }
        if let Some(value) = line.strip_prefix("author ") {
            author = Some(value.to_owned());
        } else if let Some(value) = line.strip_prefix("author-time ") {
            author_time = Some(value.to_owned());
        } else if let Some(value) = line.strip_prefix("author-tz ") {
            author_tz = Some(value.to_owned());
        } else if let Some(value) = line.strip_prefix("summary ") {
            summary = Some(value.to_owned());
        } else if line.starts_with('\t') {
            let Some((commit, final_line)) = current.take() else {
                continue;
            };
            let cached = metadata_by_commit.get(&commit);
            let metadata = BlameMetadata {
                author: author
                    .take()
                    .or_else(|| cached.map(|value| value.author.clone()))
                    .unwrap_or_else(|| "unknown".to_owned()),
                date: blame_date(author_time.take().as_deref(), author_tz.take().as_deref())
                    .or_else(|| cached.map(|value| value.date.clone()))
                    .unwrap_or_else(|| "unknown-date".to_owned()),
                summary: summary
                    .take()
                    .or_else(|| cached.map(|value| value.summary.clone()))
                    .unwrap_or_else(|| "(no summary)".to_owned()),
            };
            metadata_by_commit.insert(commit.clone(), metadata.clone());

            if let Some(previous) = ranges.last_mut() {
                if previous.commit == commit
                    && previous.metadata == metadata
                    && previous.end.saturating_add(1) == final_line
                {
                    previous.end = final_line;
                    continue;
                }
            }
            ranges.push(BlameRange {
                start: final_line,
                end: final_line,
                commit,
                metadata,
            });
        }
    }

    ranges
        .into_iter()
        .map(|range| {
            let lines = if range.start == range.end {
                range.start.to_string()
            } else {
                format!("{}-{}", range.start, range.end)
            };
            format!(
                "{lines} | {} | {} | {} | {}",
                range.commit, range.metadata.author, range.metadata.date, range.metadata.summary
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_blame_header(line: &str) -> Option<(String, usize)> {
    let mut parts = line.split_whitespace();
    let commit = parts.next()?;
    if commit.len() < 7 || !commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    parts.next()?.parse::<usize>().ok()?;
    let final_line = parts.next()?.parse::<usize>().ok()?;
    Some((commit.to_owned(), final_line))
}

fn blame_date(timestamp: Option<&str>, timezone: Option<&str>) -> Option<String> {
    let seconds = timestamp?.parse::<i64>().ok()?;
    let timezone = timezone?;
    let timezone_bytes = timezone.as_bytes();
    if timezone_bytes.len() != 5 || !timezone_bytes[1..].iter().all(u8::is_ascii_digit) {
        return None;
    }
    let sign = match timezone_bytes.first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let hours = timezone[1..3].parse::<i32>().ok()?;
    let minutes = timezone[3..5].parse::<i32>().ok()?;
    let offset = FixedOffset::east_opt(sign * (hours * 3600 + minutes * 60))?;
    Some(
        chrono::DateTime::<Utc>::from_timestamp(seconds, 0)?
            .with_timezone(&offset)
            .format("%Y-%m-%d")
            .to_string(),
    )
}

/// Runs an unsupported git subcommand by passing it through directly
pub fn run_passthrough(args: &[OsString], verbose: u8, global_args: &[String]) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("git passthrough: {:?}", args);
    }
    let status = git_cmd(global_args)
        .args(args)
        .status()
        .context("Failed to run git")?;

    let args_str = tracking::args_display(args);
    timer.track_passthrough(
        &format!("git {}", args_str),
        &format!("rtk git {} (passthrough)", args_str),
    );

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owned(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| (*a).to_string()).collect()
    }

    #[test]
    fn parsed_diff_paths_pin_the_renderer() {
        // An external diff driver (difftastic, delta) replaces the diff
        // wholesale, and a textconv driver or noprefix config moves the paths
        // compact_diff reads. Every parsed path must pin all of them.
        let cmd = git_diff_cmd(&[], &["diff", "--stat"]);
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        for flag in DIFF_HARDENING {
            assert!(args.contains(&(*flag).to_string()), "missing {flag}");
        }
        assert_eq!(args[0], "diff", "subcommand must come first");
        assert!(
            args.iter().position(|a| a == "--stat").unwrap()
                < args.iter().position(|a| a == "--no-ext-diff").unwrap(),
            "hardening goes after the subcommand, before the caller's args"
        );
    }

    #[test]
    fn compact_diff_never_silently_drops_unrecognised_output() {
        // Output from a driver that slipped past the hardening has no
        // `diff --git`/`@@`; the raw text must survive rather than becoming "".
        let foreign = "1 file changed\nfoo.rs  --- 3 lines removed";
        assert_eq!(compact_diff(foreign, 100), foreign);
        assert_eq!(compact_diff("", 100), "");
    }

    #[test]
    fn raw_diff_shapes_take_the_passthrough_path() {
        for shape in [
            "-p",
            "--patch",
            "--stat",
            "--stat=200",
            "--numstat",
            "--shortstat",
            "--name-only",
            "--name-status",
            "--raw",
            "--summary",
            "--dirstat",
        ] {
            assert!(
                requests_raw_log_output(&owned(&[shape])),
                "{shape} must pass through untouched"
            );
        }
    }

    #[test]
    fn ordinary_log_shapes_stay_on_the_filtered_path() {
        for shape in ["--oneline", "-10", "--author", "--graph", "--decorate"] {
            assert!(!requests_raw_log_output(&owned(&[shape])));
        }
    }

    #[test]
    fn a_flag_lookalike_value_is_not_read_as_a_flag() {
        // `--grep -p` searches messages for the literal "-p".
        assert!(!requests_raw_log_output(&owned(&["--grep", "-p"])));
        assert!(!requests_raw_log_output(&owned(&["--author", "--stat"])));
        assert!(!requests_raw_log_output(&owned(&[
            "--diff-algorithm",
            "--numstat"
        ])));
        assert_eq!(real_flag_args(&owned(&["--grep", "-p"])), vec!["--grep"]);
    }

    #[test]
    fn pathspec_after_double_dash_is_never_a_flag() {
        assert!(!requests_raw_log_output(&owned(&["--", "-p"])));
        assert!(!requests_raw_log_output(&owned(&[
            "--oneline",
            "--",
            "--stat"
        ])));
        assert_eq!(parse_user_limit(&owned(&["--", "-5"])), None);
    }

    #[test]
    fn limit_parsing_covers_every_documented_form() {
        assert_eq!(parse_user_limit(&owned(&["-20"])), Some(20));
        assert_eq!(parse_user_limit(&owned(&["-n", "20"])), Some(20));
        assert_eq!(parse_user_limit(&owned(&["--max-count=20"])), Some(20));
        assert_eq!(parse_user_limit(&owned(&["--max-count", "20"])), Some(20));
        assert_eq!(parse_user_limit(&owned(&["--oneline"])), None);
    }

    #[test]
    fn attached_value_options_do_not_swallow_the_next_token() {
        // `-U5` carries its own value; `--stat` after it is still the real flag.
        assert!(requests_raw_log_output(&owned(&["-U5", "--stat"])));
        assert!(requests_raw_log_output(&owned(&[
            "--max-parents=1",
            "--numstat"
        ])));
        assert_eq!(
            real_flag_args(&owned(&["-U5", "--stat"])),
            vec!["-U5", "--stat"]
        );
    }

    #[test]
    fn checkout_success_is_compacted() {
        assert_eq!(
            format_checkout_output(
                &["-b".into(), "feature/test".into()],
                "Switched to a new branch 'feature/test'\n",
                0,
            ),
            "ok feature/test (new)"
        );
        assert_eq!(
            format_checkout_output(
                &["--".into(), "src/a.rs".into(), "src/b.rs".into()],
                "Updated 2 paths from the index\n",
                0,
            ),
            "ok 2 files restored"
        );
    }

    #[test]
    fn checkout_failure_keeps_actionable_lines() {
        let raw = "error: The following untracked working tree files would be overwritten by checkout:\n\tsrc/main.rs\nPlease move or remove them before you switch branches.\nAborting\n";
        let filtered = format_checkout_output(&["main".into()], raw, 1);
        assert!(filtered.contains("error: The following untracked"));
        assert!(filtered.contains("src/main.rs"));
        assert!(filtered.contains("Aborting"));
    }

    #[test]
    fn stash_stat_is_compacted() {
        let raw = " src/main.rs | 10 +++++-----\n tests/main.rs | 2 ++\n 2 files changed, 7 insertions(+), 5 deletions(-)\n";
        assert_eq!(
            compact_stash_stat(raw),
            "src/main.rs 10 +-\ntests/main.rs 2 +\n2 changed 7 + 5 -"
        );
    }

    #[test]
    fn test_classify_git_command_read_only() {
        assert_eq!(
            classify_git_command(&GitCommand::Status, &[]),
            GitCommandClass::ReadOnly
        );
        assert_eq!(
            classify_git_command(
                &GitCommand::Stash {
                    subcommand: Some("list".to_string())
                },
                &[]
            ),
            GitCommandClass::ReadOnly
        );
        assert_eq!(
            classify_git_command(&GitCommand::Worktree, &[]),
            GitCommandClass::ReadOnly
        );
        assert_eq!(
            classify_git_command(&GitCommand::Blame, &[]),
            GitCommandClass::ReadOnly
        );
    }

    #[test]
    fn blame_porcelain_groups_contiguous_lines_below_ten_percent() {
        let commit = "0123456789abcdef0123456789abcdef01234567";
        let porcelain = (1..=100)
            .map(|line| {
                format!(
                    "{commit} {line} {line}\nauthor Ada Lovelace\nauthor-mail <ada@example.test>\nauthor-time 1704067200\nauthor-tz +0000\ncommitter Ada Lovelace\ncommitter-mail <ada@example.test>\ncommitter-time 1704067200\ncommitter-tz +0000\nsummary Establish analytical engine\nfilename src/engine.rs\n\tlet line_{line} = {line};\n"
                )
            })
            .collect::<String>();

        let compact = compact_blame(&porcelain);
        assert_eq!(
            compact,
            "1-100 | 0123456789abcdef0123456789abcdef01234567 | Ada Lovelace | 2024-01-01 | Establish analytical engine"
        );
        assert!(compact.len() * 10 <= porcelain.len());
    }

    #[test]
    fn blame_porcelain_preserves_ranges_commits_authors_dates_and_summaries() {
        let first = "0123456789abcdef0123456789abcdef01234567";
        let second = "fedcba9876543210fedcba9876543210fedcba98";
        let porcelain = format!(
            "{first} 8 20\nauthor Ada\nauthor-time 1704067200\nauthor-tz +0000\nsummary First change\nfilename src/lib.rs\n\tfirst\n{first} 9 21\nauthor Ada\nauthor-time 1704067200\nauthor-tz +0000\nsummary First change\nfilename src/lib.rs\n\tsecond\n{second} 3 22\nauthor Grace\nauthor-time 1704153600\nauthor-tz +0000\nsummary Follow-up\nfilename src/lib.rs\n\tthird\n"
        );

        assert_eq!(
            compact_blame(&porcelain),
            format!(
                "20-21 | {first} | Ada | 2024-01-01 | First change\n22 | {second} | Grace | 2024-01-02 | Follow-up"
            )
        );
    }

    #[test]
    fn test_classify_git_command_mutating() {
        assert_eq!(
            classify_git_command(&GitCommand::Add, &[]),
            GitCommandClass::Mutating
        );
        assert_eq!(
            classify_git_command(
                &GitCommand::Commit {
                    message: "x".to_string()
                },
                &[]
            ),
            GitCommandClass::Mutating
        );
        assert_eq!(
            classify_git_command(&GitCommand::Branch, &["-d".to_string(), "tmp".to_string()]),
            GitCommandClass::Mutating
        );
        assert_eq!(
            classify_git_command(
                &GitCommand::Stash {
                    subcommand: Some("drop".to_string())
                },
                &[]
            ),
            GitCommandClass::Mutating
        );
        assert_eq!(
            classify_git_command(
                &GitCommand::Worktree,
                &["remove".to_string(), "/tmp/wt".to_string()]
            ),
            GitCommandClass::Mutating
        );
    }

    #[test]
    fn test_compact_diff() {
        let diff = r#"diff --git a/foo.rs b/foo.rs
--- a/foo.rs
+++ b/foo.rs
@@ -1,3 +1,4 @@
 fn main() {
+    println!("hello");
 }
"#;
        let result = compact_diff(diff, 100);
        assert!(result.contains("foo.rs"));
        assert!(result.contains("+"));
    }

    #[test]
    fn test_filter_branch_output() {
        let output = "* main\n  feature/auth\n  fix/bug-123\n  remotes/origin/HEAD -> origin/main\n  remotes/origin/main\n  remotes/origin/feature/auth\n  remotes/origin/release/v2\n";
        let result = filter_branch_output(output);
        assert!(result.contains("* main"));
        assert!(result.contains("feature/auth"));
        assert!(result.contains("fix/bug-123"));
        // remote-only should show release/v2 but not main or feature/auth (already local)
        assert!(result.contains("remote-only"));
        assert!(result.contains("release/v2"));
    }

    #[test]
    fn test_filter_branch_no_remotes() {
        let output = "* main\n  develop\n";
        let result = filter_branch_output(output);
        assert!(result.contains("* main"));
        assert!(result.contains("develop"));
        assert!(!result.contains("remote-only"));
    }

    #[test]
    fn test_filter_stash_list() {
        let output =
            "stash@{0}: WIP on main: abc1234 fix login\nstash@{1}: On feature: def5678 wip\n";
        let result = filter_stash_list(output);
        assert!(result.contains("stash@{0}: abc1234 fix login"));
        assert!(result.contains("stash@{1}: def5678 wip"));
    }

    #[test]
    fn test_filter_worktree_list() {
        let output =
            "/home/user/project  abc1234 [main]\n/home/user/worktrees/feat  def5678 [feature]\n";
        let result = filter_worktree_list(output);
        assert!(result.contains("abc1234"));
        assert!(result.contains("[main]"));
        assert!(result.contains("[feature]"));
    }

    #[test]
    fn test_format_status_output_clean() {
        let porcelain = "";
        let result = format_status_output(porcelain);
        assert_eq!(result, "Clean working tree");
    }

    #[test]
    fn test_format_status_output_modified_files() {
        let porcelain = "## main...origin/main\n M src/main.rs\n M src/lib.rs\n";
        let result = format_status_output(porcelain);
        assert!(result.contains("📌 main...origin/main"));
        assert!(result.contains("📝 Modified: 2 files"));
        assert!(result.contains("src/main.rs"));
        assert!(result.contains("src/lib.rs"));
        assert!(!result.contains("Staged"));
        assert!(!result.contains("Untracked"));
    }

    #[test]
    fn test_format_status_output_untracked_files() {
        let porcelain = "## feature/new\n?? temp.txt\n?? debug.log\n?? test.sh\n";
        let result = format_status_output(porcelain);
        assert!(result.contains("📌 feature/new"));
        assert!(result.contains("❓ Untracked: 3 files"));
        assert!(result.contains("temp.txt"));
        assert!(result.contains("debug.log"));
        assert!(result.contains("test.sh"));
        assert!(!result.contains("Modified"));
    }

    #[test]
    fn test_format_status_output_mixed_changes() {
        let porcelain = r#"## main
M  staged.rs
 M modified.rs
A  added.rs
?? untracked.txt
"#;
        let result = format_status_output(porcelain);
        assert!(result.contains("📌 main"));
        assert!(result.contains("✅ Staged: 2 files"));
        assert!(result.contains("staged.rs"));
        assert!(result.contains("added.rs"));
        assert!(result.contains("📝 Modified: 1 files"));
        assert!(result.contains("modified.rs"));
        assert!(result.contains("❓ Untracked: 1 files"));
        assert!(result.contains("untracked.txt"));
    }

    #[test]
    fn test_format_status_output_truncation() {
        // Test that >5 staged files show "... +N more"
        let porcelain = r#"## main
M  file1.rs
M  file2.rs
M  file3.rs
M  file4.rs
M  file5.rs
M  file6.rs
M  file7.rs
"#;
        let result = format_status_output(porcelain);
        assert!(result.contains("✅ Staged: 7 files"));
        assert!(result.contains("file1.rs"));
        assert!(result.contains("file5.rs"));
        assert!(result.contains("... +2 more"));
        assert!(!result.contains("file6.rs"));
        assert!(!result.contains("file7.rs"));
    }

    #[test]
    fn test_run_passthrough_accepts_args() {
        // Test that run_passthrough compiles and has correct signature
        let _args: Vec<OsString> = vec![OsString::from("tag"), OsString::from("--list")];
        // Compile-time verification that the function exists with correct signature
    }

    #[test]
    fn test_filter_log_output() {
        let output = "abc1234 This is a commit message (2 days ago) <author>\ndef5678 Another commit (1 week ago) <other>\n";
        let result = filter_log_output(output, 10);
        assert!(result.contains("abc1234"));
        assert!(result.contains("def5678"));
        assert_eq!(result.lines().count(), 2);
    }

    #[test]
    fn test_filter_log_output_truncate_long() {
        let long_line = "abc1234 ".to_string() + &"x".repeat(100) + " (2 days ago) <author>";
        let result = filter_log_output(&long_line, 10);
        assert!(result.len() < long_line.len());
        assert!(result.contains("..."));
        assert!(result.len() <= 80);
    }

    #[test]
    fn test_filter_log_output_cap_lines() {
        let output = (0..20)
            .map(|i| format!("hash{} message {} (1 day ago) <author>", i, i))
            .collect::<Vec<_>>()
            .join("\n");
        let result = filter_log_output(&output, 5);
        assert_eq!(result.lines().count(), 5);
    }

    #[test]
    fn test_filter_status_with_args() {
        let output = r#"On branch main
Your branch is up to date with 'origin/main'.

Changes not staged for commit:
  (use "git add <file>..." to update what will be committed)
  (use "git restore <file>..." to discard changes in working directory)
	modified:   src/main.rs

no changes added to commit (use "git add" and/or "git commit -a")
"#;
        let result = filter_status_with_args(output);
        eprintln!("Result:\n{}", result);
        assert!(result.contains("On branch main"));
        assert!(result.contains("modified:   src/main.rs"));
        assert!(
            !result.contains("(use \"git"),
            "Result should not contain git hints"
        );
    }

    #[test]
    fn test_filter_status_with_args_clean() {
        let output = "nothing to commit, working tree clean\n";
        let result = filter_status_with_args(output);
        assert!(result.contains("nothing to commit"));
    }

    #[test]
    fn test_filter_log_output_multibyte() {
        // Thai characters: each is 3 bytes. A line with >80 bytes but few chars
        let thai_msg = format!("abc1234 {} (2 days ago) <author>", "ก".repeat(30));
        let result = filter_log_output(&thai_msg, 10);
        // Should not panic
        assert!(result.contains("abc1234"));
        // The line has 30 Thai chars (90 bytes) + other text, so > 80 bytes
        // It should be truncated with "..."
        assert!(result.contains("..."));
    }

    #[test]
    fn test_filter_log_output_emoji() {
        let emoji_msg = "abc1234 🎉🎊🎈🎁🎂🎄🎃🎆🎇✨🎉🎊🎈🎁🎂🎄🎃🎆🎇✨ (1 day ago) <user>";
        let result = filter_log_output(emoji_msg, 10);
        // Should not panic, should have "..."
        assert!(result.contains("..."));
    }

    #[test]
    fn test_format_status_output_thai_filename() {
        let porcelain = "## main\n M สวัสดี.txt\n?? ทดสอบ.rs\n";
        let result = format_status_output(porcelain);
        // Should not panic
        assert!(result.contains("📌 main"));
        assert!(result.contains("สวัสดี.txt"));
        assert!(result.contains("ทดสอบ.rs"));
    }

    #[test]
    fn test_format_status_output_emoji_filename() {
        let porcelain = "## main\nA  🎉-party.txt\n M 日本語ファイル.rs\n";
        let result = format_status_output(porcelain);
        assert!(result.contains("📌 main"));
    }

    // fix #248: is_blob_show_arg unit tests
    #[test]
    fn test_is_blob_show_arg_rev_path() {
        assert!(is_blob_show_arg("HEAD:src/main.rs"));
        assert!(is_blob_show_arg("develop:modules/file.py"));
        assert!(is_blob_show_arg("abc123:README.md"));
        assert!(is_blob_show_arg("origin/main:Cargo.toml"));
    }

    #[test]
    fn test_is_blob_show_arg_not_blob() {
        assert!(!is_blob_show_arg("--stat"));
        assert!(!is_blob_show_arg("-p"));
        assert!(!is_blob_show_arg("HEAD"));
        assert!(!is_blob_show_arg("abc123"));
        assert!(!is_blob_show_arg("--format=%s"));
    }

    // fix #192: git global options — unit tests for git_cmd and Clap parsing

    #[test]
    fn test_git_cmd_empty_global_args() {
        // git_cmd with no global args should produce a plain "git" command
        let global: Vec<String> = vec![];
        let _cmd = git_cmd(&global); // just ensure it builds without panic
    }

    #[test]
    fn test_git_cmd_with_no_pager() {
        let global = vec!["--no-pager".to_string()];
        let _cmd = git_cmd(&global);
        // Command args are not inspectable directly, but construction must succeed
    }

    #[test]
    fn test_git_cmd_with_directory() {
        let global = vec!["-C".to_string(), "/tmp".to_string()];
        let _cmd = git_cmd(&global);
    }

    #[test]
    fn test_git_cmd_multiple_global_args() {
        let global = vec![
            "--no-pager".to_string(),
            "--no-optional-locks".to_string(),
            "-C".to_string(),
            "/tmp".to_string(),
        ];
        let _cmd = git_cmd(&global);
    }
}
