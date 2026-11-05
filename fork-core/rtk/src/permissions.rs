use crate::discover::lexer::{self, ShellWrapperParse};
use crate::discover::registry;
use lazy_static::lazy_static;
use regex::Regex;
use serde_json::Value;
use std::path::PathBuf;

/// Verdict from checking a command against Claude Code permission rules.
#[derive(Debug, PartialEq, Eq, Clone)]
pub enum PermissionVerdict {
    /// An explicit allow rule matched.
    Allow,
    /// A deny rule matched.
    Deny,
    /// An ask rule matched.
    Ask,
    /// No rule matched. Treat as ask to preserve Claude Code's default.
    Default,
}

// 1.1.0 (US-001, upstream b465deff): how many prefixes the gate peels before it stops reading.
// Past the cap the command behind them is unread, so a deny/ask rule makes the verdict Ask.
const MAX_MATCHING_PEELS: usize = 32;
// 1.1.0 (US-002): quoted `sh -c` scripts inspected inside one another before giving up.
const MAX_WRAPPER_DEPTH: usize = 3;
// 1.1.0 (US-001): words that only run the next word as the command. The rewrite looks through
// `command`/`builtin`/`exec`/`noglob`/`nocorrect`; `sudo` and a bare `xargs` are match-only — the
// gate reads past them so a deny rule still names the command, the rewrite never routes them.
const MATCH_ONLY_KEYWORDS: &[&str] = &["sudo", "xargs"];

lazy_static! {
    // 1.1.0 (US-001): one leading `NAME=value` word (quoted values included).
    static ref ASSIGNMENT_WORD: Regex = Regex::new(
        r#"^[A-Za-z_][A-Za-z0-9_]*=(?:"(?:[^"\\]|\\.)*"|'[^']*'|[^\s'"])*(?:\s+|$)"#
    )
    .expect("assignment regex");
}

/// Check `cmd` against Claude Code Bash deny/ask/allow rules.
///
/// Precedence: Deny > Ask > Allow > Default. For compound commands, every
/// non-empty segment must independently match an allow rule before the whole
/// command can be auto-allowed.
pub fn check_command(cmd: &str, transparent_prefixes: &[String]) -> PermissionVerdict {
    // 1.1.0 (US-001): the configured transparent prefixes the rewrite looks through are read here too
    let (deny_rules, ask_rules, allow_rules) = load_permission_rules();
    check_command_with_prefixes(
        cmd,
        &deny_rules,
        &ask_rules,
        &allow_rules,
        transparent_prefixes,
    )
}

#[cfg(test)]
pub(crate) fn check_command_with_rules(
    cmd: &str,
    deny_rules: &[String],
    ask_rules: &[String],
    allow_rules: &[String],
) -> PermissionVerdict {
    check_command_with_prefixes(cmd, deny_rules, ask_rules, allow_rules, &[])
}

