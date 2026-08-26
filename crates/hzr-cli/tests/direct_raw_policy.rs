use std::process::Command;

use hzr_core::{Config, Ledger, StatsQuery};
use hzr_protocol::{EvasionClass, PolicyDecision};
use tempfile::tempdir;

#[test]
fn published_cli_does_not_expose_an_unaccounted_direct_raw_route() {
    let directory = tempdir().expect("temporary HZR home");
    let config_path = directory.path().join("config.toml");
    let config = Config {
        data_dir: directory.path().join("data"),
        ..Config::default()
    };
    config.write(&config_path).expect("write config");

    for attempt in 1..=6 {
        let output = Command::new(env!("CARGO_BIN_EXE_hzr"))
            .args(["--config"])
            .arg(&config_path)
            .args(["rtk", "--", "raw", "definitely-not-executed"])
            .env("HZR_RAW_FIDELITY", "1")
            .env("HZR_RAW_FIDELITY_REASON", "checksum")
            .output()
            .expect("run HZR");

        assert!(
            !output.status.success(),
            "attempt {attempt} must be refused"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("direct managed raw execution is disabled"));
        assert!(stderr.contains("hzr exec run"));
    }

    let ledger = Ledger::open(&config.data_dir.join("ledger/hzr.sqlite")).expect("ledger");
    let evasion = ledger
        .evasion_summary(StatsQuery::default())
        .expect("evasion summary");
    assert_eq!(evasion.fidelity_operations, 0);
    let denial = evasion
        .policy_by_class
        .iter()
        .find(|summary| {
            summary.class == EvasionClass::E7FidelityHatch
                && summary.decision == PolicyDecision::Deny
        })
        .expect("E7 denial");
    assert_eq!(denial.attempts, 6);
}

#[test]
fn published_contracts_do_not_recommend_the_disabled_direct_raw_route() {
    const CANONICAL: &str =
        "HZR_RAW_FIDELITY=1 HZR_RAW_FIDELITY_REASON=<reason> hzr exec run '<command>'";
    const DISABLED: &str = "HZR_RAW_FIDELITY=1 HZR_RAW_FIDELITY_REASON=<reason> hzr rtk -- raw";
    for path in ["../../HZR.md", "../../AGENTS.md", "../../CLAUDE.md"] {
        let contents = std::fs::read_to_string(path).expect("published contract");
        assert!(
            contents.contains(CANONICAL),
            "{path} does not publish the canonical managed fidelity route"
        );
        assert!(
            !contents.contains(DISABLED),
            "{path} still authorizes the disabled direct raw route"
        );
    }
}
