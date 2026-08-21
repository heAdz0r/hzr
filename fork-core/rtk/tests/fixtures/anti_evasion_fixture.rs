use serde::Deserialize;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ProbeLayer {
    #[default]
    Fork,
    Root,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ProbeSurface {
    #[default]
    Shell,
    Native,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ProbeDecision {
    Rewrite,
    Ask,
    Proxy,
    Raw,
    Allow,
    Deny,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ProbeNativeMode {
    Observe,
    Steer,
    Strict,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ProbeClass {
    E8NativeTool,
    E10CapabilityGap,
}

#[derive(Clone, Debug, Deserialize)]
pub struct AntiEvasionProbe {
    pub id: String,
    #[serde(default)]
    pub layer: ProbeLayer,
    #[serde(default)]
    pub surface: ProbeSurface,
    #[serde(default)]
    pub command: Option<String>,
    pub decision: ProbeDecision,
    #[serde(default)]
    pub route: Option<String>,
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub tool_input: Option<serde_json::Value>,
    #[serde(default)]
    pub mode: Option<ProbeNativeMode>,
    #[serde(default)]
    pub class: Option<ProbeClass>,
    #[serde(default)]
    pub avoidable: Option<bool>,
    /// Substrings the agent-facing reason must contain.
    ///
    /// A decision the agent cannot act on is not enforcement, so every Ask and Deny asserts that
    /// its reason names the evasion class and prescribes a route. Without this the matrix could
    /// only prove that a construct was refused, never that the refusal was useful.
    #[serde(default)]
    pub expect_reason_contains: Vec<String>,
}

pub fn load_anti_evasion_probes() -> Vec<AntiEvasionProbe> {
    let probes: Vec<AntiEvasionProbe> =
        serde_json::from_str(include_str!("anti_evasion_policy.json"))
            .expect("anti-evasion fixture must be valid JSON");
    for probe in &probes {
        match probe.surface {
            ProbeSurface::Shell => {
                assert!(
                    probe.command.is_some(),
                    "shell probe {} has no command",
                    probe.id
                );
                assert!(
                    probe.tool.is_none(),
                    "shell probe {} has a native tool",
                    probe.id
                );
                assert!(
                    probe.tool_input.is_none() && probe.mode.is_none(),
                    "shell probe {} has native input",
                    probe.id
                );
                assert!(
                    probe.class.is_none() && probe.avoidable.is_none(),
                    "shell probe {} has native attribution",
                    probe.id
                );
            }
            ProbeSurface::Native => {
                assert_eq!(
                    probe.layer,
                    ProbeLayer::Root,
                    "native probe {} is not root",
                    probe.id
                );
                assert!(
                    probe.command.is_none(),
                    "native probe {} has shell input",
                    probe.id
                );
                assert!(
                    probe.tool.is_some() && probe.tool_input.is_some() && probe.mode.is_some(),
                    "native probe {} is incomplete",
                    probe.id
                );
                assert!(
                    probe.class.is_some() && probe.avoidable.is_some(),
                    "native probe {} has no attribution",
                    probe.id
                );
            }
        }
        if matches!(probe.decision, ProbeDecision::Ask | ProbeDecision::Deny) {
            assert!(
                !probe.expect_reason_contains.is_empty(),
                "{} decision {:?} must assert what its reason tells the agent",
                probe.id,
                probe.decision
            );
        }
    }
    probes
}
