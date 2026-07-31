//! Tabular digest strategies for CSV/TSV files.
//! Extracted from read.rs (PR-2).

use crate::filter::FilterLevel;
use anyhow::{Context, Result};
use std::io::Cursor;
use std::path::Path;

const TABULAR_PREVIEW_ROWS: usize = 5;
const TABULAR_PREVIEW_HEAD_COLS: usize = 6;
const TABULAR_PREVIEW_TAIL_COLS: usize = 3;
const TABULAR_NUMERIC_STATS_LIMIT: usize = 8;
const TABULAR_MAX_CELL_CHARS: usize = 24;
const TABULAR_ANALYSIS_MAX_ROWS: usize = 2048;
const TABULAR_AGGRESSIVE_ANALYSIS_MAX_ROWS: usize = 512;
const TABULAR_AGGRESSIVE_PREVIEW_ROWS: usize = 2;

// ── Public API ──────────────────────────────────────────────

/// Check whether a tabular digest should be used for the given read parameters.
pub fn should_use_tabular_digest(
    file: &Path,
    level: FilterLevel,
    from: Option<usize>,
    to: Option<usize>,
    max_lines: Option<usize>,
    line_numbers: bool,
) -> bool {
    level != FilterLevel::None
        && from.is_none()
        && to.is_none()
        && max_lines.is_none()
        && !line_numbers
        && tabular_delimiter(file).is_some()
}

/// Detect tabular delimiter from file extension.
pub fn tabular_delimiter(file: &Path) -> Option<u8> {
    match file.extension().and_then(|ext| ext.to_str()) {
        Some(ext) if ext.eq_ignore_ascii_case("csv") => Some(b','),
        Some(ext) if ext.eq_ignore_ascii_case("tsv") => Some(b'\t'),
        _ => None,
    }
}

