use anyhow::{Context, Result};
use hzr_protocol::{
    CodecProfile, FidelityClass, MemoryImportance, MemoryScopeSelector, MemoryWriteScope,
    RiskClass, SearchMode,
};
use serde_json::Value;

pub(super) fn parse_importance(value: &str) -> Option<MemoryImportance> {
    match value {
        "critical" => Some(MemoryImportance::Critical),
        "high" => Some(MemoryImportance::High),
        "medium" => Some(MemoryImportance::Medium),
        "low" => Some(MemoryImportance::Low),
        _ => None,
    }
}

pub(super) fn parse_recall_scope(value: &str) -> Option<MemoryScopeSelector> {
    match value {
        "project" => Some(MemoryScopeSelector::Project),
        "global" => Some(MemoryScopeSelector::Global),
        "project_and_global" => Some(MemoryScopeSelector::ProjectAndGlobal),
        _ => None,
    }
}

pub(super) fn parse_write_scope(value: &str) -> Option<MemoryWriteScope> {
    match value {
        "project" => Some(MemoryWriteScope::Project),
        "global" => Some(MemoryWriteScope::Global),
        _ => None,
    }
}

pub(super) fn parse_mode(value: &str) -> Option<SearchMode> {
    match value {
        "auto" => Some(SearchMode::Auto),
        "semantic" => Some(SearchMode::Semantic),
        "exact" => Some(SearchMode::Exact),
        _ => None,
    }
}

pub(super) fn parse_fidelity(value: &str) -> Option<FidelityClass> {
    match value {
        "exact" => Some(FidelityClass::Exact),
        "lossless_structural" => Some(FidelityClass::LosslessStructural),
        "semantic" => Some(FidelityClass::Semantic),
        "summary" => Some(FidelityClass::Summary),
        _ => None,
    }
}

pub(super) fn parse_risk(value: &str) -> Option<RiskClass> {
    match value {
        "low" => Some(RiskClass::Low),
        "medium" => Some(RiskClass::Medium),
        "high" => Some(RiskClass::High),
        "irreversible" => Some(RiskClass::Irreversible),
        _ => None,
    }
}

pub(super) fn parse_codec_profile(value: &str) -> Option<CodecProfile> {
    match value {
        "off" => Some(CodecProfile::Off),
        "safe" => Some(CodecProfile::Safe),
        "adaptive" => Some(CodecProfile::Adaptive),
        "compact" => Some(CodecProfile::Compact),
        "shadow" => Some(CodecProfile::Shadow),
        _ => None,
    }
}

pub(super) fn required_string(arguments: &Value, key: &str) -> Result<String> {
    match arguments.get(key) {
        Some(Value::String(value)) if !value.trim().is_empty() => Ok(value.to_owned()),
        Some(Value::String(_)) => anyhow::bail!("argument `{key}` must not be empty"),
        Some(_) => anyhow::bail!("argument `{key}` must be a string"),
        None => anyhow::bail!("missing required argument `{key}`"),
    }
}

pub(super) fn optional_string(arguments: &Value, key: &str) -> Result<Option<String>> {
    match arguments.get(key) {
        None => Ok(None),
        Some(Value::String(value)) if !value.trim().is_empty() => Ok(Some(value.to_owned())),
        Some(Value::String(_)) => anyhow::bail!("argument `{key}` must not be empty"),
        Some(_) => anyhow::bail!("argument `{key}` must be a string"),
    }
}

pub(super) fn optional_bool(arguments: &Value, key: &str, default: bool) -> Result<bool> {
    match arguments.get(key) {
        None => Ok(default),
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => anyhow::bail!("argument `{key}` must be a boolean"),
    }
}

pub(super) fn bounded_usize(
    arguments: &Value,
    key: &str,
    default: usize,
    maximum: usize,
) -> Result<usize> {
    match arguments.get(key) {
        None => Ok(default),
        Some(value) => {
            let value = value
                .as_u64()
                .with_context(|| format!("argument `{key}` must be a positive integer"))?;
            if value == 0 || value > maximum as u64 {
                anyhow::bail!("argument `{key}` must be between 1 and {maximum}");
            }
            Ok(value as usize)
        }
    }
}

pub(super) fn optional_enum<T: Copy>(
    arguments: &Value,
    key: &str,
    default: T,
    parse: fn(&str) -> Option<T>,
    allowed: &str,
) -> Result<T> {
    match arguments.get(key) {
        None => Ok(default),
        Some(Value::String(value)) => {
            parse(value).with_context(|| format!("argument `{key}` must be one of: {allowed}"))
        }
        Some(_) => anyhow::bail!("argument `{key}` must be a string"),
    }
}

pub(super) fn string_array(arguments: &Value, key: &str, maximum: usize) -> Result<Vec<String>> {
    let Some(value) = arguments.get(key) else {
        return Ok(Vec::new());
    };
    let values = value
        .as_array()
        .with_context(|| format!("argument `{key}` must be an array of strings"))?;
    if values.len() > maximum {
        anyhow::bail!("argument `{key}` must contain at most {maximum} entries");
    }
    values
        .iter()
        .enumerate()
        .map(|(index, value)| match value {
            Value::String(value) if !value.trim().is_empty() => Ok(value.to_owned()),
            Value::String(_) => anyhow::bail!("argument `{key}[{index}]` must not be empty"),
            _ => anyhow::bail!("argument `{key}[{index}]` must be a string"),
        })
        .collect()
}

pub(super) fn reject_unknown(arguments: &Value, allowed: &[&str]) -> Result<()> {
    let object = arguments
        .as_object()
        .context("tool arguments must be an object")?;
    if let Some(key) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        anyhow::bail!("unknown argument `{key}`");
    }
    Ok(())
}
