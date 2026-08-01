use std::process::Command;

use serde_json::Value;

#[test]
fn test_tdd_reports_hzr_red_green_refactor_contract() -> anyhow::Result<()> {
    let output = Command::new(env!("CARGO_BIN_EXE_hzr"))
        .args(["tdd", "--json"])
        .output()?;

    assert!(
        output.status.success(),
        "hzr tdd failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let contract: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(contract["name"], "hzr-tdd");
    assert_eq!(contract["workflow"], "red_green_refactor");
    assert_eq!(contract["strict"], true);
    assert_eq!(contract["upstream_reference"], "rtk-tdd");
    assert_eq!(
        contract["upstream_repository"],
        "https://github.com/rtk-ai/rtk"
    );
    assert_eq!(
        contract["upstream_revision"],
        "e0ffd40ef7c450489aca4a50c0ab1358e4375691"
    );

    let phases = contract["phases"].as_array().expect("TDD phases");
    assert_eq!(phases.len(), 3);
    assert_eq!(phases[0]["name"], "red");
    assert_eq!(phases[1]["name"], "green");
    assert_eq!(phases[2]["name"], "refactor");

    let gate = contract["quality_gate"]
        .as_array()
        .expect("quality gate commands");
    assert_eq!(
        gate,
        &[
            "cargo fmt --all --check",
            "cargo clippy --workspace --all-targets --all-features -- -D warnings",
            "cargo test --workspace --all-targets --all-features",
        ]
    );
    Ok(())
}

#[test]
fn test_tdd_text_distinguishes_tdd_from_post_hoc_tests() -> anyhow::Result<()> {
    let output = Command::new(env!("CARGO_BIN_EXE_hzr"))
        .arg("tdd")
        .output()?;

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout)?;
    assert!(stdout.contains("RED"));
    assert!(stdout.contains("GREEN"));
    assert!(stdout.contains("REFACTOR"));
    assert!(
        stdout.contains("A passing test without an observed RED is regression coverage, not TDD.")
    );
    Ok(())
}