/// Build a compact digest of tabular data.
pub fn build_tabular_digest(
    content_bytes: &[u8],
    delimiter: u8,
    level: FilterLevel,
) -> Result<String> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .delimiter(delimiter)
        .from_reader(Cursor::new(content_bytes));
    let plan = tabular_digest_plan(level);
    let headers = reader
        .byte_headers()
        .context("Failed to parse tabular headers")?
        .clone();
    let col_count = headers.len();
    let row_count_estimate = estimate_tabular_rows(content_bytes);

    if col_count == 0 {
        return Ok(
            "Tabular digest\nRows: 0\nColumns: 0\nTip: use `rtk read <file> --level none` for exact output.\n"
                .to_string(),
        );
    }

    let mut sample_rows: Vec<Vec<String>> = Vec::with_capacity(plan.sample_rows);
    let mut numeric_stats = vec![NumericColumnStats::default(); col_count];
    let mut numeric_candidate = vec![true; col_count];
    let mut analyzed_rows = 0usize;

    let mut total_cells = 0usize;
    let mut empty_cells = 0usize;
    let mut minus_one_cells = 0usize;
    let mut record = csv::ByteRecord::new();

    while analyzed_rows < plan.analysis_max_rows {
        let has_more = reader
            .read_byte_record(&mut record)
            .context("Failed to parse tabular row")?;
        if !has_more {
            break;
        }
        analyzed_rows += 1;

        if sample_rows.len() < plan.sample_rows {
            let row_preview = record
                .iter()
                .map(|field| truncate_cell(&String::from_utf8_lossy(field)))
                .collect::<Vec<_>>();
            sample_rows.push(row_preview);
        }

        for (col, stats) in numeric_stats.iter_mut().enumerate().take(col_count) {
            total_cells += 1;
            let field = trim_ascii_bytes(record.get(col).unwrap_or(b""));
            if field.is_empty() {
                empty_cells += 1;
                continue;
            }
            if field == b"-1" {
                minus_one_cells += 1;
                continue;
            }
            if !plan.include_numeric_stats {
                continue;
            }
            if !numeric_candidate[col] {
                continue;
            }
            match parse_f64_ascii(field) {
                Some(value) => stats.update(value),
                None => {
                    numeric_candidate[col] = false;
                    *stats = NumericColumnStats::default();
                }
            }
        }
    }

    let delimiter_name = if delimiter == b'\t' { "TSV" } else { "CSV" };
    let mut out = String::new();
    out.push_str(&format!("Tabular digest ({delimiter_name})\n"));
    out.push_str(&format!(
        "Rows (excluding header, approx): {row_count_estimate}\n"
    ));
    out.push_str(&format!("Columns: {col_count}\n"));
    if analyzed_rows > 0 {
        let coverage =
            (analyzed_rows as f64 / row_count_estimate.max(analyzed_rows) as f64) * 100.0;
        out.push_str(&format!(
            "Analyzed rows for stats: {analyzed_rows} ({coverage:.2}% sample)\n"
        ));
    }
    if total_cells > 0 {
        let empty_pct = (empty_cells as f64 / total_cells as f64) * 100.0;
        let minus_one_pct = (minus_one_cells as f64 / total_cells as f64) * 100.0;
        out.push_str(&format!(
            "Sampled empty cells: {empty_cells}/{total_cells} ({empty_pct:.2}%)\n"
        ));
        out.push_str(&format!(
            "Sampled '-1' markers: {minus_one_cells}/{total_cells} ({minus_one_pct:.2}%)\n"
        ));
    }

    let header_preview = headers
        .iter()
        .enumerate()
        .map(|(idx, h)| format!("{}:{}", idx + 1, truncate_cell(&String::from_utf8_lossy(h))))
        .collect::<Vec<_>>();
    out.push_str(&format!(
        "Header preview: {}\n",
        preview_fields(&header_preview)
    ));

    if sample_rows.is_empty() {
        out.push_str("Sample rows: (none)\n");
    } else {
        out.push_str(&format!("Sample rows (first {}):\n", sample_rows.len()));
        for (i, row) in sample_rows.iter().enumerate() {
            out.push_str(&format!("  {}. {}\n", i + 1, preview_fields(row)));
        }
    }

    if plan.include_numeric_stats {
        let numeric_lines = numeric_stats
            .iter()
            .enumerate()
            .filter_map(|(idx, stats)| {
                if stats.count == 0 {
                    return None;
                }
                let mean = stats.sum / stats.count as f64;
                let col_name = headers.get(idx).unwrap_or(b"");
                Some(format!(
                    "  - {}: n={}, min={}, max={}, mean={}",
                    truncate_cell(&String::from_utf8_lossy(col_name)),
                    stats.count,
                    format_value(stats.min),
                    format_value(stats.max),
                    format_value(mean)
                ))
            })
            .take(TABULAR_NUMERIC_STATS_LIMIT)
            .collect::<Vec<_>>();

        if !numeric_lines.is_empty() {
            out.push_str(&format!(
                "Numeric stats (first {} numeric columns):\n",
                numeric_lines.len()
            ));
            for line in numeric_lines {
                out.push_str(&line);
                out.push('\n');
            }
        }
    }

    out.push_str("Tip: use `rtk read <file> --level none --from N --to M` for exact row ranges.\n");
    Ok(out)
}

// ── Internal helpers ────────────────────────────────────────

#[derive(Clone, Debug, Default)]
struct NumericColumnStats {
    count: usize,
    sum: f64,
    min: f64,
    max: f64,
}

impl NumericColumnStats {
    fn update(&mut self, value: f64) {
        if self.count == 0 {
            self.count = 1;
            self.sum = value;
            self.min = value;
            self.max = value;
            return;
        }
        self.count += 1;
        self.sum += value;
        self.min = self.min.min(value);
        self.max = self.max.max(value);
    }
}

struct TabularDigestPlan {
    analysis_max_rows: usize,
    sample_rows: usize,
    include_numeric_stats: bool,
}

fn tabular_digest_plan(level: FilterLevel) -> TabularDigestPlan {
    match level {
        FilterLevel::Aggressive => TabularDigestPlan {
            analysis_max_rows: TABULAR_AGGRESSIVE_ANALYSIS_MAX_ROWS,
            sample_rows: TABULAR_AGGRESSIVE_PREVIEW_ROWS,
            include_numeric_stats: false,
        },
        _ => TabularDigestPlan {
            analysis_max_rows: TABULAR_ANALYSIS_MAX_ROWS,
            sample_rows: TABULAR_PREVIEW_ROWS,
            include_numeric_stats: true,
        },
    }
}

