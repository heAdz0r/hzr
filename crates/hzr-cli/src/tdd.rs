use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct TddPhase {
    pub name: &'static str,
    pub requirement: &'static str,
}

#[derive(Debug, Serialize)]
pub struct TddContract {
    pub name: &'static str,
    pub workflow: &'static str,
    pub strict: bool,
    pub upstream_reference: &'static str,
    pub upstream_repository: &'static str,
    pub upstream_revision: &'static str,
    pub phases: [TddPhase; 3],
    pub quality_gate: [&'static str; 3],
}

pub fn contract() -> TddContract {
    TddContract {
        name: "hzr-tdd",
        workflow: "red_green_refactor",
        strict: true,
        upstream_reference: "rtk-tdd",
        upstream_repository: "https://github.com/rtk-ai/rtk",
        upstream_revision: "e0ffd40ef7c450489aca4a50c0ab1358e4375691",
        phases: [
            TddPhase {
                name: "red",
                requirement: "Run a focused new test and observe the intended failure before production changes.",
            },
            TddPhase {
                name: "green",
                requirement: "Make the smallest production change and rerun the identical test successfully.",
            },
            TddPhase {
                name: "refactor",
                requirement: "Improve the implementation without broadening scope and keep all tests green.",
            },
        ],
        quality_gate: [
            "cargo fmt --all --check",
            "cargo clippy --workspace --all-targets --all-features -- -D warnings",
            "cargo test --workspace --all-targets --all-features",
        ],
    }
}

pub fn render_text(contract: &TddContract) -> String {
    let mut output = String::from("HZR TDD — strict Red-Green-Refactor\n\n");
    for phase in &contract.phases {
        output.push_str(&phase.name.to_uppercase());
        output.push_str(": ");
        output.push_str(phase.requirement);
        output.push('\n');
    }
    output.push_str("\nA passing test without an observed RED is regression coverage, not TDD.\n\nQuality gate:\n");
    for command in contract.quality_gate {
        output.push_str("  ");
        output.push_str(command);
        output.push('\n');
    }
    output
}

#[cfg(test)]
mod tests {
    use super::{contract, render_text};

    const SKILL: &str = include_str!("../../../.claude/skills/hzr-tdd/SKILL.md");

    #[test]
    fn test_skill_and_cli_contract_share_strict_red_evidence() {
        let contract = contract();
        let rendered = render_text(&contract);

        assert!(SKILL.contains("hzr-managed-skill: hzr-tdd-v1"));
        assert!(SKILL.contains("A test that already passes is regression coverage, not TDD."));
        assert!(rendered.contains("without an observed RED is regression coverage, not TDD"));
        assert!(contract.strict);
    }
}
