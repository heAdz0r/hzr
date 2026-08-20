//! The single source of truth for classifying one recorded operation.
//!
//! An operation reaches the ledger as a command string. Three questions are asked of that
//! string — did it go through the optimizer, which subsystem owns it, and (when it did
//! not) what should the agent have run instead. Those questions used to be answered in
//! three unrelated places with three different rules, which is how `hzr stats` could
//! report a healthy reduction ratio while half of the delivered tokens had bypassed the
//! optimizer entirely. Every caller now routes through this module.

use serde::{Deserialize, Serialize};

/// The words that mean "HZR handed this straight to the shell".
const BYPASS_MARKERS: [&str; 2] = ["raw", "proxy"];

/// Command prefixes that mean the same thing, spelled the way the ledger records them.
///
/// Both the Rust classifier and the SQL predicate are generated from these, so the
/// terminal, the dashboard and the ledger cannot drift apart again.
const BYPASS_PREFIXES: [&str; 14] = [
    "raw",
    "proxy",
    "rtk raw",
    "rtk proxy",
    "hzr raw",
    "hzr proxy",
    "rtk -- raw",
    "rtk -- proxy",
    "hzr -- raw",
    "hzr -- proxy",
    "hzr rtk raw",
    "hzr rtk proxy",
    "hzr rtk -- raw",
    "hzr rtk -- proxy",
];

/// Prefixes recorded when the pinned engine degraded into a shell fallback. These are
/// matched without a trailing separator because fork-core writes `rtk fallback: <cmd>`.
const BYPASS_FALLBACK_PREFIXES: [&str; 2] = ["rtk fallback", "hzr fallback"];

/// Wrapper words that carry no meaning of their own.
const WRAPPERS: [&str; 3] = ["rtk", "hzr", "--"];

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationRoute {
    /// The command was rewritten by HZR and its output was filtered.
    Optimized,
    /// The command reached the shell unfiltered. Delivered tokens equal baseline tokens.
    /// The wire name stays `raw` because the visualizer and the stored dashboard payloads
    /// already speak it.
    #[serde(rename = "raw")]
    Bypassed,
    /// A host-native tool was observed after execution. It contributes to coverage but
    /// never to the optimizer reduction ratio because HZR did not transform its output.
    NativeUnaccounted,
}

impl OperationRoute {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Optimized => "optimized",
            Self::Bypassed => "bypassed",
            Self::NativeUnaccounted => "native_unaccounted",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationChannel {
    HookCli,
    Mcp,
    NativeHost,
}

impl OperationChannel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::HookCli => "hook_cli",
            Self::Mcp => "mcp",
            Self::NativeHost => "native_host",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationMeasurement {
    Estimated,
    Unmeasured,
}

impl OperationMeasurement {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Estimated => "estimated",
            Self::Unmeasured => "unmeasured",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationSubsystem {
    Read,
    Search,
    Write,
    Memory,
    Codec,
    Execution,
    /// Every bypassed operation, regardless of the tool underneath. Keeping bypasses in
    /// their own bucket is the whole point: folding them into `Execution` is what hid
    /// them.
    Bypass,
}

impl OperationSubsystem {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Search => "search",
            Self::Write => "write",
            Self::Memory => "memory",
            Self::Codec => "codec",
            Self::Execution => "execution",
            Self::Bypass => "bypass",
        }
    }
}

/// The first-class HZR command an agent should have reached for.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RawReplacement {
    /// The shell tool that was invoked instead (`sed`, `rg`, `cat`, …).
    pub tool: &'static str,
    /// A ready-to-run replacement, reconstructed from the bypassed arguments.
    pub suggestion: String,
    /// Why the replacement is cheaper. Shown to agents, so it states the mechanism.
    pub rationale: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OperationClassification {
    pub route: OperationRoute,
    pub subsystem: OperationSubsystem,
    /// Short stable identity for dashboards: the tool name with the wrappers removed.
    pub operation: String,
    /// Present only when the bypassed tool has a first-class HZR equivalent.
    pub replacement: Option<RawReplacement>,
}

/// Classify one recorded command.
pub fn classify_operation(command: &str) -> OperationClassification {
    let words = shell_words(command);
    let (route, payload) = strip_bypass_prefix(&words);
    let payload = match route {
        OperationRoute::Bypassed => payload,
        OperationRoute::Optimized => strip_wrappers(payload),
        OperationRoute::NativeUnaccounted => payload,
    };
    let head = payload.first().map(String::as_str).unwrap_or_default();
    let operation = operation_identity(head);
    match route {
        OperationRoute::Bypassed => OperationClassification {
            route,
            subsystem: OperationSubsystem::Bypass,
            replacement: unambiguous_shell_command(command)
                .then(|| replacement_for(head, &payload[payload.len().min(1)..]))
                .flatten(),
            operation,
        },
        OperationRoute::Optimized => OperationClassification {
            route,
            subsystem: optimized_subsystem(head),
            operation,
            replacement: None,
        },
        OperationRoute::NativeUnaccounted => unreachable!("command classification is not native"),
    }
}