fn format_value(value: f64) -> String {
    if (value.fract()).abs() < 1e-9 {
        format!("{value:.0}")
    } else {
        format!("{value:.3}")
    }
}

fn truncate_cell(cell: &str) -> String {
    let value = cell.trim();
    let char_count = value.chars().count();
    if char_count <= TABULAR_MAX_CELL_CHARS {
        return value.to_string();
    }
    value
        .chars()
        .take(TABULAR_MAX_CELL_CHARS.saturating_sub(1))
        .collect::<String>()
        + "…"
}

fn preview_fields(fields: &[String]) -> String {
    if fields.is_empty() {
        return "(none)".to_string();
    }

    if fields.len() <= TABULAR_PREVIEW_HEAD_COLS + TABULAR_PREVIEW_TAIL_COLS {
        return fields.join(" | ");
    }

    let omitted = fields.len() - TABULAR_PREVIEW_HEAD_COLS - TABULAR_PREVIEW_TAIL_COLS;
    let mut preview = Vec::with_capacity(TABULAR_PREVIEW_HEAD_COLS + TABULAR_PREVIEW_TAIL_COLS + 1);
    preview.extend(fields.iter().take(TABULAR_PREVIEW_HEAD_COLS).cloned());
    preview.push(format!("... ({omitted} cols omitted) ..."));
    preview.extend(
        fields
            .iter()
            .skip(fields.len().saturating_sub(TABULAR_PREVIEW_TAIL_COLS))
            .cloned(),
    );
    preview.join(" | ")
}

fn trim_ascii_bytes(mut bytes: &[u8]) -> &[u8] {
    while let Some(first) = bytes.first() {
        if !first.is_ascii_whitespace() {
            break;
        }
        bytes = &bytes[1..];
    }
    while let Some(last) = bytes.last() {
        if !last.is_ascii_whitespace() {
            break;
        }
        bytes = &bytes[..bytes.len().saturating_sub(1)];
    }
    bytes
}

fn parse_f64_ascii(bytes: &[u8]) -> Option<f64> {
    let text = std::str::from_utf8(bytes).ok()?;
    text.parse::<f64>().ok()
}

// ── Filename-based special format digests (PR-6) ────────────

/// Check if a file has a special format digest strategy.
pub fn has_special_digest(file: &Path) -> bool {
    special_strategy(file).is_some()
}

/// Try to build a special format digest for the file.
/// Returns None if no strategy matches or the strategy fails.
pub fn try_special_digest(file: &Path, content: &str, level: FilterLevel) -> Option<String> {
    let strategy = special_strategy(file)?;
    match strategy(content, level) {
        Ok(digest) => Some(digest),
        Err(_) => None, // fallback to normal read on error
    }
}

type DigestStrategy = fn(&str, FilterLevel) -> Result<String>;

fn special_strategy(file: &Path) -> Option<DigestStrategy> {
    let name = file.file_name()?.to_str()?;
    let name_lower = name.to_lowercase();

    // Lock files
    if name_lower == "cargo.lock"
        || name_lower == "pnpm-lock.yaml"
        || name_lower == "yarn.lock"
        || name_lower == "package-lock.json"
        || name_lower == "poetry.lock"
        || name_lower == "composer.lock"
        || name_lower == "gemfile.lock"
    {
        return Some(digest_lock_file);
    }

    // Package manifests
    if name_lower == "package.json" {
        return Some(digest_package_json);
    }
    if name_lower == "cargo.toml" {
        return Some(digest_cargo_toml);
    }

    // Config files
    if name_lower.starts_with("tsconfig") && name_lower.ends_with(".json") {
        return Some(digest_json_config);
    }
    if name_lower.starts_with("biome") && name_lower.ends_with(".json") {
        return Some(digest_json_config);
    }

    // Env files
    if name_lower.starts_with(".env") {
        return Some(digest_env_file);
    }

    // Dockerfiles
    if name_lower == "dockerfile" || name_lower.starts_with("dockerfile.") {
        return Some(digest_dockerfile);
    }

    // Generated files
    if let Some(ext) = file.extension().and_then(|e| e.to_str()) {
        let stem = file.file_stem()?.to_str()?;
        if stem.ends_with(".generated") || stem.ends_with(".g") {
            return Some(digest_generated_file);
        }
        // Markdown
        if ext == "md" || ext == "mdx" {
            return Some(digest_markdown);
        }
    }

    None
}