/// 1.1.0 (US-001…US-003): the gate.
///
/// HZR's managed form is an opaque prelude no host `Bash(...)` rule can match, so this verdict is
/// the only place an operator's deny rule is applied to a rewritten command. Deny and ask are
/// therefore matched against every reading of a segment the rewrite could look through
/// (assignments, `env`, `timeout`/`nice`/`nohup`/`time`/`stdbuf`, keyword prefixes, configured
/// transparent prefixes, `sudo`, bare `xargs`) and against the script of a quoted `sh -c` wrapper.
/// Allow is matched only against the segment as written, so no reading can loosen a verdict.
pub(crate) fn check_command_with_prefixes(
    cmd: &str,
    deny_rules: &[String],
    ask_rules: &[String],
    allow_rules: &[String],
    transparent_prefixes: &[String],
) -> PermissionVerdict {
    let prefixes = gate_prefixes(transparent_prefixes);
    let reads_past_prefixes = !deny_rules.is_empty() || !ask_rules.is_empty();
    let segments = split_compound_command(cmd);

    let readings: Vec<(&str, Readings<'_>)> = segments
        .iter()
        .map(|segment| {
            let segment = segment.trim();
            let peeled = if reads_past_prefixes {
                matching_readings(strip_grammar_residue(segment), &prefixes)
            } else {
                Readings::as_written(segment)
            };
            (segment, peeled)
        })
        .collect();
    let matches_any = |segment: &str, peeled: &Readings<'_>, pattern: &str| {
        command_matches_pattern(segment, pattern)
            || peeled
                .readings
                .iter()
                .any(|reading| command_matches_pattern(reading, pattern))
    };

    for (segment, peeled) in &readings {
        if deny_rules
            .iter()
            .any(|pattern| matches_any(segment, peeled, pattern))
        {
            return PermissionVerdict::Deny;
        }
    }

    // 1.1.0 (US-002): a quoted `sh -c` script is a second parsing boundary the host never sees.
    let mut wrapper_seen = false;
    for (_, peeled) in &readings {
        match wrapper_verdict(peeled, deny_rules, ask_rules, &prefixes, 0) {
            WrapperVerdict::Deny => return PermissionVerdict::Deny,
            WrapperVerdict::Wrapped => wrapper_seen = true,
            WrapperVerdict::None => {}
        }
    }

    let mut any_ask = false;
    let mut all_segments_allowed = true;
    let mut saw_segment = false;

    for (segment, peeled) in &readings {
        if segment.is_empty() {
            continue;
        }
        saw_segment = true;

        if !any_ask {
            any_ask = (reads_past_prefixes && !peeled.complete)
                || ask_rules
                    .iter()
                    .any(|pattern| matches_any(segment, peeled, pattern));
        }

        if all_segments_allowed
            && !allow_rules
                .iter()
                .any(|pattern| command_matches_pattern(segment, pattern))
        {
            all_segments_allowed = false;
        }
    }

    // 1.1.0 (US-003, upstream 3992e0ab): what segmentation cannot decompose — substitution, a
    // file-target redirect, a heredoc, or an ANSI-C quote whose `\'` the lexer reads differently
    // from bash — is never auto-allowed.
    // 1.1.0 (US-002): a rewritten `sh -c` wrapper is never auto-allowed either — a `*` allow rule
    // must not approve a script the host never parsed. It only caps Allow; Default stays the host's.
    let unattestable = wrapper_seen
        || lexer::contains_unattestable_construct(cmd)
        || registry::ansi_c_quote_defeats_lexer(cmd);

    if any_ask {
        PermissionVerdict::Ask
    } else if saw_segment && all_segments_allowed && !allow_rules.is_empty() {
        if unattestable {
            PermissionVerdict::Ask
        } else {
            PermissionVerdict::Allow
        }
    } else {
        PermissionVerdict::Default
    }
}

/// 1.1.0 (US-002): result of looking inside the quoted `-c` scripts a segment reaches.
enum WrapperVerdict {
    None,
    Wrapped,
    Deny,
}

fn wrapper_verdict(
    peeled: &Readings<'_>,
    deny_rules: &[String],
    ask_rules: &[String],
    prefixes: &[String],
    depth: usize,
) -> WrapperVerdict {
    let mut wrapped = false;
    for reading in &peeled.readings {
        let script = match lexer::parse_shell_command_wrapper(reading) {
            ShellWrapperParse::NotWrapper => continue,
            ShellWrapperParse::Ambiguous => {
                wrapped = true;
                continue;
            }
            ShellWrapperParse::Parsed(payload) => payload.command,
        };
        wrapped = true;
        if depth >= MAX_WRAPPER_DEPTH {
            // Too deep to read: the command behind it is unknown, and a deny rule exists.
            if !deny_rules.is_empty() {
                return WrapperVerdict::Deny;
            }
            continue;
        }
        let script = script.replace("\\\n", "");
        for inner in split_compound_command(&script) {
            let inner = inner.trim();
            let inner_peeled = matching_readings(strip_grammar_residue(inner), prefixes);
            let denied = deny_rules.iter().any(|pattern| {
                command_matches_pattern(inner, pattern)
                    || inner_peeled
                        .readings
                        .iter()
                        .any(|reading| command_matches_pattern(reading, pattern))
            });
            if denied {
                return WrapperVerdict::Deny;
            }
            if let WrapperVerdict::Deny =
                wrapper_verdict(&inner_peeled, deny_rules, ask_rules, prefixes, depth + 1)
            {
                return WrapperVerdict::Deny;
            }
        }
    }
    // An ask rule inside the script is covered by `Wrapped`: a wrapper is never auto-allowed.
    let _ = ask_rules;
    if wrapped {
        WrapperVerdict::Wrapped
    } else {
        WrapperVerdict::None
    }
}