/// The first-class HZR command that replaces `command`, if one exists.
///
/// Unlike [`classify_operation`] this answers for any shell command, whether or not it
/// carries a bypass prefix — the hook sees `sed -n 1,20p f` and the ledger sees
/// `rtk proxy sed -n 1,20p f`, and both must be told the same thing.
pub fn first_class_replacement(command: &str) -> Option<RawReplacement> {
    if !unambiguous_shell_command(command) {
        return None;
    }
    let words = shell_words(command);
    let (route, payload) = strip_bypass_prefix(&words);
    let payload = match route {
        OperationRoute::Bypassed => payload,
        OperationRoute::Optimized => strip_wrappers(payload),
        OperationRoute::NativeUnaccounted => payload,
    };
    let head = payload.first().map(String::as_str)?;
    replacement_for(head, &payload[1..])
}

/// Return an explicit managed raw/proxy payload without reparsing or reconstructing it.
///
/// Fork-core gets one more chance to apply its typed command families to this exact byte
/// slice. Commands without a recognized HZR/RTK wrapper are not candidates for a retry.
pub fn managed_raw_payload(command: &str) -> Option<&str> {
    let command = command.trim_start_matches([' ', '\t']);
    for prefix in BYPASS_PREFIXES {
        let Some(remainder) = command.strip_prefix(prefix) else {
            continue;
        };
        if !remainder.starts_with([' ', '\t']) {
            continue;
        }
        let payload = &remainder[1..];
        if !payload.trim().is_empty() {
            return Some(payload);
        }
    }
    None
}

/// Return whether `command` explicitly requests unfiltered fidelity.
///
/// Managed agent wrappers are normally removed before fork-core policy runs. The fixed
/// environment prefix is the deliberate exception for checksums, parsers, and other tasks
/// where filtered output is not an effective route. Requiring a separate marker prevents an
/// agent's habitual `raw` wrapper from silently opting out of the acceptance gate.
pub fn explicit_raw_fidelity(command: &str) -> bool {
    let command = command.trim_start_matches([' ', '\t']);
    let Some(remainder) = command.strip_prefix("HZR_RAW_FIDELITY=1") else {
        return false;
    };
    if !remainder.starts_with([' ', '\t']) {
        return false;
    }
    managed_raw_payload(remainder.trim_start_matches([' ', '\t'])).is_some()
}

/// A SQL `WHERE` fragment selecting the bypassed rows of `column`, generated from the
/// same prefix lists the Rust classifier uses.
pub fn raw_route_sql_predicate(column: &str) -> String {
    let mut clauses =
        Vec::with_capacity(BYPASS_PREFIXES.len() * 2 + BYPASS_FALLBACK_PREFIXES.len());
    for prefix in BYPASS_PREFIXES {
        clauses.push(format!("{column} = '{prefix}'"));
        clauses.push(format!("{column} LIKE '{prefix} %'"));
    }
    for prefix in BYPASS_FALLBACK_PREFIXES {
        clauses.push(format!("{column} LIKE '{prefix}%'"));
    }
    clauses.join(" OR ")
}

fn shell_words(command: &str) -> Vec<String> {
    command
        .split_whitespace()
        .map(|word| word.trim_matches(['\'', '"']).to_owned())
        .collect()
}

fn unambiguous_shell_command(command: &str) -> bool {
    !command.chars().any(|character| {
        matches!(
            character,
            '\'' | '"'
                | '\\'
                | '`'
                | '$'
                | '|'
                | '&'
                | ';'
                | '<'
                | '>'
                | '*'
                | '?'
                | '['
                | ']'
                | '('
                | ')'
                | '{'
                | '}'
                | '!'
                | '#'
                | '\n'
                | '\r'
        )
    })
}

