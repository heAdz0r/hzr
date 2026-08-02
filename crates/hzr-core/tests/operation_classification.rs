//! The single source of truth for "did this operation go through the optimizer?".
//!
//! Before this module existed the answer was computed in three places with three
//! different rules: `operation_identity` in the ledger, a `LIKE` predicate in SQL, and
//! `classify_command` in the CLI. They disagreed, and the disagreement is precisely why
//! `hzr stats` reported 87% savings while half of the delivered tokens had bypassed the
//! optimizer entirely.

use hzr_core::{
    OperationRoute, OperationSubsystem, RawReplacement, classify_operation,
    first_class_replacement, raw_route_sql_predicate,
};

/// The hook needs the same answer for a command the agent typed directly, before any
/// bypass prefix exists. Deriving it from a second rule is how the two drifted apart the
/// first time, so both callers ask this one function.
#[test]
fn test_a_plain_shell_command_resolves_to_the_same_replacement() {
    assert_eq!(
        first_class_replacement("sed -n 1030,1105p crates/hzr-core/src/ledger.rs")
            .map(|replacement| replacement.suggestion),
        Some("hzr rtk -- read crates/hzr-core/src/ledger.rs --from 1030 --to 1105".to_owned())
    );
    assert_eq!(
        first_class_replacement("hzr rtk -- raw sed -n 1030,1105p crates/hzr-core/src/ledger.rs")
            .map(|replacement| replacement.suggestion),
        Some("hzr rtk -- read crates/hzr-core/src/ledger.rs --from 1030 --to 1105".to_owned()),
        "an explicit escape hatch resolves to the same suggestion as the bare command"
    );
    assert_eq!(
        first_class_replacement("nl -ba src/main.rs").map(|replacement| replacement.suggestion),
        Some("hzr rtk -- read src/main.rs -n".to_owned())
    );
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
fn test_bypassed_reads_and_searches_carry_their_first_class_replacement() {
    let sed = classify_operation("rtk proxy sed -n 1030,1105p crates/hzr-core/src/ledger.rs");
    assert_eq!(
        sed.replacement,
        Some(RawReplacement {
            tool: "sed",
            suggestion: "hzr rtk -- read crates/hzr-core/src/ledger.rs --from 1030 --to 1105"
                .into(),
            rationale: "hzr read streams the requested span with filtering instead of the whole slice",
        })
    );

    let ripgrep = classify_operation("rtk proxy rg -n RewriteDecision crates/hzr-exec");
    let ripgrep = ripgrep
        .replacement
        .expect("rg has a first-class replacement");
    assert_eq!(ripgrep.tool, "rg");
    assert_eq!(
        ripgrep.suggestion,
        "hzr search 'RewriteDecision' --mode exact --path crates/hzr-exec"
    );

    let cat = classify_operation("rtk proxy cat README.md");
    assert_eq!(
        cat.replacement.map(|replacement| replacement.suggestion),
        Some("hzr rtk -- read README.md".to_owned())
    );

    let nl = classify_operation("rtk proxy nl -ba crates/hzr-cli/src/main.rs");
    assert_eq!(
        nl.replacement.map(|replacement| replacement.suggestion),
        Some("hzr rtk -- read crates/hzr-cli/src/main.rs -n".to_owned())
    );
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