/// 1.1.0 (US-001): every way a deny or ask rule may name the command a segment runs.
struct Readings<'a> {
    readings: Vec<&'a str>,
    /// False when the peel stopped at [`MAX_MATCHING_PEELS`] with prefixes left.
    complete: bool,
}

impl<'a> Readings<'a> {
    fn as_written(segment: &'a str) -> Self {
        Self {
            readings: vec![segment],
            complete: true,
        }
    }
}

/// 1.1.0 (US-001): the keyword and transparent prefixes the gate peels, longest first.
fn gate_prefixes(transparent_prefixes: &[String]) -> Vec<String> {
    let mut prefixes: Vec<String> = registry::BUILTIN_TRANSPARENT_PREFIXES
        .iter()
        .chain(MATCH_ONLY_KEYWORDS)
        .map(|prefix| prefix.to_string())
        .chain(transparent_prefixes.iter().cloned())
        .collect();
    prefixes = registry::normalize_transparent_prefixes(&prefixes);
    prefixes
}

/// 1.1.0 (US-001, upstream b465deff): peel one prefix at a time and keep each reading.
fn matching_readings<'a>(segment: &'a str, prefixes: &[String]) -> Readings<'a> {
    let mut readings = vec![segment.trim()];
    let mut rest = segment.trim();
    loop {
        let Some(inner) = peel_one(rest, prefixes) else {
            return Readings {
                readings,
                complete: true,
            };
        };
        if inner.is_empty() || inner == rest {
            return Readings {
                readings,
                complete: true,
            };
        }
        if readings.len() > MAX_MATCHING_PEELS {
            return Readings {
                readings,
                complete: false,
            };
        }
        rest = inner;
        readings.push(rest);
    }
}

fn peel_one<'a>(cmd: &'a str, prefixes: &[String]) -> Option<&'a str> {
    if let Some(word) = ASSIGNMENT_WORD.find(cmd) {
        return Some(cmd[word.end()..].trim_start());
    }
    if let Some(rest) = peel_env(cmd) {
        return Some(rest);
    }
    if let Some((_, inner)) = registry::execution_prefix(cmd) {
        return Some(inner);
    }
    prefixes
        .iter()
        .find_map(|prefix| registry::strip_word_prefix(cmd, prefix))
}

/// `env [-i] [-u NAME] [--] …` (also as `/usr/bin/env`): what follows is the command, after its
/// assignments, which the next peel takes.
fn peel_env(cmd: &str) -> Option<&str> {
    let words = lexer::shell_word_spans(cmd);
    let first = words.first()?;
    let program = &cmd[first.clone()];
    if program.rsplit('/').next() != Some("env") {
        return None;
    }
    let mut index = 1;
    while let Some(span) = words.get(index) {
        match &cmd[span.clone()] {
            "-i" | "-" | "--ignore-environment" | "-0" | "--null" => index += 1,
            "-u" | "--unset" | "-C" | "--chdir" => index += 2,
            "--" => {
                index += 1;
                break;
            }
            word if word.starts_with("--unset=") || word.starts_with("--chdir=") => index += 1,
            word if word.starts_with('-') => return None,
            _ => break,
        }
    }
    let next = words.get(index)?;
    Some(cmd[next.start..].trim_start())
}

/// 1.1.0 (US-003, upstream 3992e0ab): `{`, `}`, `!`, `(`, `)` left over by segmentation.
///
/// Deny and ask only: stripping can only make a rule fire on more segments. On the allow side it
/// would let `{ ls` inherit an `ls` rule.
fn strip_grammar_residue(segment: &str) -> &str {
    let mut rest = segment.trim();
    loop {
        let stripped = match rest.split_once([' ', '\t']) {
            Some(("{" | "}" | "!" | "(" | ")", tail)) => tail,
            _ => return rest,
        };
        let next = stripped.trim_start();
        if next == rest {
            return rest;
        }
        rest = next;
    }
}