/// Split a command into "did it bypass the optimizer" and the payload that follows.
///
/// The recorded forms (`rtk proxy …`, `raw …`) and the typed forms (`hzr rtk -- raw …`)
/// differ only in how many wrapper words precede the marker, so the scanner skips wrappers
/// rather than enumerating every spelling. [`BYPASS_PREFIXES`] remains the authority for
/// what a marker *is*, and generates the SQL predicate from the same words.
fn strip_bypass_prefix(words: &[String]) -> (OperationRoute, &[String]) {
    for prefix in BYPASS_FALLBACK_PREFIXES {
        if words
            .first()
            .zip(prefix.split(' ').next())
            .is_some_and(|(word, expected)| word == expected)
            && words
                .get(1)
                .zip(prefix.split(' ').nth(1))
                .is_some_and(|(word, expected)| word.trim_end_matches(':') == expected)
        {
            return (OperationRoute::Bypassed, &words[words.len().min(2)..]);
        }
    }
    let mut index = 0;
    while index < words.len() && WRAPPERS.contains(&words[index].as_str()) {
        index += 1;
    }
    if words
        .get(index)
        .is_some_and(|word| BYPASS_MARKERS.contains(&word.as_str()))
    {
        return (OperationRoute::Bypassed, &words[index + 1..]);
    }
    (OperationRoute::Optimized, words)
}

fn strip_wrappers(words: &[String]) -> &[String] {
    let mut index = 0;
    while index < words.len() && WRAPPERS.contains(&words[index].as_str()) {
        index += 1;
    }
    &words[index..]
}

fn operation_identity(head: &str) -> String {
    let identity = head
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
        .take(32)
        .collect::<String>();
    if identity.is_empty() {
        "operation".to_owned()
    } else {
        identity
    }
}

fn optimized_subsystem(head: &str) -> OperationSubsystem {
    match head {
        "read" | "cat" | "head" | "tail" | "nl" => OperationSubsystem::Read,
        "write" | "edit" | "patch" | "replace" => OperationSubsystem::Write,
        "grep" | "rg" | "rgai" | "search" | "find" | "glob" => OperationSubsystem::Search,
        "memory" | "recall" | "store" => OperationSubsystem::Memory,
        "codec" => OperationSubsystem::Codec,
        _ => OperationSubsystem::Execution,
    }
}

fn replacement_for(tool: &str, arguments: &[String]) -> Option<RawReplacement> {
    match tool {
        "sed" => sed_replacement(arguments),
        "rg" => search_replacement("rg", arguments),
        "grep" => search_replacement("grep", arguments),
        "ag" => search_replacement("ag", arguments),
        "ack" => search_replacement("ack", arguments),
        "cat" => file_replacement("cat", arguments, " --level none"),
        "nl" => nl_replacement(arguments),
        "head" => bounded_replacement("head", arguments, "--max-lines"),
        "tail" => bounded_replacement("tail", arguments, "--tail-lines"),
        _ => None,
    }
}

const READ_RATIONALE: &str =
    "hzr read streams the requested span with filtering instead of the whole slice";
const SEARCH_RATIONALE: &str =
    "hzr search returns ranked matches through the shared index instead of raw output";

fn sed_replacement(arguments: &[String]) -> Option<RawReplacement> {
    let mut quiet = false;
    let mut span = None;
    let mut file = None;
    let mut expect_expression = false;
    let mut parse_options = true;
    for argument in arguments {
        if expect_expression {
            span = Some(parse_sed_span(argument)?);
            expect_expression = false;
            continue;
        }
        if parse_options {
            match argument.as_str() {
                "--" => {
                    parse_options = false;
                    continue;
                }
                "-n" | "--quiet" | "--silent" => {
                    quiet = true;
                    continue;
                }
                "-e" | "--expression" => {
                    if span.is_some() {
                        return None;
                    }
                    expect_expression = true;
                    continue;
                }
                _ if argument.starts_with('-') => return None,
                _ => {}
            }
        }
        if span.is_none() {
            span = Some(parse_sed_span(argument)?);
        } else if file.is_none() && argument != "-" {
            file = Some(argument.clone());
        } else {
            return None;
        }
    }
    if expect_expression || !quiet {
        return None;
    }
    let file = file?;
    let (from, to) = span?;
    Some(RawReplacement {
        tool: "sed",
        suggestion: format!("hzr rtk -- read {file} --from {from} --to {to}"),
        rationale: READ_RATIONALE,
    })
}

fn parse_sed_span(argument: &str) -> Option<(u64, u64)> {
    let body = argument.strip_suffix('p')?;
    let (from, to) = match body.split_once(',') {
        Some((from, to)) => (from.parse().ok()?, to.parse().ok()?),
        None => {
            let line = body.parse().ok()?;
            (line, line)
        }
    };
    (from > 0 && to >= from).then_some((from, to))
}

