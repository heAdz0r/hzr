//! The single source of truth for classifying one recorded operation.
//!
//! An operation reaches the ledger as a command string. Three questions are asked of that
//! string — did it go through the optimizer, which subsystem owns it, and (when it did
//! not) what should the agent have run instead. Those questions used to be answered in
//! three unrelated places with three different rules, which is how `hzr stats` could
//! report a healthy reduction ratio while half of the delivered tokens had bypassed the
//! optimizer entirely. Every caller now routes through this module.

use std::io::Read;
use std::path::{Path, PathBuf};

use hzr_protocol::{
    EnforcementTier, EvasionAttribution, EvasionClass, EvasionPathForm, FidelityReason,
    FidelityValidation,
};
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
    /// The redundant HZR wrapper or top-level alias being corrected.
    pub tool: &'static str,
    /// A ready-to-run HZR command preserved from the original byte slice.
    pub suggestion: String,
    /// Why the replacement is cheaper. Shown to agents, so it states the mechanism.
    pub rationale: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RawFidelityReason {
    Binary,
    Checksum,
    MachineProtocol,
    CompleteLog,
    FullPatch,
    VerbatimSource,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RawFidelityRequest<'a> {
    NotRequested,
    MissingReason,
    InvalidReason,
    Authorized {
        reason: RawFidelityReason,
        payload: &'a str,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FidelityBudget {
    pub remaining_operations: u64,
    pub remaining_tokens: u64,
    pub exhausted: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FidelityPreflight {
    NotRequested,
    Allow {
        evasion: EvasionAttribution,
        output_tokens_upper_bound: u64,
    },
    Ask {
        evasion: EvasionAttribution,
        reason: String,
    },
}

impl FidelityPreflight {
    pub const fn evasion(&self) -> Option<&EvasionAttribution> {
        match self {
            Self::NotRequested => None,
            Self::Allow { evasion, .. } | Self::Ask { evasion, .. } => Some(evasion),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct OperationClassification {
    pub route: OperationRoute,
    pub subsystem: OperationSubsystem,
    /// Short stable identity for dashboards: the tool name with the wrappers removed.
    pub operation: String,
    /// Present only when a redundant wrapper already contains a first-class HZR command.
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
            replacement: managed_hzr_replacement(command),
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

/// Remove a redundant wrapper around an existing HZR command or retain a typed HZR file alias.
/// Shell-tool routing belongs exclusively to fork-core's typed rewrite plan.
pub fn first_class_replacement(command: &str) -> Option<RawReplacement> {
    if let Some(replacement) = direct_hzr_alias_replacement(command) {
        return Some(replacement);
    }
    managed_hzr_replacement(command)
}

/// Keep the public read/write aliases on HZR's typed path when fork-core does not know
/// about the outer `hzr` control-plane command.
///
/// The command is returned unchanged instead of tokenized and reconstructed, so quoted
/// paths, write payloads, and shell grammar retain their exact spelling. The fixed exact-read
/// marker is peeled only for recognition and remains part of the returned command.
fn direct_hzr_alias_replacement(command: &str) -> Option<RawReplacement> {
    let command = command.trim_start_matches([' ', '\t']);
    let (_, candidate) = exact_fidelity_command(command);
    let arguments = command_suffix(candidate, "hzr")?.trim_start_matches([' ', '\t']);
    if !["read", "write"]
        .into_iter()
        .any(|subcommand| command_suffix(arguments, subcommand).is_some())
    {
        return None;
    }
    Some(RawReplacement {
        tool: "hzr",
        suggestion: command.to_owned(),
        rationale: "the command already uses a typed top-level HZR file operation",
    })
}

fn command_suffix<'a>(command: &'a str, word: &str) -> Option<&'a str> {
    let suffix = command.strip_prefix(word)?;
    (suffix.is_empty() || suffix.starts_with([' ', '\t'])).then_some(suffix)
}

/// Remove a redundant managed raw/proxy wrapper when its payload is already an HZR
/// command.
///
/// This route deliberately preserves the payload byte-for-byte. In particular, quoted
/// search text and shell grammar are not tokenized and reconstructed. Nested raw/proxy
/// wrappers are excluded so the replacement cannot merely hide another bypass.
fn managed_hzr_replacement(command: &str) -> Option<RawReplacement> {
    let payload = match raw_fidelity_request(command) {
        RawFidelityRequest::Authorized { payload, .. } => payload,
        RawFidelityRequest::NotRequested
        | RawFidelityRequest::MissingReason
        | RawFidelityRequest::InvalidReason => managed_raw_payload(command)?,
    }
    .trim_start_matches([' ', '\t']);
    let suffix = payload.strip_prefix("hzr")?;
    if !suffix.is_empty() && !suffix.starts_with([' ', '\t']) {
        return None;
    }
    if managed_raw_payload(payload).is_some() {
        return None;
    }
    Some(RawReplacement {
        tool: "hzr",
        suggestion: payload.to_owned(),
        rationale: "the payload is already a first-class HZR command and needs no raw proxy",
    })
}

/// Return a lower-output route for an already managed command when the requested fidelity
/// is unbounded and the existing first-class default is sufficient.
///
/// Exact ranges, numbered reads, bounded heads/tails, and structural modes already carry
/// evidence for their fidelity or scope. Only a bare full-file `--level none` is reduced.
pub fn efficient_route_replacement(command: &str) -> Option<RawReplacement> {
    let (exact_fidelity, command) = exact_fidelity_command(command);
    if exact_fidelity || !unambiguous_shell_command(command) {
        return None;
    }
    let words = shell_words(command);
    let (route, payload) = strip_bypass_prefix(&words);
    let payload = match route {
        OperationRoute::Bypassed => payload,
        OperationRoute::Optimized => strip_wrappers(payload),
        OperationRoute::NativeUnaccounted => payload,
    };
    if payload.first().map(String::as_str) != Some("read") {
        return None;
    }
    unbounded_exact_read_replacement(&payload[1..])
}

fn exact_fidelity_command(command: &str) -> (bool, &str) {
    let command = command.trim_start_matches([' ', '\t']);
    let Some(remainder) = command.strip_prefix("HZR_EXACT_FIDELITY=") else {
        return (false, command);
    };
    let Some(boundary) = remainder.find([' ', '\t']) else {
        return (false, command);
    };
    let (value, payload) = remainder.split_at(boundary);
    (value == "1", payload.trim_start_matches([' ', '\t']))
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
    matches!(
        raw_fidelity_request(command),
        RawFidelityRequest::Authorized { .. }
    )
}

pub fn raw_fidelity_request(command: &str) -> RawFidelityRequest<'_> {
    let mut rest = command.trim_start_matches([' ', '\t']);
    let mut requested = false;
    let mut reason = None;
    let mut invalid_reason = false;

    loop {
        let boundary = rest.find([' ', '\t']).unwrap_or(rest.len());
        let word = &rest[..boundary];
        let next = rest[boundary..].trim_start_matches([' ', '\t']);
        if word == "HZR_RAW_FIDELITY=1" {
            requested = true;
        } else if let Some(value) = word.strip_prefix("HZR_RAW_FIDELITY_REASON=") {
            match parse_raw_fidelity_reason(value) {
                Some(value) if reason.is_none() => reason = Some(value),
                _ => invalid_reason = true,
            }
        } else {
            break;
        }
        rest = next;
    }

    let Some(payload) = managed_raw_payload(rest) else {
        return RawFidelityRequest::NotRequested;
    };
    if !requested {
        return RawFidelityRequest::NotRequested;
    }
    if invalid_reason {
        return RawFidelityRequest::InvalidReason;
    }
    let Some(reason) = reason else {
        return RawFidelityRequest::MissingReason;
    };
    RawFidelityRequest::Authorized { reason, payload }
}

fn parse_raw_fidelity_reason(value: &str) -> Option<RawFidelityReason> {
    match value {
        "binary" => Some(RawFidelityReason::Binary),
        "checksum" => Some(RawFidelityReason::Checksum),
        "machine_protocol" => Some(RawFidelityReason::MachineProtocol),
        "complete_log" => Some(RawFidelityReason::CompleteLog),
        "full_patch" => Some(RawFidelityReason::FullPatch),
        "verbatim_source" => Some(RawFidelityReason::VerbatimSource),
        _ => None,
    }
}

pub fn fidelity_preflight_required(command: &str) -> bool {
    !matches!(
        raw_fidelity_request(command),
        RawFidelityRequest::NotRequested
    ) || exact_fidelity_command(command).0
}

pub fn fidelity_preflight(
    command: &str,
    cwd: &Path,
    budget: Option<FidelityBudget>,
) -> FidelityPreflight {
    let request = raw_fidelity_request(command);
    let (reason, payload, mut evasion) = match request {
        RawFidelityRequest::Authorized { reason, payload } => (
            reason,
            payload,
            fidelity_evasion(reason, FidelityValidation::Valid, false),
        ),
        RawFidelityRequest::MissingReason => {
            return FidelityPreflight::Ask {
                evasion: fidelity_evasion_without_reason(FidelityValidation::MissingReason),
                reason: "HZR_RAW_FIDELITY=1 requires a closed HZR_RAW_FIDELITY_REASON".into(),
            };
        }
        RawFidelityRequest::InvalidReason => {
            return FidelityPreflight::Ask {
                evasion: fidelity_evasion_without_reason(FidelityValidation::InvalidReason),
                reason: "HZR_RAW_FIDELITY_REASON is not an allowed fidelity reason".into(),
            };
        }
        RawFidelityRequest::NotRequested => {
            let (exact, payload) = exact_fidelity_command(command);
            if !exact {
                return FidelityPreflight::NotRequested;
            }
            (
                RawFidelityReason::VerbatimSource,
                payload,
                fidelity_evasion(
                    RawFidelityReason::VerbatimSource,
                    FidelityValidation::Valid,
                    false,
                ),
            )
        }
    };
    if !raw_fidelity_reason_fits(reason, payload, cwd) {
        evasion.avoidable = true;
        evasion.fidelity_validation = FidelityValidation::Contradicted;
        return FidelityPreflight::Ask {
            evasion,
            reason: "T4 fidelity reason is contradicted by the requested command".into(),
        };
    }
    let Some(output_tokens_upper_bound) = fidelity_output_tokens_upper_bound(reason, payload, cwd)
    else {
        return FidelityPreflight::Ask {
            evasion,
            reason: "T4 exact output is remote, non-file, or otherwise not statically bounded"
                .into(),
        };
    };
    let Some(budget) = budget else {
        return FidelityPreflight::Ask {
            evasion,
            reason: "T4 fidelity preflight cannot audit the per-session allowance".into(),
        };
    };
    if budget.exhausted
        || budget.remaining_operations == 0
        || output_tokens_upper_bound > budget.remaining_tokens
    {
        evasion.avoidable = true;
        evasion.fidelity_validation = FidelityValidation::BudgetExhausted;
        return FidelityPreflight::Ask {
            evasion,
            reason: format!(
                "T4 exact output upper bound is {output_tokens_upper_bound} tokens; remaining allowance is {} operation(s) and {} tokens",
                budget.remaining_operations, budget.remaining_tokens
            ),
        };
    }
    FidelityPreflight::Allow {
        evasion,
        output_tokens_upper_bound,
    }
}

fn fidelity_evasion(
    reason: RawFidelityReason,
    validation: FidelityValidation,
    avoidable: bool,
) -> EvasionAttribution {
    EvasionAttribution {
        class: EvasionClass::E7FidelityHatch,
        wrapper_depth: 1,
        interpreter: None,
        path_form: EvasionPathForm::Bare,
        stage_count: 1,
        hatch_marker: true,
        avoidable,
        tier: EnforcementTier::T4HatchQuarantine,
        fidelity_reason: Some(protocol_fidelity_reason(reason)),
        fidelity_validation: validation,
    }
}

fn fidelity_evasion_without_reason(validation: FidelityValidation) -> EvasionAttribution {
    EvasionAttribution {
        class: EvasionClass::E7FidelityHatch,
        wrapper_depth: 1,
        interpreter: None,
        path_form: EvasionPathForm::Bare,
        stage_count: 1,
        hatch_marker: true,
        avoidable: true,
        tier: EnforcementTier::T4HatchQuarantine,
        fidelity_reason: None,
        fidelity_validation: validation,
    }
}

fn protocol_fidelity_reason(reason: RawFidelityReason) -> FidelityReason {
    match reason {
        RawFidelityReason::Binary => FidelityReason::Binary,
        RawFidelityReason::Checksum => FidelityReason::Checksum,
        RawFidelityReason::MachineProtocol => FidelityReason::MachineProtocol,
        RawFidelityReason::CompleteLog => FidelityReason::CompleteLog,
        RawFidelityReason::FullPatch => FidelityReason::FullPatch,
        RawFidelityReason::VerbatimSource => FidelityReason::VerbatimSource,
    }
}

fn raw_fidelity_reason_fits(reason: RawFidelityReason, payload: &str, cwd: &Path) -> bool {
    let words = shell_words(payload);
    let payload_words = strip_wrappers(&words);
    let tool = payload_words.first().map(String::as_str);
    match reason {
        RawFidelityReason::Checksum => {
            unambiguous_shell_command(payload)
                && (matches!(
                    tool,
                    Some(
                        "sha256sum" | "sha512sum" | "shasum" | "md5sum" | "md5" | "b2sum" | "cksum"
                    )
                ) || tool == Some("openssl")
                    && payload_words.get(1).map(String::as_str) == Some("dgst"))
        }
        RawFidelityReason::MachineProtocol => payload_words.iter().any(|word| {
            matches!(
                word.as_str(),
                "--json" | "--csv" | "--porcelain" | "-0" | "--null"
            )
        }),
        RawFidelityReason::CompleteLog => payload_words
            .iter()
            .any(|word| matches!(word.as_str(), "log" | "logs")),
        RawFidelityReason::FullPatch => payload_words
            .iter()
            .any(|word| matches!(word.as_str(), "diff" | "patch")),
        RawFidelityReason::Binary => {
            matches!(tool, Some("file" | "xxd" | "hexdump" | "base64"))
                || local_reader_is_binary(payload, cwd)
        }
        RawFidelityReason::VerbatimSource => {
            local_reader_output_upper_bound(payload, cwd).is_some()
        }
    }
}

fn fidelity_output_tokens_upper_bound(
    reason: RawFidelityReason,
    payload: &str,
    cwd: &Path,
) -> Option<u64> {
    let bytes = match reason {
        RawFidelityReason::Checksum => checksum_output_upper_bound(payload)?,
        RawFidelityReason::Binary | RawFidelityReason::VerbatimSource => {
            local_reader_output_upper_bound(payload, cwd)?
        }
        RawFidelityReason::MachineProtocol
        | RawFidelityReason::CompleteLog
        | RawFidelityReason::FullPatch => return None,
    };
    Some(bytes.saturating_add(3) / 4)
}

fn checksum_output_upper_bound(payload: &str) -> Option<u64> {
    if !unambiguous_shell_command(payload) {
        return None;
    }
    let words = shell_words(payload);
    let payload_words = strip_wrappers(&words);
    let arguments = payload_words.get(1..)?;
    let targets = arguments
        .iter()
        .filter(|word| !word.starts_with('-') && word.as_str() != "dgst")
        .count()
        .max(1);
    let targets = u64::try_from(targets).ok()?;
    Some(
        u64::try_from(payload.len())
            .ok()?
            .saturating_add(targets.saturating_mul(160)),
    )
}

fn local_reader_output_upper_bound(payload: &str, cwd: &Path) -> Option<u64> {
    local_reader_bound(payload, cwd).map(|bound| bound.bytes)
}

struct LocalReaderBound {
    path: PathBuf,
    bytes: u64,
}

fn local_reader_bound(payload: &str, cwd: &Path) -> Option<LocalReaderBound> {
    if !unambiguous_shell_command(payload) {
        return None;
    }
    let words = shell_words(payload);
    let payload_words = strip_wrappers(&words);
    let tool = payload_words.first()?.as_str();
    if !matches!(tool, "cat" | "read" | "head" | "tail") {
        return None;
    }
    let words = payload_words.get(1..)?;
    let mut byte_cap = None;
    let mut path = None;
    let mut index = 0;
    while index < words.len() {
        let word = words[index].as_str();
        if matches!(word, "--level" | "--from" | "--to") {
            index = index.checked_add(2)?;
            continue;
        }
        if word == "-n" {
            index = if matches!(tool, "head" | "tail") {
                index.checked_add(2)?
            } else {
                index + 1
            };
            continue;
        }
        if word == "-c" || word == "--bytes" {
            byte_cap = Some(words.get(index + 1)?.parse::<u64>().ok()?);
            index += 2;
            continue;
        }
        if let Some(value) = word.strip_prefix("--bytes=") {
            byte_cap = Some(value.parse::<u64>().ok()?);
            index += 1;
            continue;
        }
        if word.starts_with('-') || path.replace(word).is_some() {
            return None;
        }
        index += 1;
    }
    let path = Path::new(path?);
    let path: PathBuf = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    let metadata = path.metadata().ok()?;
    metadata.is_file().then(|| LocalReaderBound {
        path,
        bytes: byte_cap.map_or(metadata.len(), |cap| metadata.len().min(cap)),
    })
}

fn local_reader_is_binary(payload: &str, cwd: &Path) -> bool {
    let Some(bound) = local_reader_bound(payload, cwd) else {
        return false;
    };
    let sample_bytes = bound.bytes.min(8 * 1024);
    let Ok(sample_len) = usize::try_from(sample_bytes) else {
        return false;
    };
    let mut sample = vec![0; sample_len];
    let Ok(mut file) = std::fs::File::open(bound.path) else {
        return false;
    };
    let Ok(read) = file.read(&mut sample) else {
        return false;
    };
    sample[..read].contains(&0)
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

const SMART_READ_RATIONALE: &str =
    "hzr read selects its format-aware bounded default instead of an unbounded full-file read";

fn unbounded_exact_read_replacement(arguments: &[String]) -> Option<RawReplacement> {
    let mut file = None;
    let mut exact = false;
    let mut expect_level = false;
    for argument in arguments {
        if expect_level {
            if argument != "none" {
                return None;
            }
            exact = true;
            expect_level = false;
            continue;
        }
        match argument.as_str() {
            "--level" | "-l" => {
                if exact {
                    return None;
                }
                expect_level = true;
            }
            "--level=none" | "-lnone" => {
                if exact {
                    return None;
                }
                exact = true;
            }
            _ if argument.starts_with('-') => return None,
            _ if file.replace(argument).is_some() => return None,
            _ => {}
        }
    }
    if expect_level || !exact {
        return None;
    }
    let file = file?;
    Some(RawReplacement {
        tool: "read",
        suggestion: format!("hzr rtk -- read {file}"),
        rationale: SMART_READ_RATIONALE,
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
    fn test_shell_tools_are_classified_without_a_second_rewrite_authority() {
        let classification = classify_operation("rtk proxy head README.md");
        assert_eq!(classification.replacement, None);
    }

    #[test]
    fn acceptance_gate_fidelity_checksum_is_bounded_and_budgeted() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let command = "HZR_RAW_FIDELITY=1 HZR_RAW_FIDELITY_REASON=checksum hzr rtk -- raw sha256sum artifact.bin";
        let available = FidelityBudget {
            remaining_operations: 1,
            remaining_tokens: 100_000,
            exhausted: false,
        };
        let preflight = fidelity_preflight(command, directory.path(), Some(available));
        assert!(matches!(preflight, FidelityPreflight::Allow { .. }));
        let FidelityPreflight::Allow {
            evasion,
            output_tokens_upper_bound,
        } = preflight
        else {
            return;
        };
        assert_eq!(evasion.fidelity_reason, Some(FidelityReason::Checksum));
        assert_eq!(evasion.fidelity_validation, FidelityValidation::Valid);
        assert!(output_tokens_upper_bound < 100);

        let exhausted = FidelityBudget {
            remaining_operations: 0,
            remaining_tokens: 100_000,
            exhausted: true,
        };
        assert!(matches!(
            fidelity_preflight(command, directory.path(), Some(exhausted)),
            FidelityPreflight::Ask {
                evasion: EvasionAttribution {
                    fidelity_validation: FidelityValidation::BudgetExhausted,
                    ..
                },
                ..
            }
        ));
    }

    #[test]
    fn acceptance_gate_fidelity_reason_mismatch_is_contradicted() {
        let directory = tempfile::tempdir().expect("temporary directory");
        std::fs::write(directory.path().join("artifact.txt"), b"source").expect("source fixture");
        let budget = FidelityBudget {
            remaining_operations: 5,
            remaining_tokens: 100_000,
            exhausted: false,
        };
        for command in [
            "HZR_RAW_FIDELITY=1 HZR_RAW_FIDELITY_REASON=checksum hzr rtk -- raw cat artifact.txt",
            "HZR_RAW_FIDELITY=1 HZR_RAW_FIDELITY_REASON=binary hzr rtk -- raw cat artifact.txt",
        ] {
            assert!(matches!(
                fidelity_preflight(command, directory.path(), Some(budget)),
                FidelityPreflight::Ask {
                    evasion: EvasionAttribution {
                        fidelity_validation: FidelityValidation::Contradicted,
                        avoidable: true,
                        ..
                    },
                    ..
                }
            ));
        }
    }

    #[test]
    fn acceptance_gate_exact_reader_checks_first_use_token_bound() {
        let directory = tempfile::tempdir().expect("temporary directory");
        std::fs::write(directory.path().join("oversized.txt"), vec![b'x'; 400_001])
            .expect("oversized fixture");
        assert!(matches!(
            fidelity_preflight(
                "HZR_EXACT_FIDELITY=1 hzr read oversized.txt --level none",
                directory.path(),
                Some(FidelityBudget {
                    remaining_operations: 5,
                    remaining_tokens: 100_000,
                    exhausted: false,
                }),
            ),
            FidelityPreflight::Ask {
                evasion: EvasionAttribution {
                    fidelity_validation: FidelityValidation::BudgetExhausted,
                    ..
                },
                ..
            }
        ));
    }
}