fn digest_lock_file(content: &str, _level: FilterLevel) -> Result<String> {
    let line_count = content.lines().count();
    let byte_count = content.len();

    // Count unique packages/entries
    let mut packages = 0usize;
    for line in content.lines() {
        // Cargo.lock: [[package]]
        if line.starts_with("[[package]]") || line.starts_with("name = ") {
            packages += 1;
        }
        // pnpm/yarn/npm: lines starting with package-like patterns
        if line.starts_with("  ") && line.contains('@') && line.ends_with(':') {
            packages += 1;
        }
    }

    let mut out = String::new();
    out.push_str(&format!(
        "Lock file digest ({} lines, {} bytes)\n",
        line_count, byte_count
    ));
    if packages > 0 {
        out.push_str(&format!("Packages/entries: ~{packages}\n"));
    }
    out.push_str("Tip: use `rtk read <file> --level none` for full content.\n");
    Ok(out)
}

fn digest_package_json(content: &str, level: FilterLevel) -> Result<String> {
    let parsed: serde_json::Value =
        serde_json::from_str(content).context("Failed to parse package.json")?;

    let mut out = String::new();
    out.push_str("package.json digest\n");

    if let Some(name) = parsed.get("name").and_then(|v| v.as_str()) {
        out.push_str(&format!("  name: {name}\n"));
    }
    if let Some(version) = parsed.get("version").and_then(|v| v.as_str()) {
        out.push_str(&format!("  version: {version}\n"));
    }

    // Scripts
    if let Some(scripts) = parsed.get("scripts").and_then(|v| v.as_object()) {
        let keys: Vec<&str> = scripts.keys().map(|k| k.as_str()).collect();
        let limit = if level == FilterLevel::Aggressive {
            5
        } else {
            10
        };
        let shown: Vec<&str> = keys.iter().take(limit).copied().collect();
        out.push_str(&format!(
            "  scripts ({}): {}\n",
            keys.len(),
            shown.join(", ")
        ));
        if keys.len() > limit {
            out.push_str(&format!("    ... +{} more\n", keys.len() - limit));
        }
    }

    // Dependencies summary
    for section in &[
        "dependencies",
        "devDependencies",
        "peerDependencies",
        "optionalDependencies",
    ] {
        if let Some(deps) = parsed.get(*section).and_then(|v| v.as_object()) {
            out.push_str(&format!("  {} ({})\n", section, deps.len()));
        }
    }

    Ok(out)
}

fn digest_cargo_toml(content: &str, _level: FilterLevel) -> Result<String> {
    let mut out = String::new();
    out.push_str("Cargo.toml digest\n");

    let mut in_section = String::new();
    let mut dep_count = 0usize;
    let mut dev_dep_count = 0usize;
    let mut features: Vec<String> = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_section = trimmed.to_string();
            continue;
        }
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        if in_section == "[package]" {
            if let Some((key, value)) = trimmed.split_once('=') {
                let key = key.trim();
                let value = value.trim().trim_matches('"');
                if matches!(key, "name" | "version" | "edition") {
                    out.push_str(&format!("  {key}: {value}\n"));
                }
            }
        }

        if in_section == "[dependencies]" || in_section.starts_with("[dependencies.") {
            dep_count += 1;
        }
        if in_section == "[dev-dependencies]" || in_section.starts_with("[dev-dependencies.") {
            dev_dep_count += 1;
        }
        if in_section == "[features]" {
            if let Some((key, _)) = trimmed.split_once('=') {
                features.push(key.trim().to_string());
            }
        }
    }

    if dep_count > 0 {
        out.push_str(&format!("  dependencies: {dep_count}\n"));
    }
    if dev_dep_count > 0 {
        out.push_str(&format!("  dev-dependencies: {dev_dep_count}\n"));
    }
    if !features.is_empty() {
        out.push_str(&format!("  features: {}\n", features.join(", ")));
    }

    Ok(out)
}