fn search_replacement(tool: &'static str, arguments: &[String]) -> Option<RawReplacement> {
    let mut pattern = None;
    let mut paths = Vec::new();
    let mut parse_options = true;
    for argument in arguments {
        if parse_options && argument == "--" {
            parse_options = false;
            continue;
        }
        if parse_options && argument.starts_with('-') {
            if matches!(argument.as_str(), "-n" | "--line-number") {
                continue;
            }
            return None;
        }
        if pattern.is_none() {
            if argument.starts_with('-') || !literal_search_pattern(argument) {
                return None;
            }
            pattern = Some(argument.clone());
        } else if argument.starts_with('-') {
            return None;
        } else {
            paths.push(argument.clone());
        }
    }
    let pattern = pattern?;
    let mut suggestion = format!("hzr search '{pattern}' --mode exact");
    for path in paths {
        suggestion.push_str(&format!(" --path {path}"));
    }
    Some(RawReplacement {
        tool,
        suggestion,
        rationale: SEARCH_RATIONALE,
    })
}

fn literal_search_pattern(pattern: &str) -> bool {
    !pattern.is_empty()
        && !pattern.chars().any(|character| {
            matches!(
                character,
                '.' | '^' | '$' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '\\'
            )
        })
}

fn file_replacement(
    tool: &'static str,
    arguments: &[String],
    suffix: &str,
) -> Option<RawReplacement> {
    let mut file = None;
    let mut parse_options = true;
    for argument in arguments {
        if parse_options && argument == "--" {
            parse_options = false;
            continue;
        }
        if parse_options && argument.starts_with('-') {
            return None;
        }
        if argument == "-" || argument.starts_with('-') || file.replace(argument).is_some() {
            return None;
        }
    }
    let file = file?;
    Some(RawReplacement {
        tool,
        suggestion: format!("hzr rtk -- read {file}{suffix}"),
        rationale: READ_RATIONALE,
    })
}

fn nl_replacement(arguments: &[String]) -> Option<RawReplacement> {
    let mut number_all = false;
    let mut expect_body_style = false;
    let mut file = None;
    let mut parse_options = true;
    for argument in arguments {
        if expect_body_style {
            if argument != "a" {
                return None;
            }
            number_all = true;
            expect_body_style = false;
            continue;
        }
        if parse_options {
            match argument.as_str() {
                "--" => {
                    parse_options = false;
                    continue;
                }
                "-ba" | "--body-numbering=a" => {
                    number_all = true;
                    continue;
                }
                "-b" | "--body-numbering" => {
                    expect_body_style = true;
                    continue;
                }
                _ if argument.starts_with('-') => return None,
                _ => {}
            }
        }
        if argument == "-" || argument.starts_with('-') || file.replace(argument).is_some() {
            return None;
        }
    }
    if expect_body_style || !number_all {
        return None;
    }
    let file = file?;
    Some(RawReplacement {
        tool: "nl",
        suggestion: format!("hzr rtk -- read {file} -n"),
        rationale: READ_RATIONALE,
    })
}

fn bounded_replacement(
    tool: &'static str,
    arguments: &[String],
    flag: &str,
) -> Option<RawReplacement> {
    let mut lines = None;
    let mut file = None;
    let mut expect_lines = false;
    let mut parse_options = true;
    for argument in arguments {
        if expect_lines {
            expect_lines = false;
            let value = argument.parse::<u64>().ok()?;
            if value == 0 {
                return None;
            }
            lines = Some(value);
            continue;
        }
        if parse_options && argument == "--" {
            parse_options = false;
            continue;
        }
        if parse_options && argument.starts_with('-') {
            if argument == "-n" || argument == "--lines" {
                expect_lines = true;
                continue;
            }
            let inline = argument
                .strip_prefix("-n")
                .or_else(|| argument.strip_prefix("--lines="))?;
            let value = inline.parse::<u64>().ok()?;
            if value == 0 {
                return None;
            }
            lines = Some(value);
            continue;
        }
        if argument == "-" || argument.starts_with('-') || file.replace(argument).is_some() {
            return None;
        }
    }
    if expect_lines {
        return None;
    }
    let file = file?;
    let lines = lines.unwrap_or(10);
    Some(RawReplacement {
        tool,
        suggestion: format!("hzr rtk -- read {file} {flag} {lines}"),
        rationale: READ_RATIONALE,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wrapped_bypass_separator_is_recognized() {
        let classification = classify_operation("rtk -- raw cargo test");

        assert_eq!(classification.route, OperationRoute::Bypassed);
        assert_eq!(classification.operation, "cargo");
    }

    #[test]
    fn test_search_semantic_flags_are_not_reconstructed() {
        let classification = classify_operation("rtk proxy rg -n -C 5 needle src");
        assert_eq!(classification.replacement, None);
    }

    #[test]
    fn test_head_defaults_to_ten_lines_like_the_shell() {
        let classification = classify_operation("rtk proxy head README.md");
        let replacement = classification.replacement.expect("head replacement");

        assert_eq!(
            replacement.suggestion,
            "hzr rtk -- read README.md --max-lines 10"
        );
    }
}
