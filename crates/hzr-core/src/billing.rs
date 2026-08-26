use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

pub const BUILTIN_PRICING_CATALOG_IDENTITY: &str = "hzr-public-api-pricing-2026-08-26-v1";
pub const PRICING_CATALOG_SCHEMA_VERSION: u16 = 1;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PricingCatalog {
    pub schema_version: u16,
    pub identity: String,
    pub retrieved_at: String,
    #[serde(default)]
    pub max_age_days: Option<u64>,
    pub entries: Vec<PricingEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PricingEntry {
    pub version: String,
    pub harness: String,
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    pub method: String,
    pub currency: String,
    pub effective_at: String,
    #[serde(default)]
    pub retrieved_at: Option<String>,
    #[serde(default)]
    pub max_age_days: Option<u64>,
    #[serde(default)]
    pub valid_until: Option<String>,
    #[serde(default)]
    pub min_context_tokens: Option<u64>,
    #[serde(default)]
    pub max_context_tokens_exclusive: Option<u64>,
    pub source_url: String,
    pub unit: String,
    pub rates: TokenRates,
    pub note: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenRates {
    pub input_microunits_per_million: Option<u64>,
    pub output_microunits_per_million: Option<u64>,
    pub cache_read_microunits_per_million: Option<u64>,
    pub cache_write_5m_microunits_per_million: Option<u64>,
    pub cache_write_1h_microunits_per_million: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderTokenUsage {
    /// Provider-reported non-cached input tokens. Adapters must not include cache reads/writes here.
    pub input_tokens: u64,
    /// Provider-reported output total. Reasoning tokens are informational and already included here.
    pub output_tokens: u64,
    #[serde(default)]
    pub reasoning_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: u64,
    #[serde(default)]
    pub cache_write_5m_tokens: u64,
    #[serde(default)]
    pub cache_write_1h_tokens: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderEconomicReceipt {
    pub receipt_id: String,
    pub source: String,
    pub observed_at_ms: u64,
    pub harness: String,
    pub provider: String,
    pub model: String,
    pub method: String,
    pub currency: String,
    #[serde(default)]
    pub context_window_tokens: Option<u64>,
    pub session_id: String,
    pub project_path: String,
    pub baseline: ProviderTokenUsage,
    pub delivered: ProviderTokenUsage,
    #[serde(default)]
    pub actual_baseline_cost_microunits: Option<u64>,
    #[serde(default)]
    pub actual_delivered_cost_microunits: Option<u64>,
    #[serde(default)]
    pub enable_public_estimate: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderReceiptRecordResult {
    pub recorded: bool,
    pub idempotent_replay: bool,
    pub receipt_hash: String,
    pub reported_actual: Option<EconomicAmount>,
    pub provenance: ReceiptProvenance,
    pub externally_verified: bool,
    pub public_estimate: Option<PublicEstimate>,
    pub unavailable_reason: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptProvenance {
    #[default]
    UserSupplied,
}

impl ReceiptProvenance {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::UserSupplied => "user_supplied",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct EconomicAmount {
    pub currency: String,
    pub baseline_microunits: u64,
    pub delivered_microunits: u64,
    pub savings_microunits: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PublicEstimate {
    pub classification: String,
    pub preliminary: bool,
    pub amount: EconomicAmount,
    pub price_table_identity: String,
    pub entry_version: String,
    pub source_url: String,
    pub retrieved_at: String,
    pub disclaimer: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RawPublicEstimate {
    pub classification: String,
    pub preliminary: bool,
    pub potential: bool,
    pub avoided_input_tokens_estimated: u64,
    pub pricing_basis: String,
    pub harness: String,
    pub provider: String,
    pub model: String,
    pub method: String,
    pub currency: String,
    pub savings_microunits: u64,
    pub price_table_identity: String,
    pub entry_version: String,
    pub source_url: String,
    pub retrieved_at: String,
    pub disclaimer: String,
}

#[derive(Clone, Copy, Debug)]
pub struct RawPublicEstimateRequest<'a> {
    pub harness: &'a str,
    pub provider: &'a str,
    pub model: &'a str,
    pub method: &'a str,
    pub context_window_tokens: Option<u64>,
    pub basis: &'a str,
    pub avoided_tokens: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SessionEconomicSummary {
    pub paired_receipts: u64,
    pub reported_actual: Option<EconomicAmount>,
    pub provenance: Option<ReceiptProvenance>,
    pub externally_verified: bool,
    pub public_estimate: Option<EconomicAmount>,
    pub public_estimate_preliminary: bool,
    pub price_table_identities: Vec<String>,
    pub unavailable_reasons: Vec<String>,
}

#[derive(Debug, Error)]
pub enum BillingError {
    #[error("invalid pricing catalog: {0}")]
    InvalidCatalog(String),
    #[error("failed to read pricing catalog {path}: {source}")]
    CatalogRead {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    #[error("failed to parse pricing catalog {path}: {source}")]
    CatalogParse {
        path: std::path::PathBuf,
        source: serde_json::Error,
    },
    #[error("invalid provider receipt: {0}")]
    InvalidReceipt(String),
    #[error("pricing unavailable: {0}")]
    PricingUnavailable(String),
    #[error("economic arithmetic overflow")]
    ArithmeticOverflow,
}

pub fn builtin_pricing_catalog() -> Result<PricingCatalog, BillingError> {
    let mut catalog: PricingCatalog = serde_json::from_str(include_str!(
        "../../../data/pricing/public-api-pricing-2026-08-26-v1.json"
    ))
    .map_err(|source| BillingError::CatalogParse {
        path: Path::new("<embedded-public-pricing>").to_path_buf(),
        source,
    })?;
    normalize_catalog_entries(&mut catalog);
    validate_catalog(&catalog)?;
    Ok(catalog)
}

pub fn load_pricing_catalog(user_override: Option<&Path>) -> Result<PricingCatalog, BillingError> {
    let mut catalog = builtin_pricing_catalog()?;
    let Some(path) = user_override else {
        return Ok(catalog);
    };
    let bytes = std::fs::read(path).map_err(|source| BillingError::CatalogRead {
        path: path.to_path_buf(),
        source,
    })?;
    let mut override_catalog: PricingCatalog =
        serde_json::from_slice(&bytes).map_err(|source| BillingError::CatalogParse {
            path: path.to_path_buf(),
            source,
        })?;
    normalize_catalog_entries(&mut override_catalog);
    validate_catalog(&override_catalog)?;
    let mut merged = catalog
        .entries
        .into_iter()
        .map(|entry| (entry_key(&entry), entry))
        .collect::<BTreeMap<_, _>>();
    for entry in override_catalog.entries {
        merged.insert(entry_key(&entry), entry);
    }
    let override_sha256 = hex::encode(Sha256::digest(&bytes));
    catalog.identity = format!(
        "{}+{}@sha256:{}",
        catalog.identity, override_catalog.identity, override_sha256
    );
    catalog.retrieved_at = catalog.retrieved_at.max(override_catalog.retrieved_at);
    catalog.entries = merged.into_values().collect();
    validate_catalog(&catalog)?;
    Ok(catalog)
}

fn normalize_catalog_entries(catalog: &mut PricingCatalog) {
    for entry in &mut catalog.entries {
        if entry.retrieved_at.is_none() {
            entry.retrieved_at = Some(catalog.retrieved_at.clone());
        }
        if entry.max_age_days.is_none() {
            entry.max_age_days = catalog.max_age_days;
        }
    }
}

pub fn validate_receipt(receipt: &ProviderEconomicReceipt) -> Result<(), BillingError> {
    for (name, value, limit) in [
        ("receipt_id", receipt.receipt_id.as_str(), 256),
        ("source", receipt.source.as_str(), 128),
        ("harness", receipt.harness.as_str(), 64),
        ("provider", receipt.provider.as_str(), 64),
        ("model", receipt.model.as_str(), 128),
        ("method", receipt.method.as_str(), 64),
        ("currency", receipt.currency.as_str(), 8),
        ("session_id", receipt.session_id.as_str(), 512),
    ] {
        if value.is_empty()
            || value.len() > limit
            || !value.is_ascii()
            || value.chars().any(char::is_control)
        {
            return Err(BillingError::InvalidReceipt(format!("invalid {name}")));
        }
    }
    if receipt.project_path.len() > 4096 || receipt.project_path.contains('\0') {
        return Err(BillingError::InvalidReceipt("invalid project_path".into()));
    }
    if receipt
        .context_window_tokens
        .is_some_and(|tokens| tokens == 0)
    {
        return Err(BillingError::InvalidReceipt(
            "context_window_tokens must be positive when supplied".into(),
        ));
    }
    if receipt.baseline.reasoning_tokens > receipt.baseline.output_tokens
        || receipt.delivered.reasoning_tokens > receipt.delivered.output_tokens
    {
        return Err(BillingError::InvalidReceipt(
            "reasoning_tokens must be included in output_tokens".into(),
        ));
    }
    if receipt.actual_baseline_cost_microunits.is_some()
        != receipt.actual_delivered_cost_microunits.is_some()
    {
        return Err(BillingError::InvalidReceipt(
            "actual provider cost requires both baseline and delivered amounts".into(),
        ));
    }
    Ok(())
}

pub fn price_receipt(
    catalog: &PricingCatalog,
    receipt: &ProviderEconomicReceipt,
) -> Result<PublicEstimate, BillingError> {
    validate_receipt(receipt)?;
    let entry = find_entry(
        catalog,
        &receipt.harness,
        &receipt.provider,
        &receipt.model,
        &receipt.method,
        receipt.context_window_tokens,
    )?;
    if entry.currency != receipt.currency {
        return Err(BillingError::PricingUnavailable(format!(
            "catalog currency {} does not match receipt currency {}",
            entry.currency, receipt.currency
        )));
    }
    let baseline = price_usage(receipt.baseline, entry.rates)?;
    let delivered = price_usage(receipt.delivered, entry.rates)?;
    Ok(PublicEstimate {
        classification: "public_estimate".into(),
        preliminary: true,
        amount: EconomicAmount {
            currency: entry.currency.clone(),
            baseline_microunits: baseline,
            delivered_microunits: delivered,
            savings_microunits: signed_difference(baseline, delivered)?,
        },
        price_table_identity: catalog.identity.clone(),
        entry_version: entry.version.clone(),
        source_url: entry.source_url.clone(),
        retrieved_at: entry.retrieved_at.clone().unwrap_or_default(),
        disclaimer: "Preliminary public-list-price estimate; not a provider invoice or billed amount. Contract, region, tier, discounts, taxes, and tool fees may differ.".into(),
    })
}

pub fn price_avoided_input_tokens(
    catalog: &PricingCatalog,
    request: RawPublicEstimateRequest<'_>,
) -> Result<RawPublicEstimate, BillingError> {
    let entry = find_entry(
        catalog,
        request.harness,
        request.provider,
        request.model,
        request.method,
        request.context_window_tokens,
    )?;
    let rate = match request.basis {
        "input" => entry.rates.input_microunits_per_million,
        "cache_read" => entry.rates.cache_read_microunits_per_million,
        _ => {
            return Err(BillingError::PricingUnavailable(
                "pricing basis must be input or cache_read".into(),
            ));
        }
    }
    .ok_or_else(|| {
        BillingError::PricingUnavailable(format!("catalog has no {} rate", request.basis))
    })?;
    let numerator = u128::from(request.avoided_tokens)
        .checked_mul(u128::from(rate))
        .ok_or(BillingError::ArithmeticOverflow)?;
    let savings_microunits = u64::try_from(numerator.div_ceil(1_000_000))
        .map_err(|_| BillingError::ArithmeticOverflow)?;
    Ok(RawPublicEstimate {
        classification: "raw_public_estimate".into(),
        preliminary: true,
        potential: true,
        avoided_input_tokens_estimated: request.avoided_tokens,
        pricing_basis: request.basis.into(),
        harness: request.harness.into(),
        provider: request.provider.into(),
        model: request.model.into(),
        method: request.method.into(),
        currency: entry.currency.clone(),
        savings_microunits,
        price_table_identity: catalog.identity.clone(),
        entry_version: entry.version.clone(),
        source_url: entry.source_url.clone(),
        retrieved_at: entry.retrieved_at.clone().unwrap_or_default(),
        disclaimer: "Preliminary potential savings from estimated avoided tool-output tokens priced as future model input; not usage, an invoice, or a billed amount.".into(),
    })
}

pub fn receipt_payload_hash(receipt: &ProviderEconomicReceipt) -> Result<String, BillingError> {
    let bytes = serde_json::to_vec(receipt)
        .map_err(|error| BillingError::InvalidReceipt(error.to_string()))?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

fn validate_catalog(catalog: &PricingCatalog) -> Result<(), BillingError> {
    if catalog.schema_version != PRICING_CATALOG_SCHEMA_VERSION {
        return Err(BillingError::InvalidCatalog(format!(
            "schema_version must be {PRICING_CATALOG_SCHEMA_VERSION}"
        )));
    }
    if !valid_identifier(&catalog.identity, 192)
        || date_to_unix_days(&catalog.retrieved_at).is_none()
        || catalog.entries.len() > 2048
    {
        return Err(BillingError::InvalidCatalog(
            "invalid identity or too many entries".into(),
        ));
    }
    let mut keys = BTreeSet::new();
    let mut resolvable_names = BTreeSet::new();
    for entry in &catalog.entries {
        if !valid_identifier(&entry.version, 128)
            || !valid_identifier(&entry.harness, 64)
            || !valid_identifier(&entry.provider, 64)
            || !valid_identifier(&entry.model, 128)
            || !valid_identifier(&entry.method, 64)
            || !valid_identifier(&entry.currency, 8)
            || date_to_unix_days(&entry.effective_at).is_none()
            || entry
                .retrieved_at
                .as_deref()
                .is_none_or(|date| date_to_unix_days(date).is_none())
            || entry.max_age_days.is_some_and(|days| days == 0)
            || entry
                .valid_until
                .as_deref()
                .is_some_and(|date| date_to_unix_days(date).is_none())
            || entry
                .min_context_tokens
                .zip(entry.max_context_tokens_exclusive)
                .is_some_and(|(minimum, maximum)| minimum >= maximum)
            || entry.unit != "per_1m_tokens"
            || !entry.source_url.starts_with("https://")
        {
            return Err(BillingError::InvalidCatalog(format!(
                "invalid entry {}",
                entry.version
            )));
        }
        if !entry
            .aliases
            .iter()
            .all(|alias| valid_identifier(alias, 128))
        {
            return Err(BillingError::InvalidCatalog(format!(
                "invalid alias in {}",
                entry.version
            )));
        }
        if !keys.insert(entry_key(entry)) {
            return Err(BillingError::InvalidCatalog(format!(
                "duplicate pricing key for {}",
                entry.version
            )));
        }
        for name in std::iter::once(&entry.model).chain(entry.aliases.iter()) {
            if !resolvable_names.insert((
                entry.harness.clone(),
                entry.provider.clone(),
                entry.method.clone(),
                name.clone(),
            )) {
                return Err(BillingError::InvalidCatalog(format!(
                    "overlapping model or alias {name} for {}/{}/{}",
                    entry.harness, entry.provider, entry.method
                )));
            }
        }
    }
    Ok(())
}

fn price_usage(usage: ProviderTokenUsage, rates: TokenRates) -> Result<u64, BillingError> {
    let categories = [
        (
            "input",
            usage.input_tokens,
            rates.input_microunits_per_million,
        ),
        (
            "output",
            usage.output_tokens,
            rates.output_microunits_per_million,
        ),
        (
            "cache_read",
            usage.cache_read_tokens,
            rates.cache_read_microunits_per_million,
        ),
        (
            "cache_write_5m",
            usage.cache_write_5m_tokens,
            rates.cache_write_5m_microunits_per_million,
        ),
        (
            "cache_write_1h",
            usage.cache_write_1h_tokens,
            rates.cache_write_1h_microunits_per_million,
        ),
    ];
    categories
        .into_iter()
        .try_fold(0_u64, |total, (name, tokens, rate)| {
            if tokens == 0 {
                return Ok(total);
            }
            let rate = rate.ok_or_else(|| {
                BillingError::PricingUnavailable(format!("catalog has no {name} rate"))
            })?;
            let numerator = u128::from(tokens)
                .checked_mul(u128::from(rate))
                .ok_or(BillingError::ArithmeticOverflow)?;
            let rounded = numerator.div_ceil(1_000_000);
            let amount = u64::try_from(rounded).map_err(|_| BillingError::ArithmeticOverflow)?;
            total
                .checked_add(amount)
                .ok_or(BillingError::ArithmeticOverflow)
        })
}

fn signed_difference(baseline: u64, delivered: u64) -> Result<i64, BillingError> {
    let baseline = i128::from(baseline);
    let delivered = i128::from(delivered);
    i64::try_from(baseline - delivered).map_err(|_| BillingError::ArithmeticOverflow)
}

fn model_matches(entry: &PricingEntry, model: &str) -> bool {
    entry.model == model || entry.aliases.iter().any(|alias| alias == model)
}

fn find_entry<'a>(
    catalog: &'a PricingCatalog,
    harness: &str,
    provider: &str,
    model: &str,
    method: &str,
    context_window_tokens: Option<u64>,
) -> Result<&'a PricingEntry, BillingError> {
    let matches = catalog
        .entries
        .iter()
        .filter(|entry| {
            entry.harness == harness
                && entry.provider == provider
                && entry.method == method
                && model_matches(entry, model)
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [entry] => {
            let current_days = current_unix_days();
            if date_to_unix_days(&entry.effective_at)
                .is_none_or(|effective| current_days < effective)
            {
                return Err(BillingError::PricingUnavailable(format!(
                    "catalog entry {} is not effective until {}",
                    entry.version, entry.effective_at
                )));
            }
            if entry
                .valid_until
                .as_deref()
                .is_some_and(pricing_date_expired)
            {
                return Err(BillingError::PricingUnavailable(format!(
                    "catalog entry {} expired on {}",
                    entry.version,
                    entry.valid_until.as_deref().unwrap_or_default()
                )));
            }
            let retrieved_at = entry.retrieved_at.as_deref().unwrap_or_default();
            if entry.max_age_days.is_some_and(|max_age| {
                date_to_unix_days(retrieved_at).is_none_or(|retrieved| {
                    current_days
                        > retrieved.saturating_add(i64::try_from(max_age).unwrap_or(i64::MAX))
                })
            }) {
                return Err(BillingError::PricingUnavailable(format!(
                    "catalog entry {} is stale: retrieved {} with max_age_days={}",
                    entry.version,
                    retrieved_at,
                    entry.max_age_days.unwrap_or_default()
                )));
            }
            if entry.min_context_tokens.is_some() || entry.max_context_tokens_exclusive.is_some() {
                let context = context_window_tokens.ok_or_else(|| {
                    BillingError::PricingUnavailable(format!(
                        "catalog entry {} requires an explicit context_window_tokens value",
                        entry.version
                    ))
                })?;
                if entry
                    .min_context_tokens
                    .is_some_and(|minimum| context < minimum)
                    || entry
                        .max_context_tokens_exclusive
                        .is_some_and(|maximum| context >= maximum)
                {
                    return Err(BillingError::PricingUnavailable(format!(
                        "context_window_tokens={context} does not match catalog entry {}",
                        entry.version
                    )));
                }
            }
            Ok(entry)
        }
        [] => Err(BillingError::PricingUnavailable(format!(
            "no exact catalog entry for harness={harness} provider={provider} model={model} method={method}"
        ))),
        _ => Err(BillingError::PricingUnavailable(
            "model alias resolves to multiple catalog entries".into(),
        )),
    }
}

fn pricing_date_expired(valid_until: &str) -> bool {
    let Some(expiry_days) = date_to_unix_days(valid_until) else {
        return true;
    };
    current_unix_days() > expiry_days
}

fn current_unix_days() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_secs() / 86_400).unwrap_or(i64::MAX)
        })
}

fn date_to_unix_days(value: &str) -> Option<i64> {
    let mut parts = value.split('-');
    let year = parts.next()?.parse::<i64>().ok()?;
    let month = parts.next()?.parse::<i64>().ok()?;
    let day = parts.next()?.parse::<i64>().ok()?;
    if parts.next().is_some() || !(1..=12).contains(&month) {
        return None;
    }
    let leap_year =
        year.rem_euclid(4) == 0 && (year.rem_euclid(100) != 0 || year.rem_euclid(400) == 0);
    let days_in_month = match month {
        2 if leap_year => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    if !(1..=days_in_month).contains(&day) {
        return None;
    }
    let adjusted_year = year - i64::from(month <= 2);
    let era = adjusted_year.div_euclid(400);
    let year_of_era = adjusted_year - era * 400;
    let shifted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some(era * 146_097 + day_of_era - 719_468)
}

fn valid_identifier(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value.is_ascii()
        && !value.chars().any(char::is_control)
}

fn entry_key(entry: &PricingEntry) -> String {
    format!(
        "{}\0{}\0{}\0{}",
        entry.harness, entry.provider, entry.model, entry.method
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt(model: &str) -> ProviderEconomicReceipt {
        ProviderEconomicReceipt {
            receipt_id: "provider-receipt-1".into(),
            source: "provider-api".into(),
            observed_at_ms: 1,
            harness: "codex".into(),
            provider: "openai".into(),
            model: model.into(),
            method: "standard_short_context_lte_272k".into(),
            currency: "USD".into(),
            context_window_tokens: Some(100_000),
            session_id: "private-session".into(),
            project_path: "/work/project".into(),
            baseline: ProviderTokenUsage {
                input_tokens: 1_000_000,
                output_tokens: 100_000,
                ..ProviderTokenUsage::default()
            },
            delivered: ProviderTokenUsage {
                input_tokens: 500_000,
                output_tokens: 50_000,
                ..ProviderTokenUsage::default()
            },
            actual_baseline_cost_microunits: None,
            actual_delivered_cost_microunits: None,
            enable_public_estimate: true,
        }
    }

    #[test]
    fn public_catalog_prices_exact_and_documented_aliases() {
        let catalog = builtin_pricing_catalog().expect("catalog");
        let exact = price_receipt(&catalog, &receipt("gpt-5.6-sol")).expect("exact price");
        let alias = price_receipt(&catalog, &receipt("gpt-5.6")).expect("documented alias");

        assert_eq!(exact.amount, alias.amount);
        assert_eq!(exact.amount.baseline_microunits, 6_000_000);
        assert_eq!(exact.classification, "public_estimate");
        assert!(exact.preliminary);
        assert!(exact.disclaimer.contains("not a provider invoice"));
    }

    #[test]
    fn unknown_model_or_method_never_falls_back() {
        let catalog = builtin_pricing_catalog().expect("catalog");
        assert!(matches!(
            price_receipt(&catalog, &receipt("unknown-model")),
            Err(BillingError::PricingUnavailable(_))
        ));
        assert!(price_receipt(&catalog, &receipt("gpt-5.6-sol-2099-01-01")).is_err());
        let mut unknown_method = receipt("gpt-5.6-sol");
        unknown_method.method = "subscription".into();
        assert!(price_receipt(&catalog, &unknown_method).is_err());
    }

    #[test]
    fn user_catalog_replaces_only_an_exact_pricing_key() {
        let directory = tempfile::tempdir().expect("temp directory");
        let path = directory.path().join("pricing.json");
        let mut override_catalog = PricingCatalog {
            schema_version: 1,
            identity: "customer-contract-2026-v1".into(),
            retrieved_at: "2026-08-26".into(),
            max_age_days: Some(30),
            entries: vec![builtin_pricing_catalog().expect("builtin").entries[0].clone()],
        };
        override_catalog.entries[0]
            .rates
            .input_microunits_per_million = Some(1);
        std::fs::write(&path, serde_json::to_vec(&override_catalog).expect("json"))
            .expect("write override");

        let merged = load_pricing_catalog(Some(&path)).expect("merged");
        let estimate = price_receipt(&merged, &receipt("gpt-5.6-sol")).expect("override price");

        assert!(
            estimate
                .price_table_identity
                .contains("customer-contract-2026-v1")
        );
        assert_eq!(estimate.amount.baseline_microunits, 2_000_001);
    }

    #[test]
    fn explicit_missing_override_is_an_error() {
        let directory = tempfile::tempdir().expect("temp directory");
        let path = directory.path().join("missing-pricing.json");

        assert!(matches!(
            load_pricing_catalog(Some(&path)),
            Err(BillingError::CatalogRead { .. })
        ));
    }

    #[test]
    fn raw_estimate_uses_selected_input_basis_and_native_currency() {
        let catalog = builtin_pricing_catalog().expect("catalog");
        let estimate = price_avoided_input_tokens(
            &catalog,
            RawPublicEstimateRequest {
                harness: "openai_compatible",
                provider: "alibaba_model_studio",
                model: "qwen3.5-plus",
                method: "global_standard_0_128k",
                context_window_tokens: Some(100_000),
                basis: "input",
                avoided_tokens: 1_000_000,
            },
        )
        .expect("raw estimate");

        assert_eq!(estimate.classification, "raw_public_estimate");
        assert_eq!(estimate.currency, "CNY");
        assert_eq!(estimate.savings_microunits, 800_000);
        assert!(estimate.disclaimer.contains("not usage"));
    }

    #[test]
    fn xai_context_tiers_require_explicit_matching_evidence() {
        let catalog = builtin_pricing_catalog().expect("catalog");
        let estimate = |method, context| {
            price_avoided_input_tokens(
                &catalog,
                RawPublicEstimateRequest {
                    harness: "openai_compatible",
                    provider: "xai",
                    model: "grok-4.6",
                    method,
                    context_window_tokens: context,
                    basis: "input",
                    avoided_tokens: 1_000_000,
                },
            )
        };

        assert!(estimate("standard_short_context_lt_200k", None).is_err());
        assert_eq!(
            estimate("standard_short_context_lt_200k", Some(199_999))
                .expect("short tier")
                .savings_microunits,
            2_000_000
        );
        assert!(estimate("standard_short_context_lt_200k", Some(200_000)).is_err());
        assert_eq!(
            estimate("standard_long_context_gte_200k", Some(200_000))
                .expect("long tier")
                .savings_microunits,
            4_000_000
        );
    }

    #[test]
    fn openai_long_context_multiplier_is_explicit_and_bounded() {
        let catalog = builtin_pricing_catalog().expect("catalog");
        let mut short = receipt("gpt-5.6-sol");
        short.context_window_tokens = None;
        assert!(price_receipt(&catalog, &short).is_err());
        short.context_window_tokens = Some(272_000);
        assert_eq!(
            price_receipt(&catalog, &short)
                .expect("short tier")
                .amount
                .baseline_microunits,
            6_000_000
        );
        short.context_window_tokens = Some(272_001);
        assert!(price_receipt(&catalog, &short).is_err());

        let mut long = short;
        long.method = "standard_long_context_gt_272k".into();
        assert_eq!(
            price_receipt(&catalog, &long)
                .expect("long tier")
                .amount
                .baseline_microunits,
            11_000_000
        );
    }

    #[test]
    fn missing_cache_rate_is_unavailable_instead_of_zero() {
        let catalog = builtin_pricing_catalog().expect("catalog");
        let mut value = receipt("gpt-5.3-codex");
        value.method = "standard".into();
        value.context_window_tokens = None;
        value.baseline.cache_write_5m_tokens = 10;

        let error = price_receipt(&catalog, &value).expect_err("missing rate");

        assert!(error.to_string().contains("cache_write_5m"));
    }

    #[test]
    fn receipt_hash_is_stable_and_excludes_plaintext_from_result() {
        let value = receipt("gpt-5.6-sol");
        let first = receipt_payload_hash(&value).expect("hash");
        let second = receipt_payload_hash(&value).expect("hash");

        assert_eq!(first, second);
        assert_eq!(first.len(), 64);
        assert!(!first.contains("private-session"));
    }

    #[test]
    fn expired_promotional_price_fails_closed() {
        let mut catalog = builtin_pricing_catalog().expect("catalog");
        catalog.entries[0].valid_until = Some("2020-01-01".into());

        let error = price_receipt(&catalog, &receipt("gpt-5.6-sol"))
            .expect_err("expired entry must not price usage");

        assert!(error.to_string().contains("expired on 2020-01-01"));
    }

    #[test]
    fn future_and_stale_prices_fail_closed() {
        let mut future = builtin_pricing_catalog().expect("catalog");
        future.entries[0].effective_at = "2099-01-01".into();
        assert!(
            price_receipt(&future, &receipt("gpt-5.6-sol"))
                .expect_err("future price")
                .to_string()
                .contains("not effective")
        );

        let mut stale = builtin_pricing_catalog().expect("catalog");
        stale.entries[0].retrieved_at = Some("2020-01-01".into());
        stale.entries[0].max_age_days = Some(1);
        assert!(
            price_receipt(&stale, &receipt("gpt-5.6-sol"))
                .expect_err("stale price")
                .to_string()
                .contains("is stale")
        );
    }

    #[test]
    fn merged_override_revalidates_aliases_against_builtins() {
        let directory = tempfile::tempdir().expect("temp directory");
        let path = directory.path().join("pricing.json");
        let builtin = builtin_pricing_catalog().expect("builtin");
        let mut colliding = builtin.entries[0].clone();
        colliding.model = "customer-model".into();
        colliding.aliases = vec!["gpt-5.6-sol".into()];
        let override_catalog = PricingCatalog {
            schema_version: PRICING_CATALOG_SCHEMA_VERSION,
            identity: "customer-collision-v1".into(),
            retrieved_at: "2026-08-26".into(),
            max_age_days: Some(30),
            entries: vec![colliding],
        };
        std::fs::write(
            &path,
            serde_json::to_vec(&override_catalog).expect("override JSON"),
        )
        .expect("write override");

        assert!(matches!(
            load_pricing_catalog(Some(&path)),
            Err(BillingError::InvalidCatalog(_))
        ));
    }
}
