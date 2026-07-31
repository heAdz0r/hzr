use std::collections::HashSet;
use std::sync::LazyLock;

use hzr_protocol::{CodecProfile, FidelityClass, ProtectedSpan, RiskClass};
use regex::Regex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

static PROTECTED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?ms)\x60\x60\x60.*?\x60\x60\x60|~~~.*?~~~|\x60[^\x60\n]+\x60|https?://[^\s<>"']+|(?:^|[\s("'=])(?:\./|\.\./|/)[A-Za-z0-9_.@%+~/-]+|--?[A-Za-z][A-Za-z0-9_-]*|\b(?:[A-Fa-f0-9]{7,64}|v?\d+(?:\.\d+){1,3})\b"#,
    )
    .expect("protected-span regex is a static invariant")
});

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Density {
    Normal,
    Concise,
    DenseStructured,
}

impl Density {
    pub fn contract(self) -> &'static str {
        match self {
            Self::Normal => {
                "Answer at the detail level required for correctness. Preserve code, commands, paths, identifiers, errors, numbers, caveats, and causality."
            }
            Self::Concise => {
                "Be concise. Lead with the result. Omit greetings, request restatement, tool recap, and unchosen alternatives. Preserve evidence, caveats, and exact technical artifacts."
            }
            Self::DenseStructured => {
                "Return dense structured fields: status, evidence, changed_files, verification, risks, next_action. Preserve exact code, commands, paths, identifiers, errors, and numbers."
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EconomicInput {
    pub expected_input_saved: u64,
    pub expected_output_saved: u64,
    pub added_instruction_tokens: u64,
    pub input_price_per_million: f64,
    pub output_price_per_million: f64,
    pub retry_probability_delta: f64,
    pub expected_retry_cost: f64,
    pub minimum_margin: f64,
}

impl EconomicInput {
    pub fn expected_value(self) -> f64 {
        let input = self.expected_input_saved as f64 * self.input_price_per_million / 1_000_000.0;
        let output =
            self.expected_output_saved as f64 * self.output_price_per_million / 1_000_000.0;
        let overhead =
            self.added_instruction_tokens as f64 * self.input_price_per_million / 1_000_000.0;
        input + output - overhead - self.retry_probability_delta * self.expected_retry_cost
    }

    pub fn is_profitable(self) -> bool {
        self.expected_value() > self.minimum_margin
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Transform {
    pub content: String,
    pub changed: bool,
    pub profile: CodecProfile,
    pub protected_spans: Vec<ProtectedSpan>,
}

#[derive(Debug, Error)]
pub enum CodecError {
    #[error("protected content changed or disappeared")]
    ProtectedContentChanged,
}

pub fn choose_density(
    profile: CodecProfile,
    risk: RiskClass,
    expected_output_tokens: u64,
    profitable: bool,
) -> Density {
    if matches!(risk, RiskClass::High | RiskClass::Irreversible) {
        return Density::Normal;
    }

    match profile {
        CodecProfile::Off | CodecProfile::Shadow => Density::Normal,
        CodecProfile::Safe => Density::Concise,
        CodecProfile::Adaptive if profitable && expected_output_tokens >= 600 => Density::Concise,
        CodecProfile::Compact if expected_output_tokens >= 300 => Density::DenseStructured,
        CodecProfile::Adaptive | CodecProfile::Compact => Density::Normal,
    }
}

pub fn transform(
    input: &str,
    fidelity: FidelityClass,
    profile: CodecProfile,
) -> Result<Transform, CodecError> {
    let spans = protected_spans(input);
    if fidelity == FidelityClass::Exact || profile == CodecProfile::Off {
        return Ok(Transform {
            content: input.to_owned(),
            changed: false,
            profile,
            protected_spans: spans,
        });
    }

    let content = deduplicate_paragraphs(input);
    if validate_protected(input, &content).is_err() {
        return Ok(Transform {
            content: input.to_owned(),
            changed: false,
            profile,
            protected_spans: spans,
        });
    }

    Ok(Transform {
        changed: content != input,
        content,
        profile,
        protected_spans: spans,
    })
}

pub fn compact_catalog_description(input: &str) -> Result<Transform, CodecError> {
    transform(input, FidelityClass::LosslessStructural, CodecProfile::Safe)
}

pub fn protected_spans(input: &str) -> Vec<ProtectedSpan> {
    PROTECTED
        .find_iter(input)
        .map(|matched| ProtectedSpan {
            start: matched.start(),
            end: matched.end(),
            kind: classify(matched.as_str()).into(),
        })
        .collect()
}

fn classify(value: &str) -> &'static str {
    if value.starts_with("\x60\x60\x60")
        || value.starts_with("~~~")
        || value.as_bytes().first() == Some(&0x60)
    {
        "code"
    } else if value.starts_with("http") {
        "url"
    } else if value.trim_start().starts_with('-') {
        "flag"
    } else if value.contains('/') {
        "path"
    } else {
        "literal"
    }
}

fn deduplicate_paragraphs(input: &str) -> String {
    let mut seen = HashSet::new();
    input
        .split("\n\n")
        .filter(|paragraph| seen.insert(paragraph.trim()))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn validate_protected(original: &str, transformed: &str) -> Result<(), CodecError> {
    if protected_values(original)
        .iter()
        .all(|value| transformed.matches(value).count() == original.matches(value).count())
    {
        Ok(())
    } else {
        Err(CodecError::ProtectedContentChanged)
    }
}

fn protected_values(input: &str) -> HashSet<&str> {
    PROTECTED
        .find_iter(input)
        .map(|matched| matched.as_str())
        .collect()
}

#[cfg(test)]
mod tests {
    use hzr_protocol::{CodecProfile, FidelityClass, RiskClass};

    use super::{Density, EconomicInput, choose_density, compact_catalog_description, transform};

    #[test]
    fn test_exact_content_is_never_changed() {
        let input = "run cargo test against ./crates/core and keep error E0123";
        let result = transform(input, FidelityClass::Exact, CodecProfile::Compact)
            .expect("exact transform must succeed");

        assert_eq!(result.content, input);
        assert!(!result.changed);
    }

    #[test]
    fn test_structural_transform_only_removes_exact_duplicate_paragraphs() {
        let input = "Keep --mode STARTER/BUSINESS.\n\nRepeat me.\n\nRepeat me.";
        let result = compact_catalog_description(input).expect("description must compact");

        assert_eq!(
            result.content,
            "Keep --mode STARTER/BUSINESS.\n\nRepeat me."
        );
        assert!(result.content.contains("STARTER/BUSINESS"));
        assert!(result.content.contains("--mode"));
    }

    #[test]
    fn test_protected_duplicate_falls_back_to_raw() {
        let input = "Run `cargo test`.\n\nRun `cargo test`.";
        let result = compact_catalog_description(input).expect("codec returns raw fallback");

        assert_eq!(result.content, input);
        assert!(!result.changed);
    }

    #[test]
    fn test_adaptive_density_requires_positive_value_and_long_output() {
        assert_eq!(
            choose_density(CodecProfile::Adaptive, RiskClass::Low, 1_000, true),
            Density::Concise
        );
        assert_eq!(
            choose_density(CodecProfile::Adaptive, RiskClass::Low, 200, true),
            Density::Normal
        );
        assert_eq!(
            choose_density(CodecProfile::Compact, RiskClass::Irreversible, 2_000, true),
            Density::Normal
        );
    }

    #[test]
    fn test_economic_gate_charges_instruction_and_retry_cost() {
        let input = EconomicInput {
            expected_input_saved: 0,
            expected_output_saved: 500,
            added_instruction_tokens: 1_500,
            input_price_per_million: 10.0,
            output_price_per_million: 20.0,
            retry_probability_delta: 0.01,
            expected_retry_cost: 1.0,
            minimum_margin: 0.0,
        };

        assert!(!input.is_profitable());
    }
}
