//! The single source of truth for "did this operation go through the optimizer?".
//!
//! Before this module existed the answer was computed in three places with three
//! different rules: `operation_identity` in the ledger, a `LIKE` predicate in SQL, and
//! `classify_command` in the CLI. They disagreed, and the disagreement is precisely why
//! `hzr stats` reported 87% savings while half of the delivered tokens had bypassed the
//! optimizer entirely.

use hzr_core::{
    OperationRoute, OperationSubsystem, RawFidelityReason, RawFidelityRequest, classify_operation,
    efficient_route_replacement, explicit_raw_fidelity, first_class_replacement,
    managed_raw_payload, raw_fidelity_request, raw_route_sql_predicate,
};

/// The hook needs the same answer for a command the agent typed directly, before any
/// bypass prefix exists. Deriving it from a second rule is how the two drifted apart the
/// first time, so both callers ask this one function.
#[test]
fn test_plain_shell_commands_have_no_second_rewrite_authority() {
    for command in [
        "sed -n 1030,1105p crates/hzr-core/src/ledger.rs",
        "hzr rtk -- raw sed -n 1030,1105p crates/hzr-core/src/ledger.rs",
        "nl -ba src/main.rs",
        "rg -n needle src",
        "cat README.md",
        "head README.md",
        "tail README.md",
    ] {
        assert_eq!(
            first_class_replacement(command),
            None,
            "shell policy must come only from the typed fork plan: {command}"
        );
    }
}

/// Steering must stay silent where raw is the correct tool, otherwise every build turns
/// into a permission prompt and the signal is ignored.
#[test]
fn test_commands_without_an_equivalent_are_left_alone() {
    assert_eq!(first_class_replacement("cargo clippy --workspace"), None);
    assert_eq!(first_class_replacement("git commit -m wip"), None);
    assert_eq!(
        first_class_replacement("sed -i '' s/a/b/ file.rs"),
        None,
        "an in-place edit is not a read and hzr read cannot replace it"
    );
    assert_eq!(first_class_replacement(""), None);
}

#[test]
fn acceptance_gate_private_ledger_sqlite_is_left_to_the_typed_fork_plan() {
    for command in [
        "sqlite3 ledger/hzr.sqlite 'select * from operations'",
        "hzr rtk -- raw /usr/bin/sqlite3 /tmp/ledger/hzr.sqlite 'select command from operations'",
    ] {
        assert_eq!(
            first_class_replacement(command),
            None,
            "core must not override the canonical E9 decision"
        );
    }

    assert_eq!(
        first_class_replacement("sqlite3 /tmp/application.db 'select 1'"),
        None,
        "generic SQLite remains a genuine no-equivalent fallback"
    );
}

#[test]
fn acceptance_gate_no_unbounded_exact_read_defaults() {
    for command in [
        "hzr read src/main.rs --level none",
        "hzr rtk -- read src/main.rs --level none",
        "rtk read README.md -l none",
        "hzr rtk -- raw read src/lib.rs --level=none",
    ] {
        let replacement = efficient_route_replacement(command)
            .expect("unbounded exact read must select the smart default");
        assert!(!replacement.suggestion.contains("--level none"));
    }

    for command in [
        "hzr rtk -- read src/main.rs --from 40 --to 80 --level none",
        "hzr rtk -- read src/main.rs -n --level none",
        "hzr rtk -- read src/main.rs --max-lines 80 --level none",
        "hzr rtk -- read src/main.rs --outline --level none",
        "HZR_EXACT_FIDELITY=1 hzr read src/main.rs --level none",
        "HZR_EXACT_FIDELITY=1 hzr rtk -- read src/main.rs --level none",
        "hzr search RewriteDecision --mode exact",
    ] {
        assert_eq!(
            efficient_route_replacement(command),
            None,
            "bounded or explicit fidelity route was changed: {command}"
        );
    }

    assert!(
        efficient_route_replacement(
            "HZR_EXACT_FIDELITY=10 hzr rtk -- read src/main.rs --level none"
        )
        .is_some(),
        "an invalid fidelity marker must not be interpreted as the exact escape hatch"
    );
}

#[test]
fn test_ambiguous_shell_syntax_is_never_reconstructed_for_automatic_execution() {
    for command in [
        "hzr rtk -- raw nl -ba \"src/file with spaces.rs\"",
        "hzr rtk -- raw rg -n \"two words\" src",
        "hzr rtk -- raw rg -n needle src | head -n 20",
        "hzr rtk -- raw rg -n 'a.*b' src",
        "hzr rtk -- raw cat src/*.rs",
    ] {
        assert_eq!(
            first_class_replacement(command),
            None,
            "ambiguous command was reconstructed: {command}"
        );
    }
}