fn digest_json_config(content: &str, _level: FilterLevel) -> Result<String> {
    let parsed: serde_json::Value =
        serde_json::from_str(content).context("Failed to parse JSON config")?;

    let mut out = String::new();
    out.push_str("JSON config digest\n");

    if let Some(obj) = parsed.as_object() {
        for (key, value) in obj {
            match value {
                serde_json::Value::Object(inner) => {
                    out.push_str(&format!("  {key}: {{...}} ({} keys)\n", inner.len()));
                }
                serde_json::Value::Array(arr) => {
                    out.push_str(&format!("  {key}: [...] ({} items)\n", arr.len()));
                }
                serde_json::Value::String(s) => {
                    let display = if s.len() > 50 {
                        format!("{}...", s.chars().take(50).collect::<String>().as_str())
                    } else {
                        s.clone()
                    };
                    out.push_str(&format!("  {key}: \"{display}\"\n"));
                }
                other => {
                    out.push_str(&format!("  {key}: {other}\n"));
                }
            }
        }
    }

    Ok(out)
}

fn digest_env_file(content: &str, _level: FilterLevel) -> Result<String> {
    let mut out = String::new();
    out.push_str(".env digest (keys only, values masked)\n");

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((key, _)) = trimmed.split_once('=') {
            out.push_str(&format!("  {key}=***\n"));
        }
    }

    Ok(out)
}

fn digest_dockerfile(content: &str, level: FilterLevel) -> Result<String> {
    let mut out = String::new();
    out.push_str("Dockerfile digest\n");

    let key_instructions = [
        "FROM",
        "RUN",
        "CMD",
        "ENTRYPOINT",
        "EXPOSE",
        "COPY",
        "ADD",
        "ENV",
        "ARG",
        "WORKDIR",
        "VOLUME",
        "USER",
        "LABEL",
    ];

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let upper = trimmed
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_uppercase();
        if key_instructions.contains(&upper.as_str()) {
            let max_len = if level == FilterLevel::Aggressive {
                80
            } else {
                120
            };
            let display = if trimmed.len() > max_len {
                format!("{}...", &trimmed[..max_len])
            } else {
                trimmed.to_string()
            };
            out.push_str(&format!("  {display}\n"));
        }
    }

    Ok(out)
}

fn digest_generated_file(content: &str, _level: FilterLevel) -> Result<String> {
    let line_count = content.lines().count();
    Ok(format!(
        "Generated file ({line_count} lines). Use `rtk read <file> --level none` for content.\n"
    ))
}

fn digest_markdown(content: &str, level: FilterLevel) -> Result<String> {
    let mut out = String::new();
    out.push_str("Markdown digest\n");

    let total_lines = content.lines().count();
    out.push_str(&format!("  Lines: {total_lines}\n"));

    let mut headers: Vec<String> = Vec::new();
    let header_limit = if level == FilterLevel::Aggressive {
        10
    } else {
        20
    };

    for line in content.lines() {
        if line.starts_with('#') {
            let level_marker = line.chars().take_while(|c| *c == '#').count();
            let indent = "  ".repeat(level_marker);
            let text = line.trim_start_matches('#').trim();
            headers.push(format!("{indent}{text}"));
            if headers.len() >= header_limit {
                break;
            }
        }
    }

    if !headers.is_empty() {
        out.push_str("  Sections:\n");
        for h in &headers {
            out.push_str(&format!("    {h}\n"));
        }
    }

    Ok(out)
}

// ── Long-line truncation (PR-6) ─────────────────────────────

/// Truncate lines exceeding max width. Only applies in minimal/aggressive modes.
pub fn truncate_long_lines(content: &str, level: FilterLevel) -> String {
    if level == FilterLevel::None {
        return content.to_string();
    }

    let max_width = match level {
        FilterLevel::Aggressive => 200,
        FilterLevel::Minimal => 500,
        FilterLevel::None => unreachable!(),
    };

    let mut out = String::new();
    for line in content.lines() {
        if let Some((byte_idx, _)) = line.char_indices().nth(max_width) {
            out.push_str(&line[..byte_idx]);
            out.push_str("…\n");
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }

    // Remove trailing newline if original didn't have one
    if !content.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }

    out
}

