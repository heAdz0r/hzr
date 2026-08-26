//! Machine-readable, justified exemptions for fleet instruction policy.
//!
//! Some repositories legitimately cannot route a directive through HZR: a benchmark whose
//! measured subject *is* the engine has to name that engine in its own instructions, and
//! rewriting those lines would destroy the measurement. Before this module that case was a
//! hardcoded path heuristic, which is indistinguishable from a silently hidden bypass.
//!
//! A project now declares the waiver in `.hzr/policy.toml`, naming the rule it covers and
//! why. `hzr doctor` reports the waiver explicitly instead of either failing forever or
//! passing without evidence. Only instruction-directive rules are waivable: an exemption can
//! never suppress a bypass that HZR could have replaced at execution time.

use anyhow::{Context, Result, bail};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use toml_edit::{DocumentMut, Item};

/// Where a project declares its waivers, relative to the workspace root.
pub const POLICY_RELATIVE_PATH: &str = ".hzr/policy.toml";

const SCHEMA_VERSION: i64 = 1;

/// A waiver has to say something a reviewer can audit. This is a floor, not a quality bar,
/// but it stops `reason = "n/a"` from passing as a justification.
const MIN_JUSTIFICATION_BYTES: usize = 40;

/// Instruction-directive rules a project may waive.
///
/// Deliberately excludes every execution-time route: `replacement-capable bypass` must stay
/// at zero, and no file on disk is allowed to buy an exception to that.
const WAIVABLE_RULES: &[&str] = &["direct-rtk", "direct-grepai", "direct-icm"];

/// The waivers a workspace declares, already validated.
#[derive(Debug, Default, Clone)]
pub struct FleetExemptions {
    path: Option<PathBuf>,
    rules: BTreeSet<String>,
    reasons: Vec<String>,
}

impl FleetExemptions {
    /// Whether a formatted `conflicting_mandates` entry is covered by a declared waiver.
    ///
    /// The audit renders conflicts as `<rule> at line N: ...`, so the rule is the prefix.
    #[must_use]
    pub fn covers(&self, conflict: &str) -> bool {
        let rule = conflict.split(" at line ").next().unwrap_or_default();
        self.rules.contains(rule)
    }

    /// A one-line, auditable description for the doctor detail.
    #[must_use]
    pub fn summary(&self) -> String {
        let path = self
            .path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        format!(
            "{} declares {} ({})",
            path,
            self.rules
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", "),
            self.reasons.join("; ")
        )
    }
}

/// Read and validate `<root>/.hzr/policy.toml`.
///
/// A missing file is not an error: most projects have nothing to waive. An unreadable or
/// invalid file *is* an error, because a waiver that cannot be audited must not be honoured.
pub fn load(root: &Path) -> Result<FleetExemptions> {
    let path = root.join(POLICY_RELATIVE_PATH);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(FleetExemptions::default());
        }
        Err(error) => {
            return Err(error).with_context(|| format!("failed to read {}", path.display()));
        }
    };
    let document = text
        .parse::<DocumentMut>()
        .with_context(|| format!("{} is not valid TOML", path.display()))?;
    parse(&document, &path)
}

fn parse(document: &DocumentMut, path: &Path) -> Result<FleetExemptions> {
    let schema_version = document
        .get("schema_version")
        .and_then(Item::as_integer)
        .with_context(|| format!("{} needs an integer `schema_version`", path.display()))?;
    if schema_version != SCHEMA_VERSION {
        bail!(
            "{} declares schema_version {schema_version}; this HZR understands {SCHEMA_VERSION}",
            path.display()
        );
    }
    let Some(entries) = document.get("exemption").and_then(Item::as_array_of_tables) else {
        return Ok(FleetExemptions {
            path: Some(path.to_path_buf()),
            ..FleetExemptions::default()
        });
    };
    let mut rules = BTreeSet::new();
    let mut reasons = Vec::new();
    for entry in entries {
        let field = |name: &str| entry.get(name).and_then(Item::as_str).unwrap_or_default();
        let rule = field("rule");
        let reason = field("reason");
        let justification = field("justification");
        if !WAIVABLE_RULES.contains(&rule) {
            bail!(
                "{} waives `{rule}`, which is not an instruction-directive rule; waivable rules are {}",
                path.display(),
                WAIVABLE_RULES.join(", ")
            );
        }
        if reason.trim().is_empty() {
            bail!("{} waives `{rule}` without a `reason`", path.display());
        }
        if justification.trim().len() < MIN_JUSTIFICATION_BYTES {
            bail!(
                "{} waives `{rule}` with a justification shorter than {MIN_JUSTIFICATION_BYTES} bytes; state why the directive cannot be routed through HZR",
                path.display()
            );
        }
        rules.insert(rule.to_owned());
        reasons.push(format!("{rule}={reason}"));
    }
    Ok(FleetExemptions {
        path: Some(path.to_path_buf()),
        rules,
        reasons,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_policy(body: &str) -> tempfile::TempDir {
        let fixture = tempfile::tempdir().expect("fixture");
        let directory = fixture.path().join(".hzr");
        std::fs::create_dir_all(&directory).expect("policy directory");
        std::fs::write(directory.join("policy.toml"), body).expect("policy file");
        fixture
    }

    const VALID: &str = r#"
schema_version = 1

[[exemption]]
rule = "direct-rtk"
reason = "benchmark-subject"
justification = "This repository measures upstream RTK against HZR fork-core, so the rtk invocations are the measured baseline."
"#;

    #[test]
    fn a_workspace_without_a_policy_file_declares_nothing() {
        let fixture = tempfile::tempdir().expect("fixture");
        let exemptions = load(fixture.path()).expect("absent policy is not an error");
        assert!(!exemptions.covers("direct-rtk at line 1: rtk git status"));
    }

    #[test]
    fn a_justified_waiver_covers_only_its_own_rule() {
        let fixture = write_policy(VALID);
        let exemptions = load(fixture.path()).expect("valid policy");
        assert!(!exemptions.rules.is_empty());
        assert!(exemptions.covers("direct-rtk at line 34: rtk cargo build"));
        assert!(!exemptions.covers("direct-icm at line 9: icm_memory_store"));
        assert!(exemptions.summary().contains("benchmark-subject"));
    }

    #[test]
    fn an_execution_route_can_never_be_waived_by_a_file() {
        let fixture = write_policy(
            r#"
schema_version = 1

[[exemption]]
rule = "replacement-capable-bypass"
reason = "convenience"
justification = "We would rather not route these commands through the control plane at all."
"#,
        );
        let error = load(fixture.path()).expect_err("execution routes are not waivable");
        assert!(
            error
                .to_string()
                .contains("not an instruction-directive rule")
        );
    }

    #[test]
    fn a_waiver_without_an_auditable_justification_is_rejected() {
        let fixture = write_policy(
            r#"
schema_version = 1

[[exemption]]
rule = "direct-rtk"
reason = "n/a"
justification = "because"
"#,
        );
        let error = load(fixture.path()).expect_err("thin justification");
        assert!(error.to_string().contains("shorter than"));
    }

    #[test]
    fn an_unknown_schema_version_is_not_honoured() {
        let fixture = write_policy("schema_version = 99\n");
        let error = load(fixture.path()).expect_err("unknown schema");
        assert!(error.to_string().contains("understands 1"));
    }
}