/// 1.1.0 (US-004, upstream e27b47d1 + 455ea7fa): load rules, failing closed.
///
/// Missing (`NotFound`/`NotADirectory`) and empty files deny nothing. A file that exists but
/// cannot be read or parsed may hold deny rules that can no longer be applied, so it adds a
/// catch-all ask rule and a warning instead of being skipped. A UTF-8 BOM is not an error.
fn load_permission_rules() -> (Vec<String>, Vec<String>, Vec<String>) {
    load_permission_rules_from(&get_settings_paths())
}

fn load_permission_rules_from(paths: &[PathBuf]) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut deny_rules = Vec::new();
    let mut ask_rules = Vec::new();
    let mut allow_rules = Vec::new();

    for path in paths {
        let content = match std::fs::read_to_string(path) {
            Ok(content) => content,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) =>
            {
                continue;
            }
            Err(error) => {
                eprintln!(
                    "[rtk] warning: failed to read permissions from {}: {error}; asking for every command",
                    path.display()
                );
                ask_rules.push("*".to_string());
                continue;
            }
        };
        let content = content.strip_prefix('\u{feff}').unwrap_or(&content);
        if content.trim().is_empty() {
            continue;
        }
        let Ok(json) = serde_json::from_str::<Value>(content) else {
            eprintln!(
                "[rtk] warning: failed to parse permissions from {}; asking for every command",
                path.display()
            );
            ask_rules.push("*".to_string());
            continue;
        };
        let Some(permissions) = json.get("permissions") else {
            continue;
        };

        append_bash_rules(permissions.get("deny"), &mut deny_rules);
        append_bash_rules(permissions.get("ask"), &mut ask_rules);
        append_bash_rules(permissions.get("allow"), &mut allow_rules);
    }

    (deny_rules, ask_rules, allow_rules)
}

fn append_bash_rules(rules_value: Option<&Value>, target: &mut Vec<String>) {
    let Some(arr) = rules_value.and_then(|v| v.as_array()) else {
        return;
    };

    for rule in arr {
        if let Some(s) = rule.as_str() {
            if s.starts_with("Bash(") {
                target.push(extract_bash_pattern(s).to_string());
            }
        }
    }
}

fn get_settings_paths() -> Vec<PathBuf> {
    get_settings_paths_from(find_project_root(), claude_config_dir())
}

/// 1.1.0 (US-004, upstream f4404cb): `$CLAUDE_CONFIG_DIR` names the user's Claude directory.
fn claude_config_dir() -> Option<PathBuf> {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".claude")))
}

fn get_settings_paths_from(
    project_root: Option<PathBuf>,
    claude_dir: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Some(root) = project_root {
        paths.push(root.join(".claude").join("settings.json"));
        paths.push(root.join(".claude").join("settings.local.json"));
    }
    if let Some(claude_dir) = claude_dir {
        paths.push(claude_dir.join("settings.json"));
        paths.push(claude_dir.join("settings.local.json"));
    }

    paths
}

fn find_project_root() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        if dir.join(".claude").exists() {
            return Some(dir);
        }
        if !dir.pop() {
            break;
        }
    }

    let output = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()?;

    if output.status.success() {
        let path = String::from_utf8(output.stdout).ok()?;
        return Some(PathBuf::from(path.trim()));
    }

    None
}

pub(crate) fn extract_bash_pattern(rule: &str) -> &str {
    if let Some(inner) = rule.strip_prefix("Bash(") {
        if let Some(pattern) = inner.strip_suffix(')') {
            return pattern;
        }
    }
    rule
}

