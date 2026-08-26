use std::collections::HashSet;
use std::sync::LazyLock;

use hzr_protocol::{CodecProfile, FidelityClass, ProtectedSpan, RiskClass};
use regex::Regex;
use serde::{Deserialize, Serialize};
use thiserror::Error;

static PROTECTED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?ms)\x60\x60\x60.*?\x60\x60\x60|~~~.*?~~~|\x60[^\x60\n]+\x60|https?://[^\s<>"']+|\{[^\r\n{}]*\}|\[[^\r\n\[\]]*\]|(?:\./|\.\./|/)[A-Za-z0-9_.@%+~/-]+|\b(?:[A-Za-z0-9_.@%+~-]+/)+[A-Za-z0-9_.@%+~-]+\b|--?[A-Za-z][A-Za-z0-9_-]*|\b[A-Za-z_][A-Za-z0-9_]*_[A-Za-z0-9_]+\b|\b(?:[A-Fa-f0-9]{7,64}|v?\d+(?:\.\d+){1,3})\b"#,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub counterfactual: Option<CounterfactualSize>,
    pub coverage_state: ResponseCodecCoverageState,
    pub global_response_replacement_confirmed: bool,
    pub estimated_token_credit_eligible: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResponseCodecCoverageState {
    Applied,
    ShadowMeasured,
    Instructed,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CounterfactualSize {
    pub input_bytes: usize,
    pub output_bytes: usize,
    pub saved_bytes: usize,
    pub would_change: bool,
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
    if profile == CodecProfile::Shadow {
        let candidate = candidate_transform(input, fidelity);
        return Ok(Transform {
            content: input.to_owned(),
            changed: false,
            profile,
            protected_spans: spans,
            counterfactual: Some(CounterfactualSize {
                input_bytes: input.len(),
                output_bytes: candidate.len(),
                saved_bytes: input.len().saturating_sub(candidate.len()),
                would_change: candidate != input,
            }),
            coverage_state: ResponseCodecCoverageState::ShadowMeasured,
            global_response_replacement_confirmed: false,
            estimated_token_credit_eligible: false,
        });
    }
    if fidelity == FidelityClass::Exact || profile == CodecProfile::Off {
        return Ok(Transform {
            content: input.to_owned(),
            changed: false,
            profile,
            protected_spans: spans,
            counterfactual: None,
            coverage_state: ResponseCodecCoverageState::Applied,
            global_response_replacement_confirmed: false,
            estimated_token_credit_eligible: false,
        });
    }

    let content = candidate_transform(input, fidelity);
    let changed = content != input;

    Ok(Transform {
        changed,
        content,
        profile,
        protected_spans: spans,
        counterfactual: None,
        coverage_state: ResponseCodecCoverageState::Applied,
        global_response_replacement_confirmed: false,
        estimated_token_credit_eligible: changed,
    })
}

pub fn transform_for_risk(
    input: &str,
    fidelity: FidelityClass,
    profile: CodecProfile,
    risk: RiskClass,
) -> Result<Transform, CodecError> {
    let effective_fidelity = if matches!(risk, RiskClass::High | RiskClass::Irreversible) {
        FidelityClass::Exact
    } else {
        fidelity
    };
    transform(input, effective_fidelity, profile)
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
    } else if value.starts_with('{') || value.starts_with('[') {
        "structured"
    } else if value.contains('_')
        && value.chars().all(|character| {
            character.is_ascii_uppercase() || character.is_ascii_digit() || character == '_'
        })
    {
        "enum"
    } else if value.contains('_') {
        "identifier"
    } else {
        "literal"
    }
}

fn candidate_transform(input: &str, fidelity: FidelityClass) -> String {
    if fidelity == FidelityClass::Exact {
        return input.to_owned();
    }
    let candidate = deduplicate_paragraphs(input);
    if validate_protected(input, &candidate).is_ok() {
        candidate
    } else {
        input.to_owned()
    }
}

fn deduplicate_paragraphs(input: &str) -> String {
    let trailing_newlines = input
        .as_bytes()
        .iter()
        .rev()
        .take_while(|byte| **byte == b'\n')
        .count();
    let mut seen = HashSet::new();
    let mut output = input
        .split("\n\n")
        .filter(|paragraph| seen.insert(paragraph.trim()))
        .collect::<Vec<_>>()
        .join("\n\n");
    let output_trailing_newlines = output
        .as_bytes()
        .iter()
        .rev()
        .take_while(|byte| **byte == b'\n')
        .count();
    output.extend(std::iter::repeat_n(
        '\n',
        trailing_newlines.saturating_sub(output_trailing_newlines),
    ));
    output
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

    use super::{
        Density, EconomicInput, ResponseCodecCoverageState, choose_density,
        compact_catalog_description, protected_spans, transform, transform_for_risk,
    };

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
        assert!(result.counterfactual.is_none());
        assert_eq!(result.coverage_state, ResponseCodecCoverageState::Applied);
        assert!(!result.global_response_replacement_confirmed);
        assert!(result.estimated_token_credit_eligible);
    }

    #[test]
    fn test_shadow_reports_counterfactual_without_changing_content() {
        let input = "The budget is exhausted.\n\nSecond paragraph.\n\nThe budget is exhausted.";
        let result = transform(
            input,
            FidelityClass::LosslessStructural,
            CodecProfile::Shadow,
        )
        .expect("shadow transform");

        assert_eq!(result.content, input);
        assert!(!result.changed);
        assert_eq!(
            result.coverage_state,
            ResponseCodecCoverageState::ShadowMeasured
        );
        assert!(!result.global_response_replacement_confirmed);
        assert!(!result.estimated_token_credit_eligible);
        let counterfactual = result.counterfactual.expect("shadow measurement");
        assert!(counterfactual.would_change);
        assert_eq!(counterfactual.input_bytes, input.len());
        assert!(counterfactual.output_bytes < counterfactual.input_bytes);
        assert_eq!(
            counterfactual.saved_bytes,
            input.len() - counterfactual.output_bytes
        );
    }

    #[test]
    fn test_protected_spans_cover_relative_paths_identifiers_enums_and_json() {
        let input =
            r#"Edit src/main.rs and set MAX_RETRIES for handle_budget_overflow with {"k":1}."#;
        let spans = protected_spans(input);
        let protected = spans
            .iter()
            .map(|span| (&input[span.start..span.end], span.kind.as_str()))
            .collect::<Vec<_>>();

        assert!(protected.contains(&("src/main.rs", "path")));
        assert!(protected.contains(&("MAX_RETRIES", "enum")));
        assert!(protected.contains(&("handle_budget_overflow", "identifier")));
        assert!(protected.contains(&(r#"{"k":1}"#, "structured")));
    }

    #[test]
    fn test_structural_transform_preserves_trailing_newline_after_deduplication() {
        let input = "First paragraph.\n\nSecond paragraph.\n\nFirst paragraph.\n";
        let result = compact_catalog_description(input).expect("description must compact");

        assert_eq!(result.content, "First paragraph.\n\nSecond paragraph.\n");
        assert!(result.changed);
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
    fn test_high_risk_content_is_never_changed() {
        let input = "Step A must happen twice.\n\nStep A must happen twice.";
        let result = transform_for_risk(
            input,
            FidelityClass::LosslessStructural,
            CodecProfile::Compact,
            RiskClass::High,
        )
        .expect("high-risk transform must succeed");

        assert_eq!(result.content, input);
        assert!(!result.changed);
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