fn estimate_tabular_rows(content_bytes: &[u8]) -> usize {
    if content_bytes.is_empty() {
        return 0;
    }
    let mut lines = content_bytes.iter().filter(|&&b| b == b'\n').count();
    if !content_bytes.ends_with(b"\n") {
        lines += 1;
    }
    lines.saturating_sub(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_use_digest_for_csv() {
        let p = Path::new("data.csv");
        assert!(should_use_tabular_digest(
            p,
            FilterLevel::Minimal,
            None,
            None,
            None,
            false
        ));
        assert!(!should_use_tabular_digest(
            p,
            FilterLevel::None,
            None,
            None,
            None,
            false
        ));
        assert!(!should_use_tabular_digest(
            p,
            FilterLevel::Minimal,
            Some(1),
            None,
            None,
            false
        ));
    }

    #[test]
    fn tabular_delimiter_csv_tsv() {
        assert_eq!(tabular_delimiter(Path::new("x.csv")), Some(b','));
        assert_eq!(tabular_delimiter(Path::new("x.tsv")), Some(b'\t'));
        assert_eq!(tabular_delimiter(Path::new("x.txt")), None);
    }

    #[test]
    fn digest_csv_basic() -> Result<()> {
        let csv = b"id,a,b\n1,10,-1\n2,20,30\n";
        let digest = build_tabular_digest(csv, b',', FilterLevel::Minimal)?;
        assert!(digest.contains("Tabular digest (CSV)"));
        assert!(digest.contains("Columns: 3"));
        assert!(digest.contains("Numeric stats"));
        Ok(())
    }

    #[test]
    fn digest_aggressive_no_numeric_stats() -> Result<()> {
        let csv = b"id,a\n1,10\n2,20\n";
        let digest = build_tabular_digest(csv, b',', FilterLevel::Aggressive)?;
        assert!(!digest.contains("Numeric stats"));
        Ok(())
    }

    #[test]
    fn estimate_rows_basic() {
        assert_eq!(estimate_tabular_rows(b""), 0);
        assert_eq!(estimate_tabular_rows(b"a,b\n"), 0);
        assert_eq!(estimate_tabular_rows(b"a,b\n1,2\n"), 1);
        assert_eq!(estimate_tabular_rows(b"a,b\n1,2"), 1);
    }

    #[test]
    fn truncate_cell_short() {
        assert_eq!(truncate_cell("hello"), "hello");
    }

    #[test]
    fn truncate_cell_long() {
        let long = "a".repeat(30);
        let result = truncate_cell(&long);
        assert!(result.ends_with('…'));
        assert!(result.chars().count() <= TABULAR_MAX_CELL_CHARS);
    }

    #[test]
    fn preview_fields_small() {
        let fields: Vec<String> = vec!["a".into(), "b".into(), "c".into()];
        assert_eq!(preview_fields(&fields), "a | b | c");
    }

    #[test]
    fn preview_fields_empty() {
        assert_eq!(preview_fields(&[]), "(none)");
    }

    // ── Special format digest tests (PR-6) ──────────────────

    #[test]
    fn special_strategy_lock_files() {
        assert!(has_special_digest(Path::new("Cargo.lock")));
        assert!(has_special_digest(Path::new("pnpm-lock.yaml")));
        assert!(has_special_digest(Path::new("yarn.lock")));
        assert!(has_special_digest(Path::new("package-lock.json")));
        assert!(!has_special_digest(Path::new("main.rs")));
    }

    #[test]
    fn special_strategy_package_json() {
        assert!(has_special_digest(Path::new("package.json")));
    }

    #[test]
    fn special_strategy_cargo_toml() {
        assert!(has_special_digest(Path::new("Cargo.toml")));
    }

    #[test]
    fn special_strategy_env() {
        assert!(has_special_digest(Path::new(".env")));
        assert!(has_special_digest(Path::new(".env.local")));
        assert!(has_special_digest(Path::new(".env.production")));
    }

    #[test]
    fn special_strategy_dockerfile() {
        assert!(has_special_digest(Path::new("Dockerfile")));
        assert!(has_special_digest(Path::new("Dockerfile.prod")));
    }

    #[test]
    fn special_strategy_tsconfig() {
        assert!(has_special_digest(Path::new("tsconfig.json")));
        assert!(has_special_digest(Path::new("tsconfig.build.json")));
    }

    #[test]
    fn special_strategy_markdown() {
        assert!(has_special_digest(Path::new("README.md")));
        assert!(has_special_digest(Path::new("docs.mdx")));
    }

    #[test]
    fn special_strategy_generated() {
        assert!(has_special_digest(Path::new("types.generated.ts")));
        assert!(has_special_digest(Path::new("schema.g.ts")));
    }

    #[test]
    fn digest_lock_file_output() {
        let content =
            "[[package]]\nname = \"foo\"\nversion = \"1.0\"\n[[package]]\nname = \"bar\"\n";
        let result = try_special_digest(Path::new("Cargo.lock"), content, FilterLevel::Minimal);
        assert!(result.is_some());
        let digest = result.unwrap();
        assert!(digest.contains("Lock file digest"));
    }

    #[test]
    fn digest_package_json_output() {
        let content = r#"{"name":"my-app","version":"1.0.0","scripts":{"dev":"next dev","build":"next build"},"dependencies":{"react":"18"}}"#;
        let result = try_special_digest(Path::new("package.json"), content, FilterLevel::Minimal);
        assert!(result.is_some());
        let digest = result.unwrap();
        assert!(digest.contains("package.json digest"));
        assert!(digest.contains("my-app"));
        assert!(digest.contains("scripts"));
    }

    #[test]
    fn digest_env_masks_values() {
        let content = "DB_HOST=localhost\nDB_PASS=secret123\n# comment\n";
        let result = try_special_digest(Path::new(".env"), content, FilterLevel::Minimal);
        assert!(result.is_some());
        let digest = result.unwrap();
        assert!(digest.contains("DB_HOST=***"));
        assert!(digest.contains("DB_PASS=***"));
        assert!(!digest.contains("secret123"), "values must be masked");
    }

    #[test]
    fn digest_markdown_shows_headers() {
        let content = "# Title\n\nSome text.\n\n## Section 1\n\nContent.\n\n## Section 2\n";
        let result = try_special_digest(Path::new("README.md"), content, FilterLevel::Minimal);
        assert!(result.is_some());
        let digest = result.unwrap();
        assert!(digest.contains("Markdown digest"));
        assert!(digest.contains("Title"));
        assert!(digest.contains("Section 1"));
    }

    // ── Long-line truncation tests ──────────────────────────

    #[test]
    fn truncate_long_lines_none_passthrough() {
        let long = "a".repeat(1000);
        assert_eq!(truncate_long_lines(&long, FilterLevel::None), long);
    }

    #[test]
    fn truncate_long_lines_aggressive() {
        let long = format!("{}\nshort\n", "x".repeat(300));
        let result = truncate_long_lines(&long, FilterLevel::Aggressive);
        assert!(result.contains('…'), "long line should be truncated");
        assert!(result.contains("short"), "short line preserved");
    }

    #[test]
    fn truncate_long_lines_minimal_higher_threshold() {
        let line = "y".repeat(400);
        let result = truncate_long_lines(&line, FilterLevel::Minimal);
        // 400 < 500 (minimal threshold), so no truncation
        assert!(!result.contains('…'));
    }

    #[test]
    fn truncate_long_lines_minimal_above_threshold() {
        let line = "z".repeat(600);
        let result = truncate_long_lines(&line, FilterLevel::Minimal);
        assert!(
            result.contains('…'),
            "line > 500 chars truncated in minimal"
        );
    }

    #[test]
    fn truncate_long_lines_unicode_safe() {
        let line = "あ".repeat(600);
        let result = truncate_long_lines(&line, FilterLevel::Minimal);
        assert!(result.contains('…'));
    }
}