pub(crate) fn command_matches_pattern(cmd: &str, pattern: &str) -> bool {
    // 1.1.0 (US-001): bash elides `\<newline>`; a stray `\` must not hide the command from a rule
    let cmd_norm = cmd
        .replace("\\\n", "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let pattern_norm = pattern.split_whitespace().collect::<Vec<_>>().join(" ");
    let cmd = cmd_norm.as_str();
    let pattern = pattern_norm.as_str();

    if pattern == "*" {
        return true;
    }

    if let Some(p) = pattern.strip_suffix('*') {
        let prefix = p.trim_end_matches(':').trim_end();
        if prefix.is_empty() || prefix == "*" {
            return true;
        }
        if !prefix.contains('*') {
            return cmd == prefix || cmd.starts_with(&format!("{} ", prefix));
        }
    }

    if pattern.contains('*') {
        return glob_matches(cmd, pattern);
    }

    cmd == pattern || cmd.starts_with(&format!("{} ", pattern))
}

fn glob_matches(cmd: &str, pattern: &str) -> bool {
    let normalized = pattern.replace(":*", " *").replace("*:", "* ");
    let parts: Vec<&str> = normalized.split('*').collect();

    if parts.iter().all(|p| p.is_empty()) {
        return true;
    }

    let mut search_from = 0;

    for (i, part) in parts.iter().enumerate() {
        if part.is_empty() {
            continue;
        }

        if i == 0 {
            if !cmd.starts_with(part) {
                return false;
            }
            search_from = part.len();
        } else if i == parts.len() - 1 {
            if !cmd[search_from..].ends_with(*part) {
                return false;
            }
        } else {
            let remaining = &cmd[search_from..];
            if let Some(pos) = remaining.find(*part) {
                search_from += pos + part.len();
            } else {
                let trimmed = part.trim_end();
                if !trimmed.is_empty() && remaining.ends_with(trimmed) {
                    search_from += remaining.len();
                } else {
                    return false;
                }
            }
        }
    }

    true
}

/// 1.1.0 (US-003): one segmenter for the gate — the lexer's, which also breaks on newline,
/// background `&` and subshells, steps over a leading redirect and truncates at a trailing one.
/// The quote-only splitter it replaces missed all of those.
fn split_compound_command(cmd: &str) -> Vec<&str> {
    lexer::split_for_permissions(cmd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bash_permission_rule() {
        assert_eq!(
            extract_bash_pattern("Bash(git push --force)"),
            "git push --force"
        );
        assert_eq!(extract_bash_pattern("Read(**/.env*)"), "Read(**/.env*)");
    }

    #[test]
    fn default_is_not_allow() {
        assert_eq!(
            check_command_with_rules("git status", &[], &[], &[]),
            PermissionVerdict::Default
        );
    }

    #[test]
    fn deny_precedes_ask_and_allow() {
        let deny = vec!["git push --force".to_string()];
        let ask = vec!["git push".to_string()];
        let allow = vec!["git *".to_string()];
        assert_eq!(
            check_command_with_rules("git push --force origin main", &deny, &ask, &allow),
            PermissionVerdict::Deny
        );
    }

    #[test]
    fn ask_precedes_allow() {
        let ask = vec!["git push".to_string()];
        let allow = vec!["git *".to_string()];
        assert_eq!(
            check_command_with_rules("git push origin main", &[], &ask, &allow),
            PermissionVerdict::Ask
        );
    }

    #[test]
    fn all_compound_segments_must_be_allowed() {
        let allow = vec!["git status".to_string()];
        assert_eq!(
            check_command_with_rules("git status && git add .", &[], &[], &allow),
            PermissionVerdict::Default
        );
    }

    #[test]
    fn compound_allows_when_every_segment_matches() {
        let allow = vec!["git *".to_string(), "cargo *".to_string()];
        assert_eq!(
            check_command_with_rules("git status && cargo test", &[], &[], &allow),
            PermissionVerdict::Allow
        );
    }

    #[test]
    fn quoted_operators_are_not_split() {
        let deny = vec!["git push --force".to_string()];
        assert_eq!(
            check_command_with_rules(r#"echo "git push --force && danger""#, &deny, &[], &[]),
            PermissionVerdict::Default
        );
    }

    #[test]
    fn complex_wildcards_match() {
        assert!(command_matches_pattern("git push --force", "* --force"));
        assert!(command_matches_pattern(
            "git -C /tmp/repo diff",
            "git -C * diff:*"
        ));
        assert!(!command_matches_pattern("git push --forceful", "* --force"));
    }

    #[test]
    fn extra_whitespace_still_matches() {
        assert!(command_matches_pattern("git  push", "git push"));
        assert!(command_matches_pattern("git\tpush origin", "git push"));
        assert!(command_matches_pattern(
            "git   push   --force",
            "git push --force"
        ));
    }

    #[test]
    fn extra_whitespace_does_not_evade_deny() {
        let deny = vec!["git push".to_string()];
        assert_eq!(
            check_command_with_rules("git  push origin main", &deny, &[], &[]),
            PermissionVerdict::Deny
        );
    }

    #[test]
    fn whitespace_normalization_preserves_word_boundaries() {
        assert!(!command_matches_pattern(
            "git  push  --forceful",
            "git push --force"
        ));
        assert!(!command_matches_pattern("sudoedit /etc/hosts", "sudo:*"));
    }

    // ---- 1.1.0 upstream sync: permission gate (US-001…US-004) ----

    fn rules(list: &[&str]) -> Vec<String> {
        list.iter().map(|rule| rule.to_string()).collect()
    }

    #[test]
    fn deny_reads_past_assignments_and_process_wrappers() {
        // 1.1.0 (US-001): every form below ran under `allow timeout:*`/`env:*` before.
        let deny = rules(&["git push:*"]);
        let allow = rules(&["timeout:*", "env:*", "nohup:*", "ls:*"]);
        for cmd in [
            "HUSKY=0 git push",
            "A=1 B='x y' git push",
            "env FOO=1 git push",
            "env -i git push",
            "/usr/bin/env git push",
            "timeout 30 git push",
            "timeout -s KILL 30 git push",
            "nice -n 5 git push",
            "nohup git push",
            "time git push",
            "command git push",
            "builtin git push",
            "exec git push",
            "noglob git push",
            "sudo git push",
            "xargs git push",
            "uv run git push",
            "ls; timeout 5 git push",
            "nohup HUSKY=0 timeout 5 git push origin",
        ] {
            assert_eq!(
                check_command_with_rules(cmd, &deny, &[], &allow),
                PermissionVerdict::Deny,
                "{cmd}"
            );
        }
    }

    #[test]
    fn configured_transparent_prefixes_are_read_for_deny() {
        let deny = rules(&["git push:*"]);
        assert_eq!(
            check_command_with_prefixes(
                "docker exec app git push",
                &deny,
                &[],
                &[],
                &["docker exec app".to_string()],
            ),
            PermissionVerdict::Deny
        );
    }

    #[test]
    fn program_arguments_are_not_peeled() {
        // `git -C . push` stays `git`: reading past a program's own options needs its grammar.
        let deny = rules(&["git push:*"]);
        assert_eq!(
            check_command_with_rules("git -C . push", &deny, &[], &[]),
            PermissionVerdict::Default
        );
    }

    #[test]
    fn ask_reads_past_prefixes_too() {
        let ask = rules(&["git push:*"]);
        let allow = rules(&["timeout:*"]);
        assert_eq!(
            check_command_with_rules("timeout 5 git push", &[], &ask, &allow),
            PermissionVerdict::Ask
        );
    }

    #[test]
    fn allow_is_never_inherited_through_a_wrapper() {
        // Peeling is for deny/ask only: `timeout 30 <cmd>` must not inherit `<cmd>`'s allow.
        let allow = rules(&["git status:*"]);
        assert_eq!(
            check_command_with_rules("timeout 30 git status", &[], &[], &allow),
            PermissionVerdict::Default
        );
    }

    #[test]
    fn deny_inside_a_quoted_shell_wrapper_wins() {
        // 1.1.0 (US-002)
        let deny = rules(&["git push:*"]);
        let allow = rules(&["env:*", "bash:*"]);
        for cmd in [
            "bash -c 'git push'",
            "env bash -c 'git push'",
            "sh -c \"cd x && git push\"",
            "bash -c 'timeout 5 git push'",
            "bash -c 'FOO=1 git push'",
            "uv run bash -c 'git push'",
            "bash -c \"bash -c 'git push'\"",
        ] {
            assert_eq!(
                check_command_with_rules(cmd, &deny, &[], &allow),
                PermissionVerdict::Deny,
                "{cmd}"
            );
        }
    }

    #[test]
    fn a_shell_wrapper_is_never_auto_allowed_but_keeps_default() {
        // 1.1.0 (US-002): an outer allow cannot answer for a script the host never parsed.
        let allow = rules(&["bash:*", "ls:*"]);
        assert_eq!(
            check_command_with_rules("bash -c 'ls'", &[], &[], &allow),
            PermissionVerdict::Ask
        );
        assert_eq!(
            check_command_with_rules("bash -c 'ls'", &[], &[], &[]),
            PermissionVerdict::Default
        );
    }

    #[test]
    fn ansi_c_quote_that_defeats_the_lexer_is_never_allowed() {
        // 1.1.0 (US-003, upstream 3992e0ab): bash keeps `$'\''` open; the lexer closed it, so
        // `; rm -rf /` rode along inside an `ls` segment.
        let allow = rules(&["ls:*"]);
        assert_eq!(
            check_command_with_rules(r"ls -la $'\'' ; rm -rf /", &[], &[], &allow),
            PermissionVerdict::Ask
        );
    }

    #[test]
    fn leading_redirect_and_grammar_residue_do_not_hide_a_deny() {
        // 1.1.0 (US-003)
        let deny = rules(&["git push:*", "rm -rf /"]);
        for cmd in [
            "2>&1 git push --force",
            ">/tmp/log git push",
            "ls && { rm -rf / ; }",
            "! git push",
            "( git push )",
        ] {
            assert_eq!(
                check_command_with_rules(cmd, &deny, &[], &rules(&["ls:*"])),
                PermissionVerdict::Deny,
                "{cmd}"
            );
        }
    }

    #[test]
    fn grammar_residue_never_widens_allow() {
        let allow = rules(&["git status:*"]);
        assert_eq!(
            check_command_with_rules("{ git status; }", &[], &[], &allow),
            PermissionVerdict::Default
        );
    }

    #[test]
    fn unattestable_constructs_cap_allow_at_ask() {
        // 1.1.0 (US-003): file redirects, substitution and heredocs are never auto-allowed.
        let allow = rules(&["ls:*", "cat:*"]);
        for cmd in [
            "ls > out.txt",
            "ls $(echo x)",
            "cat <<EOF\nx\nEOF",
            "cat << /dev/null",
        ] {
            assert_ne!(
                check_command_with_rules(cmd, &[], &[], &allow),
                PermissionVerdict::Allow,
                "{cmd}"
            );
        }
        for cmd in ["ls 2>&1", "ls 2>/dev/null", "ls -la"] {
            assert_eq!(
                check_command_with_rules(cmd, &[], &[], &allow),
                PermissionVerdict::Allow,
                "{cmd}"
            );
        }
    }

    #[test]
    fn a_line_continuation_does_not_hide_the_command() {
        let deny = rules(&["git push:*"]);
        assert_eq!(
            check_command_with_rules("git \\\npush", &deny, &[], &[]),
            PermissionVerdict::Deny
        );
    }

    #[test]
    fn settings_fail_closed_and_tolerate_a_bom() {
        // 1.1.0 (US-004, upstream e27b47d1 + 455ea7fa)
        let dir = tempfile::tempdir().expect("tempdir");
        let bom = dir.path().join("bom.json");
        std::fs::write(
            &bom,
            "\u{feff}{\"permissions\":{\"deny\":[\"Bash(git push:*)\"]}}",
        )
        .expect("write");
        let broken = dir.path().join("broken.json");
        std::fs::write(&broken, "{bad").expect("write");
        let empty = dir.path().join("empty.json");
        std::fs::write(&empty, "  \n").expect("write");
        let missing = dir.path().join("missing.json");
        let not_a_dir = bom.join("settings.json");

        let (deny, ask, _) = load_permission_rules_from(std::slice::from_ref(&bom));
        assert_eq!(deny, rules(&["git push:*"]));
        assert!(ask.is_empty());

        let (_, ask, _) = load_permission_rules_from(&[broken]);
        assert_eq!(
            ask,
            rules(&["*"]),
            "an unparseable file asks for everything"
        );

        let (deny, ask, allow) = load_permission_rules_from(&[empty, missing, not_a_dir]);
        assert!(deny.is_empty() && ask.is_empty() && allow.is_empty());
    }

    #[test]
    fn settings_paths_use_the_claude_config_dir() {
        let project = PathBuf::from("/workspace/project");
        let profile = PathBuf::from("/profiles/work/.claude");
        assert_eq!(
            get_settings_paths_from(Some(project.clone()), Some(profile.clone())),
            vec![
                project.join(".claude").join("settings.json"),
                project.join(".claude").join("settings.local.json"),
                profile.join("settings.json"),
                profile.join("settings.local.json"),
            ]
        );
    }
}