#[test]
fn acceptance_gate_no_raw_commands_have_a_first_class_route() {
    for command in [
        "hzr rtk -- raw nl -ba src/main.rs",
        "hzr rtk -- raw sed -n 40,80p src/main.rs",
        "hzr rtk -- raw rg -n needle src",
    ] {
        assert_eq!(
            first_class_replacement(command),
            None,
            "fork-core must select the managed shell route: {command}"
        );
    }

    for (command, expected_payload) in [
        ("hzr rtk -- raw bun test", "bun test"),
        (
            "hzr rtk -- raw ssh host \"docker ps --format '{{.Names}}'\"",
            "ssh host \"docker ps --format '{{.Names}}'\"",
        ),
        ("hzr rtk -- raw git status --short", "git status --short"),
        (
            "hzr rtk -- raw cargo test --workspace",
            "cargo test --workspace",
        ),
        ("hzr rtk -- raw npm test", "npm test"),
        ("hzr rtk -- raw pnpm test", "pnpm test"),
    ] {
        assert_eq!(
            managed_raw_payload(command),
            Some(expected_payload),
            "managed wrapper did not preserve its payload: {command}"
        );
    }

    assert!(explicit_raw_fidelity(
        "HZR_RAW_FIDELITY=1 HZR_RAW_FIDELITY_REASON=binary hzr rtk -- raw cat artifact.json"
    ));
    assert!(!explicit_raw_fidelity(
        "HZR_RAW_FIDELITY=1 hzr rtk -- raw cat artifact.json"
    ));
    assert!(!explicit_raw_fidelity("hzr rtk -- raw cat artifact.json"));
}

#[test]
fn acceptance_gate_raw_fidelity_uses_a_closed_reason_set_without_echoing_values() {
    for (value, expected) in [
        ("binary", RawFidelityReason::Binary),
        ("checksum", RawFidelityReason::Checksum),
        ("machine_protocol", RawFidelityReason::MachineProtocol),
        ("complete_log", RawFidelityReason::CompleteLog),
        ("full_patch", RawFidelityReason::FullPatch),
        ("verbatim_source", RawFidelityReason::VerbatimSource),
    ] {
        let command = format!(
            "HZR_RAW_FIDELITY_REASON={value} HZR_RAW_FIDELITY=1 hzr rtk -- raw cat artifact.bin"
        );
        assert!(matches!(
            raw_fidelity_request(&command),
            RawFidelityRequest::Authorized { reason, payload: "cat artifact.bin" }
                if reason == expected
        ));
    }

    assert_eq!(
        raw_fidelity_request("HZR_RAW_FIDELITY=1 hzr rtk -- raw cat artifact.bin"),
        RawFidelityRequest::MissingReason
    );
    let secret = "do-not-echo-private-value";
    let invalid_command = format!(
        "HZR_RAW_FIDELITY=1 HZR_RAW_FIDELITY_REASON={secret} hzr rtk -- raw cat artifact.bin"
    );
    let invalid = raw_fidelity_request(&invalid_command);
    assert_eq!(invalid, RawFidelityRequest::InvalidReason);
    assert!(!format!("{invalid:?}").contains(secret));
}

#[test]
fn acceptance_gate_no_raw_wrapper_around_first_class_hzr_commands() {
    for (command, expected) in [
        ("hzr rtk -- raw hzr stats", "hzr stats"),
        (
            "hzr rtk -- raw hzr search \"two words\" --mode exact",
            "hzr search \"two words\" --mode exact",
        ),
        (
            "rtk proxy hzr rtk -- read \"docs/file with spaces.md\" --outline",
            "hzr rtk -- read \"docs/file with spaces.md\" --outline",
        ),
    ] {
        let replacement = first_class_replacement(command)
            .expect("a redundant raw wrapper around HZR must be removed");
        assert_eq!(replacement.suggestion, expected);
        assert_eq!(
            classify_operation(command)
                .replacement
                .expect("stats classification must expose the same recovery")
                .suggestion,
            expected
        );
    }

    let nested = "hzr rtk -- raw hzr rtk -- raw cargo test";
    assert_eq!(
        first_class_replacement(nested),
        None,
        "nested raw must remain an escape hatch: {nested}"
    );

    assert_eq!(
        first_class_replacement(
            "HZR_RAW_FIDELITY=1 HZR_RAW_FIDELITY_REASON=machine_protocol hzr rtk -- raw hzr stats"
        )
        .map(|replacement| replacement.suggestion),
        Some("hzr stats".into())
    );
}

#[test]
fn acceptance_gate_no_raw_for_top_level_hzr_file_aliases() {
    for command in [
        "hzr read \"docs/file with spaces.md\" --outline",
        "hzr write patch \"docs/file with spaces.md\" --old 'a b' --new 'c d'",
        "HZR_EXACT_FIDELITY=1 hzr read \"docs/file with spaces.md\" --level none",
    ] {
        let replacement = first_class_replacement(command)
            .expect("a typed top-level HZR file operation must not remain raw");
        assert_eq!(
            replacement.suggestion, command,
            "the top-level alias must preserve command bytes"
        );
    }

    for command in [
        "hzr reader file.md",
        "hzr writer file.md",
        "hzr unknown file.md",
    ] {
        assert_eq!(
            first_class_replacement(command),
            None,
            "an unknown HZR command must not be promoted to a typed file operation"
        );
    }
}

#[test]
fn acceptance_gate_raw_fidelity_rejects_unproven_equivalents() {
    for command in [
        "HZR_RAW_FIDELITY=1 HZR_RAW_FIDELITY_REASON=binary hzr rtk -- raw cat artifact.json",
        "HZR_RAW_FIDELITY=1 HZR_RAW_FIDELITY_REASON=verbatim_source hzr rtk -- raw rg -n needle src",
        "HZR_RAW_FIDELITY=1 HZR_RAW_FIDELITY_REASON=complete_log hzr rtk -- raw sh -c 'printf complete-output'",
    ] {
        assert!(explicit_raw_fidelity(command));
        assert_eq!(
            first_class_replacement(command),
            None,
            "a format-changing route must not override byte fidelity"
        );
        assert_eq!(efficient_route_replacement(command), None);
    }
}

#[test]
fn test_proxy_and_raw_prefixes_are_classified_as_bypassed() {
    for command in [
        "raw sed -n 1,5p src/lib.rs",
        "proxy rg -n needle",
        "rtk proxy sed -n 1,5p src/lib.rs",
        "rtk raw rg -n needle",
        "hzr proxy cargo test",
        "hzr rtk -- raw rg -n needle",
        "rtk fallback: grep -rn needle",
    ] {
        let classification = classify_operation(command);
        assert_eq!(
            classification.route,
            OperationRoute::Bypassed,
            "{command} must be classified as a bypass"
        );
        assert_eq!(
            classification.subsystem,
            OperationSubsystem::Bypass,
            "{command} must not be hidden inside another subsystem"
        );
    }
}

#[test]
fn test_optimized_commands_keep_their_own_subsystem() {
    for (command, expected) in [
        ("rtk read src/lib.rs", OperationSubsystem::Read),
        ("rtk read --outline src/lib.rs", OperationSubsystem::Read),
        ("rtk write patch src/lib.rs", OperationSubsystem::Write),
        ("rtk grep needle", OperationSubsystem::Search),
        ("rtk rgai (grepai)", OperationSubsystem::Search),
        ("rtk memory recall budget", OperationSubsystem::Memory),
        ("hzr codec compile", OperationSubsystem::Codec),
        ("rtk cargo test", OperationSubsystem::Execution),
    ] {
        let classification = classify_operation(command);
        assert_eq!(
            classification.subsystem, expected,
            "{command} was classified as {:?}",
            classification.subsystem
        );
        assert_eq!(classification.route, OperationRoute::Optimized);
    }
}

#[test]
fn test_bypassed_shell_tools_do_not_carry_a_second_rewrite_plan() {
    for command in [
        "rtk proxy sed -n 1030,1105p crates/hzr-core/src/ledger.rs",
        "rtk proxy rg -n RewriteDecision crates/hzr-exec",
        "rtk proxy cat README.md",
        "rtk proxy nl -ba crates/hzr-cli/src/main.rs",
    ] {
        assert_eq!(
            classify_operation(command).replacement,
            None,
            "ledger classification must not reconstruct shell policy: {command}"
        );
    }
}

#[test]
fn test_bypassed_execution_without_a_replacement_is_still_a_bypass() {
    let classification = classify_operation("rtk proxy cargo clippy --workspace");

    assert_eq!(classification.route, OperationRoute::Bypassed);
    assert_eq!(classification.replacement, None);
}

#[test]
fn test_operation_identity_survives_the_bypass_prefix() {
    assert_eq!(
        classify_operation("rtk proxy sed -n 1,2p f").operation,
        "sed"
    );
    assert_eq!(classify_operation("rtk read f").operation, "read");
    assert_eq!(classify_operation("").operation, "operation");
}

/// The SQL predicate and the Rust classifier must be generated from one list of markers,
/// otherwise the dashboard and the terminal disagree about the same row again.
#[test]
fn test_sql_predicate_matches_the_rust_classifier() {
    let predicate = raw_route_sql_predicate("rtk_cmd");

    assert!(predicate.contains("rtk_cmd LIKE 'rtk proxy %'"));
    assert!(predicate.contains("rtk_cmd LIKE 'hzr rtk -- raw %'"));
    assert!(predicate.contains("rtk_cmd LIKE 'raw %'"));
    assert!(predicate.contains("rtk_cmd LIKE 'rtk fallback%'"));
}
