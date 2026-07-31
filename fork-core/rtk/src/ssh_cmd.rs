use crate::tracking;
use crate::utils::strip_ansi;
use anyhow::{Context, Result};
use std::process::Command;
use std::sync::LazyLock; // added: compile regex once per process

// Pre-compiled regexes (LazyLock: compiled on first use, reused thereafter)
static RE_TYPE_CAST: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"::[a-z_]+").unwrap());
static RE_FK: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"FOREIGN KEY \(([^)]+)\) REFERENCES ([a-zA-Z_][a-zA-Z0-9_.]*)\(([^)]+)\)"#)
        .unwrap()
});

/// Detected output format for auto-routing to the appropriate filter.
#[derive(Debug, PartialEq)]
enum DetectedFormat {
    PsqlTable,
    PsqlSchema,
    JsonLogs,
    Html,
    DockerPs,     // added: docker ps tabular output
    DockerImages, // added: docker images tabular output
    Generic,
}

// added: parsed SSH options for --tail and --format flags
struct SshOptions {
    tail: Option<usize>,    // --tail N: limit output to last N lines
    format: Option<String>, // --format psql|json|docker|docker-images|html|generic: force format
    ssh_args: Vec<String>,  // remaining args passed to ssh
}

/// Parse --tail and --format from args before passing rest to SSH.
fn parse_ssh_options(args: &[String]) -> SshOptions {
    let mut tail: Option<usize> = None;
    let mut format: Option<String> = None;
    let mut ssh_args: Vec<String> = Vec::new();
    let mut i = 0;

    while i < args.len() {
        if args[i] == "--tail" && i + 1 < args.len() {
            // added: --tail N extraction
            tail = args[i + 1].parse().ok();
            i += 2;
        } else if args[i].starts_with("--tail=") {
            tail = args[i].strip_prefix("--tail=").and_then(|v| v.parse().ok());
            i += 1;
        } else if args[i] == "--format" && i + 1 < args.len() {
            // added: --format X extraction
            format = Some(args[i + 1].clone());
            i += 2;
        } else if args[i].starts_with("--format=") {
            format = args[i].strip_prefix("--format=").map(|v| v.to_string());
            i += 1;
        } else {
            ssh_args.push(args[i].clone());
            i += 1;
        }
    }

    SshOptions {
        tail,
        format,
        ssh_args,
    }
}

/// SSH commands with smart output filtering.
/// Executes ssh with all args, captures output, detects format, applies filter.
pub fn run(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();
    let options = parse_ssh_options(args); // added: parse --tail/--format before SSH

    if verbose > 0 {
        eprintln!("ssh args: {:?}", options.ssh_args);
        if let Some(n) = options.tail {
            eprintln!("--tail: {}", n);
        }
        if let Some(ref f) = options.format {
            eprintln!("--format: {}", f);
        }
    }

    // 1. Execute: ssh <ssh_args only (--tail/--format stripped)>
    let output = Command::new("ssh")
        .args(&options.ssh_args) // changed: use parsed ssh_args
        .output()
        .context("Failed to execute ssh")?;

    let exit_code = output.status.code().unwrap_or(1);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let raw = format!("{}{}", stdout, stderr);

    // 2. Strip ANSI codes
    let clean_stdout = strip_ansi(&stdout);
    let clean_stderr = strip_ansi(&stderr);

    // 3. Strip SSH noise from stderr
    let filtered_stderr = strip_ssh_noise(&clean_stderr);

    // 4. Apply --tail: limit to last N lines if specified
    let combined_clean = if let Some(n) = options.tail {
        // added: --tail truncation before format detection
        let lines: Vec<&str> = clean_stdout.trim().lines().collect();
        let start = lines.len().saturating_sub(n);
        lines[start..].join("\n")
    } else {
        clean_stdout.trim().to_string()
    };

    // 5. Detect output format (or use --format override)
    let format = match options.format.as_deref() {
        // added: --format override for manual format selection
        Some("psql") => DetectedFormat::PsqlTable,
        Some("json") => DetectedFormat::JsonLogs,
        Some("docker") => DetectedFormat::DockerPs,
        Some("docker-images") => DetectedFormat::DockerImages,
        Some("html") => DetectedFormat::Html,
        Some("generic") => DetectedFormat::Generic,
        _ => detect_format(&combined_clean),
    };

    if verbose > 0 {
        eprintln!("Detected format: {:?}", format);
    }

    // 6. Apply appropriate filter
    let filtered_stdout = match format {
        DetectedFormat::PsqlTable => filter_psql_table(&combined_clean, verbose),
        DetectedFormat::PsqlSchema => filter_psql_schema(&combined_clean, verbose),
        DetectedFormat::JsonLogs => filter_json_logs(&combined_clean),
        DetectedFormat::Html => filter_html(&combined_clean, verbose),
        DetectedFormat::DockerPs => filter_docker_ps(&combined_clean, verbose), // added
        DetectedFormat::DockerImages => filter_docker_images(&combined_clean, verbose), // added
        DetectedFormat::Generic => filter_generic(&combined_clean, verbose),
    };

    // -v means "show full output" (after ANSI stripping).
    let final_stdout = if verbose > 0 {
        combined_clean
    } else {
        filtered_stdout
    };
    let final_stderr = if verbose > 0 {
        clean_stderr.trim().to_string()
    } else {
        filtered_stderr
    };

    // 7. Build final output
    let mut result = String::new();
    if !final_stderr.is_empty() {
        result.push_str(&final_stderr);
        if !final_stdout.is_empty() {
            result.push('\n');
        }
    }
    result.push_str(&final_stdout);

    // 8. Print filtered output
    print!("{}", result);

    // 9. Track savings
    let args_str = args.join(" ");
    timer.track(&format!("ssh {}", args_str), "rtk ssh", &raw, &result);

    // 10. Preserve exit code
    if exit_code != 0 {
        std::process::exit(exit_code);
    }

    Ok(())
}

// ── SSH noise stripping ──

/// Remove SSH connection noise lines from stderr.
fn strip_ssh_noise(stderr: &str) -> String {
    stderr
        .lines()
        .filter(|line| !is_ssh_noise(line))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// Check if a line is SSH connection noise.
fn is_ssh_noise(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return true; // strip blank lines
    }
    trimmed.starts_with("Pseudo-terminal will not be allocated")
        || trimmed.starts_with("Warning: Permanently added")
        || (trimmed.starts_with("Connection to") && trimmed.ends_with("closed."))
}

fn is_psql_row_count_footer(line: &str) -> bool {
    let t = line.trim();
    t.starts_with('(') && (t.ends_with("rows)") || t.ends_with("row)"))
}

// ── Format auto-detection ──

/// Detect the output format by analyzing content patterns.
fn detect_format(output: &str) -> DetectedFormat {
    if output.trim().is_empty() {
        return DetectedFormat::Generic;
    }

    // psql schema: "Table "public." or "Column | Type" pattern
    if is_psql_schema(output) {
        return DetectedFormat::PsqlSchema;
    }

    // psql tabular: lines contain ----+---- separator
    if is_psql_table(output) {
        return DetectedFormat::PsqlTable;
    }

    // JSON logs: >50% of non-empty lines start with `{`
    if is_json_logs(output) {
        return DetectedFormat::JsonLogs;
    }

    // HTML: contains <!DOCTYPE or <html
    if is_html(output) {
        return DetectedFormat::Html;
    }

    // added: docker ps tabular output
    if is_docker_ps(output) {
        return DetectedFormat::DockerPs;
    }

    // added: docker images tabular output
    if is_docker_images(output) {
        return DetectedFormat::DockerImages;
    }

    DetectedFormat::Generic
}

fn is_psql_schema(output: &str) -> bool {
    // Check for psql \d output patterns with stronger signal.
    let has_table_header = output.lines().any(|l| {
        let trimmed = l.trim();
        trimmed.starts_with("Table \"") || trimmed.starts_with("View \"")
    });
    let has_column_type = output.lines().any(|l| {
        let trimmed = l.trim();
        trimmed.contains("Column") && trimmed.contains("Type") && trimmed.contains("|")
    });
    let has_separator = output.lines().any(|l| l.trim().contains("-+-"));
    (has_table_header && has_separator) || (has_column_type && has_separator)
}

fn is_psql_table(output: &str) -> bool {
    // Require header + separator + at least one data line or row footer.
    let lines: Vec<&str> = output.lines().collect();
    let sep_idx = match lines.iter().position(|l| l.trim().contains("-+-")) {
        Some(i) => i,
        None => return false,
    };
    if sep_idx == 0 {
        return false;
    }
    let header = lines[sep_idx - 1];
    if !header.contains('|') {
        return false;
    }

    lines[sep_idx + 1..].iter().any(|l| {
        let t = l.trim();
        !t.is_empty() && (t.contains('|') || is_psql_row_count_footer(t))
    })
}

fn is_json_logs(output: &str) -> bool {
    let non_empty: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();
    if non_empty.is_empty() {
        return false;
    }
    let json_count = non_empty
        .iter()
        .filter(|l| l.trim().starts_with('{'))
        .count();
    // >50% of lines are JSON objects
    json_count * 2 > non_empty.len()
}

fn is_html(output: &str) -> bool {
    let lower = output.to_lowercase();
    lower.contains("<!doctype") || lower.contains("<html")
}

// added: docker ps header detection (CONTAINER ID + IMAGE + STATUS)
fn is_docker_ps(output: &str) -> bool {
    if let Some(first_line) = output.lines().next() {
        let upper = first_line.to_uppercase();
        upper.contains("CONTAINER ID") && upper.contains("IMAGE") && upper.contains("STATUS")
    } else {
        false
    }
}

// added: docker images header detection (REPOSITORY + TAG or SIZE)
fn is_docker_images(output: &str) -> bool {
    if let Some(first_line) = output.lines().next() {
        let upper = first_line.to_uppercase();
        upper.contains("REPOSITORY") && (upper.contains("TAG") || upper.contains("SIZE"))
    } else {
        false
    }
}

// ── psql Tabular Filter ──

/// Filter psql tabular output: hide wide columns, truncate values, limit rows.
fn filter_psql_table(output: &str, _verbose: u8) -> String {
    let lines: Vec<&str> = output.lines().collect();
    if lines.is_empty() {
        return output.to_string();
    }

    // Find separator line (---+---)
    let sep_idx = lines.iter().position(|l| l.contains("-+-"));
    if sep_idx.is_none() {
        return output.to_string();
    }
    let sep_idx = sep_idx.unwrap();

    // Header is the line before separator
    if sep_idx == 0 {
        return output.to_string();
    }
    let header_line = lines[sep_idx - 1];
    let data_lines: Vec<&str> = lines[sep_idx + 1..]
        .iter()
        .filter(|l| !l.trim().is_empty())
        .copied()
        // Filter out psql row count footer like "(10 rows)" or "(1 row)"
        .filter(|l| !is_psql_row_count_footer(l))
        .collect();

    // Parse columns from header
    let columns: Vec<&str> = header_line.split('|').map(|c| c.trim()).collect();
    let num_cols = columns.len();

    // Parse data rows
    let mut rows: Vec<Vec<String>> = Vec::new();
    for line in &data_lines {
        let vals: Vec<String> = line.split('|').map(|v| v.trim().to_string()).collect();
        rows.push(vals);
    }

    let num_rows = rows.len();

    // Calculate column width stats for smart hiding
    let col_stats: Vec<(f64, usize, bool)> = (0..num_cols)
        .map(|col_idx| {
            let widths: Vec<usize> = rows
                .iter()
                .map(|row| row.get(col_idx).map(|v| v.len()).unwrap_or(0))
                .collect();
            let avg = if widths.is_empty() {
                0.0
            } else {
                widths.iter().sum::<usize>() as f64 / widths.len() as f64
            };
            let max = widths.iter().copied().max().unwrap_or(0);
            let all_empty = widths.iter().all(|&w| w == 0); // added: detect all-empty columns
            (avg, max, all_empty)
        })
        .collect();

    // Auto-hide columns: avg > 30 OR max > 36 (catches sparse UUIDs) OR all values empty.
    // For "(0 rows)" keep all headers visible so schema context is preserved for LLM.
    let mut visible_cols: Vec<usize> = if num_rows == 0 {
        (0..num_cols).collect()
    } else {
        (0..num_cols)
            .filter(|&i| {
                let (avg, max, all_empty) = col_stats[i];
                !all_empty && avg <= 30.0 && max < 36 // changed: hide UUID-length (36+) and empty cols
            })
            .collect()
    };
    if visible_cols.is_empty() && num_cols > 0 {
        // Keep at least one column to avoid an empty table skeleton.
        let keep = (0..num_cols).min_by_key(|&i| col_stats[i].1).unwrap_or(0);
        visible_cols.push(keep);
    }
    let hidden_cols: Vec<&str> = (0..num_cols)
        .filter(|&i| !visible_cols.contains(&i))
        .map(|i| columns[i])
        .collect();

    let mut result = Vec::new();

    // Summary line
    result.push(format!("{} rows, {} cols", num_rows, num_cols));

    // Header with visible columns only (no artificial padding)
    let visible_header: Vec<&str> = visible_cols.iter().map(|&i| columns[i]).collect();
    result.push(format!(" {}", visible_header.join(" | ")));

    // Data rows (max 15)
    let show_rows = num_rows.min(15);
    for row in rows.iter().take(show_rows) {
        let visible_vals: Vec<String> = visible_cols
            .iter()
            .map(|&i| {
                let val = row.get(i).map(|v| v.as_str()).unwrap_or("");
                truncate_val(val, 25)
            })
            .collect();
        result.push(format!(" {}", visible_vals.join(" | ")));
    }

    if num_rows > 15 {
        result.push(format!("[+{} rows]", num_rows - 15));
    }

    // Hidden columns list
    if !hidden_cols.is_empty() {
        result.push(format!("Hidden cols: {}", hidden_cols.join(", ")));
    }

    result.join("\n")
}

/// Truncate a value to max_len chars.
fn truncate_val(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        s.to_string()
    } else {
        format!("{}...", s.chars().take(max_len - 3).collect::<String>())
    }
}

// ── psql Schema Filter ──

/// Filter psql \d schema output: compact column defs, summarize indexes/FKs/triggers.
fn filter_psql_schema(output: &str, _verbose: u8) -> String {
    let lines: Vec<&str> = output.lines().collect();
    if lines.is_empty() {
        return output.to_string();
    }

    let mut table_name = String::new();
    let mut columns: Vec<String> = Vec::new();
    let mut indexes: Vec<String> = Vec::new();
    let mut fk_constraints: Vec<String> = Vec::new();
    let mut triggers: Vec<String> = Vec::new();

    // Parse table name from header like: Table "public.utm_visits"
    for line in &lines {
        let trimmed = line.trim();
        if trimmed.contains("Table \"") || trimmed.contains("View \"") {
            if let Some(start) = trimmed.find('"') {
                if let Some(end) = trimmed.rfind('"') {
                    let full_name = &trimmed[start + 1..end];
                    // Strip "public." prefix
                    table_name = full_name
                        .strip_prefix("public.")
                        .unwrap_or(full_name)
                        .to_string();
                }
            }
            break;
        }
    }

    // Find separator and parse column definitions
    let sep_idx = lines.iter().position(|l| l.contains("-+-"));
    if let Some(sep) = sep_idx {
        // Parse column rows after separator
        let mut i = sep + 1;
        while i < lines.len() {
            let line = lines[i].trim();
            if line.is_empty()
                || line.starts_with("Indexes:")
                || line.starts_with("Foreign-key")
                || line.starts_with("Triggers:")
                || line.starts_with("Check constraints:")
                || line.starts_with("Referenced by:")
            {
                break;
            }
            // Parse: " name | type | collation | nullable | default"
            let parts: Vec<&str> = line.split('|').map(|p| p.trim()).collect();
            if parts.len() >= 4 {
                let col_name = parts[0];
                let col_type = compact_pg_type(parts[1]); // changed: shorten verbose type names
                let nullable = parts[3];
                let default = if parts.len() > 4 { parts[4].trim() } else { "" };

                let mut compact = format!(" {} {}", col_name, col_type);
                if nullable.contains("not null") {
                    compact.push_str(" NOT NULL");
                }
                if !default.is_empty() {
                    compact.push_str(&format!(" = {}", compact_default(default)));
                }
                columns.push(compact);
            }
            i += 1;
        }

        // Parse indexes, FK, triggers sections
        let mut section = "";
        for &line in &lines[sep + 1..] {
            let trimmed = line.trim();
            if trimmed.starts_with("Indexes:") {
                section = "indexes";
                continue;
            } else if trimmed.starts_with("Foreign-key constraints:") {
                section = "fk";
                continue;
            } else if trimmed.starts_with("Triggers:") {
                section = "triggers";
                continue;
            } else if trimmed.starts_with("Referenced by:")
                || trimmed.starts_with("Check constraints:")
            {
                section = "other";
                continue;
            } else if trimmed.is_empty() && !section.is_empty() {
                continue;
            }

            match section {
                "indexes" => {
                    if trimmed.starts_with('"') || trimmed.starts_with('\"') {
                        indexes.push(compact_index(trimmed));
                    }
                }
                "fk" => {
                    if trimmed.starts_with('"') || trimmed.starts_with('\"') {
                        fk_constraints.push(compact_fk(trimmed));
                    }
                }
                "triggers" => {
                    if !trimmed.is_empty() {
                        triggers.push(compact_trigger(trimmed));
                    }
                }
                _ => {}
            }
        }
    }

    // Build compact output
    let mut result = Vec::new();

    // Header: table_name (N cols, M idx, K FK, J trigger)
    let mut header_parts = vec![format!("{} cols", columns.len())];
    if !indexes.is_empty() {
        header_parts.push(format!("{} idx", indexes.len()));
    }
    if !fk_constraints.is_empty() {
        header_parts.push(format!("{} FK", fk_constraints.len()));
    }
    if !triggers.is_empty() {
        header_parts.push(format!(
            "{} trigger{}",
            triggers.len(),
            if triggers.len() > 1 { "s" } else { "" }
        ));
    }
    result.push(format!("{} ({})", table_name, header_parts.join(", ")));

    // Column definitions
    for col in &columns {
        result.push(col.clone());
    }

    // Indexes summary
    if !indexes.is_empty() {
        let show = indexes.len().min(4);
        let idx_summary: Vec<&str> = indexes.iter().take(show).map(|s| s.as_str()).collect();
        let mut idx_line = format!("Idx: {}", idx_summary.join(" "));
        if indexes.len() > 4 {
            idx_line.push_str(&format!(" +{}", indexes.len() - 4));
        }
        result.push(idx_line);
    }

    // FK summary
    if !fk_constraints.is_empty() {
        result.push(format!("FK: {}", fk_constraints.join(" ")));
    }

    // Triggers
    if !triggers.is_empty() {
        result.push(format!("Triggers: {}", triggers.join(", ")));
    }

    result.join("\n")
}

/// Compact a default value expression.
fn compact_default(default: &str) -> String {
    let d = default.trim();
    // Simplify common defaults
    if d.starts_with("nextval(") {
        return "serial".to_string();
    }
    if d.contains("gen_random_uuid()") {
        return "gen_random_uuid()".to_string();
    }
    if d == "now()" || d.contains("CURRENT_TIMESTAMP") || d == "timezone('utc'::text, now())" {
        return "now()".to_string();
    }
    // Strip type casts like ::text, ::boolean (LazyLock: compiled once)
    RE_TYPE_CAST.replace_all(d, "").trim().to_string()
}

/// Shorten verbose Postgres type names for compact schema output.
fn compact_pg_type(pg_type: &str) -> String {
    let t = pg_type.trim();
    // character varying(N) -> varchar(N)
    if let Some(rest) = t.strip_prefix("character varying") {
        return format!("varchar{}", rest);
    }
    // character(N) -> char(N)
    if let Some(rest) = t.strip_prefix("character(") {
        return format!("char({}", rest);
    }
    // timestamp with time zone -> timestamptz
    if t == "timestamp with time zone" {
        return "timestamptz".to_string();
    }
    // timestamp without time zone -> timestamp
    if t == "timestamp without time zone" {
        return "timestamp".to_string();
    }
    // double precision -> float8
    if t == "double precision" {
        return "float8".to_string();
    }
    // boolean -> bool
    if t == "boolean" {
        return "bool".to_string();
    }
    t.to_string()
}

/// Compact an index definition.
fn compact_index(line: &str) -> String {
    // Input: "idx_name" UNIQUE, btree (col1, col2)
    // Output: UNIQUE(col1,col2) or idx_name(col1,col2)
    let parts: Vec<&str> = line.splitn(2, ' ').collect();
    let name = parts[0].trim_matches('"');
    let rest = if parts.len() > 1 { parts[1] } else { "" };

    let is_pk = rest.contains("PRIMARY KEY");
    let is_unique = rest.contains("UNIQUE");

    // Extract column list from btree (col1, col2)
    let cols = if let Some(start) = rest.find('(') {
        if let Some(end) = rest.rfind(')') {
            rest[start + 1..end].replace(' ', "")
        } else {
            name.to_string()
        }
    } else {
        name.to_string()
    };

    if is_pk {
        format!("pkey({})", cols)
    } else if is_unique {
        format!("UNIQUE({})", cols)
    } else {
        format!("{}({})", name, cols)
    }
}

/// Compact a foreign key constraint.
fn compact_fk(line: &str) -> String {
    // Input: "fk_name" FOREIGN KEY (col) REFERENCES other_table(id)
    // Output: col->other_table(id)  (LazyLock: regex compiled once)
    if let Some(caps) = RE_FK.captures(line) {
        let col = caps.get(1).map(|m| m.as_str()).unwrap_or("?");
        let ref_table = caps.get(2).map(|m| m.as_str()).unwrap_or("?");
        let ref_col = caps.get(3).map(|m| m.as_str()).unwrap_or("?");
        return format!("{}->{}({})", col.trim(), ref_table, ref_col.trim());
    }
    // Fallback: return trimmed line
    line.trim_matches('"').trim().to_string()
}

/// Compact a trigger definition.
fn compact_trigger(line: &str) -> String {
    // Extract trigger name from line
    let trimmed = line.trim();
    if let Some(name_end) = trimmed.find(' ') {
        trimmed[..name_end].trim_matches('"').to_string()
    } else {
        trimmed.trim_matches('"').to_string()
    }
}

// ── JSON Logs Filter ──

/// JSON-aware log deduplication: parse msg/message fields, group by level, dedup.
fn filter_json_logs(output: &str) -> String {
    use std::collections::HashMap;

    let mut error_msgs: HashMap<String, usize> = HashMap::new();
    let mut warn_msgs: HashMap<String, usize> = HashMap::new();
    let mut info_count: usize = 0;
    let mut other_count: usize = 0;
    let mut first_errors: Vec<String> = Vec::new(); // preserve first occurrence
    let mut first_warns: Vec<String> = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || !trimmed.starts_with('{') {
            continue;
        }

        // Parse JSON line
        let parsed: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // Extract level and message
        let level = parsed
            .get("level")
            .or_else(|| parsed.get("severity"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_lowercase();
        let msg = parsed
            .get("msg")
            .or_else(|| parsed.get("message"))
            .and_then(|v| v.as_str())
            .unwrap_or("(no message)")
            .to_string();

        match level.as_str() {
            "error" | "fatal" | "panic" | "critical" => {
                let count = error_msgs.entry(msg.clone()).or_insert(0);
                if *count == 0 {
                    first_errors.push(msg);
                }
                *count += 1;
            }
            "warn" | "warning" => {
                let count = warn_msgs.entry(msg.clone()).or_insert(0);
                if *count == 0 {
                    first_warns.push(msg);
                }
                *count += 1;
            }
            "info" | "debug" | "trace" => {
                info_count += 1;
            }
            _ => {
                other_count += 1;
            }
        }
    }

    let total_errors: usize = error_msgs.values().sum();
    let total_warns: usize = warn_msgs.values().sum();

    let mut result = Vec::new();

    // Summary line
    let total = total_errors + total_warns + info_count + other_count;
    if other_count > 0 {
        result.push(format!(
            "{} log lines: {} err, {} warn, {} info, {} other",
            total, total_errors, total_warns, info_count, other_count
        ));
    } else {
        result.push(format!(
            "{} log lines: {} err, {} warn, {} info",
            total, total_errors, total_warns, info_count
        ));
    }

    // Errors (deduplicated)
    if !first_errors.is_empty() {
        result.push(String::new());
        for msg in &first_errors {
            let count = error_msgs.get(msg).copied().unwrap_or(1);
            if count > 1 {
                result.push(format!("  ERR [x{}] {}", count, msg));
            } else {
                result.push(format!("  ERR {}", msg));
            }
        }
    }

    // Warnings (deduplicated)
    if !first_warns.is_empty() {
        for msg in &first_warns {
            let count = warn_msgs.get(msg).copied().unwrap_or(1);
            if count > 1 {
                result.push(format!("  WARN [x{}] {}", count, msg));
            } else {
                result.push(format!("  WARN {}", msg));
            }
        }
    }

    result.join("\n")
}

// ── HTML Filter ──

/// Truncate HTML output to structure summary.
fn filter_html(output: &str, _verbose: u8) -> String {
    let line_count = output.lines().count();
    let char_count = output.len();

    let mut result = Vec::new();
    result.push(format!("HTML ({} lines, {} chars)", line_count, char_count));

    // Extract title if present
    let lower = output.to_lowercase();
    if let Some(start) = lower.find("<title>") {
        if let Some(end) = lower[start..].find("</title>") {
            let title = &output[start + 7..start + end];
            let title = title.trim();
            if !title.is_empty() {
                result.push(format!("title=\"{}\"", truncate_val(title, 60)));
            }
        }
    }

    // Detect common features
    let mut features = Vec::new();
    let json_ld_count = output.matches("ld+json").count();
    if json_ld_count > 0 {
        features.push(format!("{} ld+json schemas", json_ld_count));
    }
    if (lower.contains("google") && lower.contains("analytics")) || lower.contains("gtag") {
        // parens: GA = (google+analytics) OR gtag script present
        features.push("GA".to_string());
    }
    if lower.contains("metrika") || lower.contains("mc.yandex") {
        // simplified: "yandex && mc.yandex" was redundant (mc.yandex always contains yandex)
        features.push("Yandex.Metrika".to_string());
    }
    if !features.is_empty() {
        result.push(features.join(", "));
    }

    result.push("[use -v for full output]".to_string());
    result.join("\n")
}

// ── Docker PS Filter ──

/// Filter docker ps tabular output: show Name, Image, Status, Ports (compact). Limit 15 rows.
fn filter_docker_ps(output: &str, _verbose: u8) -> String {
    let lines: Vec<&str> = output.lines().collect();
    if lines.len() < 2 {
        return output.to_string();
    }

    // Parse header to find column positions
    let header = lines[0];
    let col_starts = parse_docker_columns(header);

    let data_lines: Vec<&str> = lines[1..]
        .iter()
        .filter(|l| !l.trim().is_empty())
        .copied()
        .collect();
    let num_rows = data_lines.len();

    let mut result = Vec::new();
    result.push(format!("{} containers", num_rows)); // added: summary line

    // Extract columns: NAME, IMAGE, STATUS, PORTS
    let name_idx = col_starts.iter().position(|(name, _)| *name == "NAMES");
    let image_idx = col_starts.iter().position(|(name, _)| *name == "IMAGE");
    let status_idx = col_starts.iter().position(|(name, _)| *name == "STATUS");
    let ports_idx = col_starts.iter().position(|(name, _)| *name == "PORTS");

    let show_rows = num_rows.min(15); // added: limit to 15 rows
    for line in data_lines.iter().take(show_rows) {
        let fields = extract_docker_fields(line, &col_starts);
        let name = name_idx
            .map(|i| fields.get(i).copied().unwrap_or(""))
            .unwrap_or("");
        let image = image_idx
            .map(|i| fields.get(i).copied().unwrap_or(""))
            .unwrap_or("");
        let status = status_idx
            .map(|i| fields.get(i).copied().unwrap_or(""))
            .unwrap_or("");
        let ports = ports_idx
            .map(|i| fields.get(i).copied().unwrap_or(""))
            .unwrap_or("");

        let compact_port = compact_ports(ports); // added: compact port display
        result.push(format!(
            " {} | {} | {} | {}",
            truncate_val(name, 30),
            truncate_val(image, 35),
            truncate_val(status, 20),
            compact_port
        ));
    }

    if num_rows > 15 {
        result.push(format!("[+{} containers]", num_rows - 15)); // added: overflow indicator
    }

    result.join("\n")
}

// ── Docker Images Filter ──

/// Filter docker images tabular output: show Repository, Tag, Size. Aggregate total size. Limit 15 rows.
fn filter_docker_images(output: &str, _verbose: u8) -> String {
    let lines: Vec<&str> = output.lines().collect();
    if lines.len() < 2 {
        return output.to_string();
    }

    let header = lines[0];
    let col_starts = parse_docker_columns(header);

    let data_lines: Vec<&str> = lines[1..]
        .iter()
        .filter(|l| !l.trim().is_empty())
        .copied()
        .collect();
    let num_rows = data_lines.len();

    let repo_idx = col_starts
        .iter()
        .position(|(name, _)| *name == "REPOSITORY");
    let tag_idx = col_starts.iter().position(|(name, _)| *name == "TAG");
    let size_idx = col_starts.iter().position(|(name, _)| *name == "SIZE");

    let mut result = Vec::new();

    // Collect sizes for aggregation
    let mut total_mb: f64 = 0.0; // added: total size aggregation
    let show_rows = num_rows.min(15);
    for line in data_lines.iter().take(show_rows) {
        let fields = extract_docker_fields(line, &col_starts);
        let repo = repo_idx
            .map(|i| fields.get(i).copied().unwrap_or(""))
            .unwrap_or("");
        let tag = tag_idx
            .map(|i| fields.get(i).copied().unwrap_or(""))
            .unwrap_or("");
        let size = size_idx
            .map(|i| fields.get(i).copied().unwrap_or(""))
            .unwrap_or("");

        total_mb += parse_size_mb(size);
        result.push(format!(
            " {} | {} | {}",
            truncate_val(repo, 40),
            truncate_val(tag, 15),
            size
        ));
    }

    if num_rows > 15 {
        // Parse remaining sizes for total
        for line in data_lines.iter().skip(15) {
            let fields = extract_docker_fields(line, &col_starts);
            let size = size_idx
                .map(|i| fields.get(i).copied().unwrap_or(""))
                .unwrap_or("");
            total_mb += parse_size_mb(size);
        }
        result.push(format!("[+{} images]", num_rows - 15));
    }

    // Summary header with total size
    let summary = if total_mb >= 1024.0 {
        format!("{} images, {:.1}GB total", num_rows, total_mb / 1024.0) // added: GB display
    } else {
        format!("{} images, {:.0}MB total", num_rows, total_mb)
    };
    result.insert(0, summary);

    result.join("\n")
}

/// Parse docker-style fixed-width column headers. Returns (name, start_pos) pairs.
fn parse_docker_columns(header: &str) -> Vec<(&str, usize)> {
    let mut cols = Vec::new();
    let mut i = 0;
    let bytes = header.as_bytes();
    let len = bytes.len();

    while i < len {
        // Skip whitespace
        while i < len && bytes[i] == b' ' {
            i += 1;
        }
        if i >= len {
            break;
        }
        let start = i;
        // Find end of column name (next run of 2+ spaces or end)
        while i < len && bytes[i] != b' ' {
            i += 1;
        }
        // Check for multi-word column names like "CONTAINER ID"
        while i < len {
            // Peek: if exactly one space followed by uppercase letter, continue
            if i + 1 < len
                && bytes[i] == b' '
                && bytes[i + 1] != b' '
                && bytes[i + 1].is_ascii_uppercase()
            {
                i += 1; // skip the single space
                while i < len && bytes[i] != b' ' {
                    i += 1;
                }
            } else {
                break;
            }
        }
        let name = header[start..i].trim();
        if !name.is_empty() {
            cols.push((name, start));
        }
    }
    cols
}

/// Extract field values from a docker output line using column positions.
fn extract_docker_fields<'a>(line: &'a str, cols: &[(&str, usize)]) -> Vec<&'a str> {
    let mut fields = Vec::new();
    for (idx, &(_, start)) in cols.iter().enumerate() {
        let end = if idx + 1 < cols.len() {
            cols[idx + 1].1
        } else {
            line.len()
        };
        let s = start.min(line.len());
        let e = end.min(line.len());
        fields.push(line[s..e].trim());
    }
    fields
}

/// Compact port display: extract port numbers, limit to 3. (Logic from container.rs)
fn compact_ports(ports: &str) -> String {
    if ports.is_empty() || ports == "-" {
        return "-".to_string();
    }
    let port_nums: Vec<&str> = ports
        .split(',')
        .filter_map(|p| p.split("->").next().and_then(|s| s.split(':').next_back()))
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    if port_nums.is_empty() {
        return "-".to_string();
    }
    if port_nums.len() <= 3 {
        port_nums.join(", ")
    } else {
        format!(
            "{}, ... +{}",
            port_nums[..2].join(", "),
            port_nums.len() - 2
        )
    }
}

/// Parse a docker size string (e.g. "150MB", "1.2GB") into megabytes.
fn parse_size_mb(s: &str) -> f64 {
    let s = s.trim();
    if s.is_empty() {
        return 0.0;
    }
    let lower = s.to_lowercase();
    if let Some(num) = lower.strip_suffix("gb") {
        num.trim().parse::<f64>().unwrap_or(0.0) * 1024.0
    } else if let Some(num) = lower.strip_suffix("mb") {
        num.trim().parse::<f64>().unwrap_or(0.0)
    } else if let Some(num) = lower.strip_suffix("kb") {
        num.trim().parse::<f64>().unwrap_or(0.0) / 1024.0
    } else if let Some(num) = lower.strip_suffix('b') {
        num.trim().parse::<f64>().unwrap_or(0.0) / (1024.0 * 1024.0)
    } else {
        0.0
    }
}

// ── Generic Fallback ──

/// Truncate generic output: limit rows to 20, truncate wide lines to 120 chars.
fn filter_generic(output: &str, _verbose: u8) -> String {
    let lines: Vec<&str> = output.lines().collect();
    let total_lines = lines.len();
    let total_chars = output.len();

    let max_lines = 20;
    let max_line_width = 120; // added: truncate wide lines for token savings

    let show_lines = total_lines.min(max_lines);
    let mut result: Vec<String> = lines[..show_lines]
        .iter()
        .map(|l| truncate_val(l, max_line_width))
        .collect();

    if total_lines > max_lines {
        result.push(format!(
            "[... {} more lines, {} chars total]",
            total_lines - max_lines,
            total_chars
        ));
    }

    result.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── SSH noise stripping tests ──

    #[test]
    fn test_strip_ssh_noise_pseudo_terminal() {
        let stderr = "Pseudo-terminal will not be allocated because stdin is not a terminal.\nActual error message";
        let result = strip_ssh_noise(stderr);
        assert_eq!(result, "Actual error message");
    }

    #[test]
    fn test_strip_ssh_noise_known_hosts() {
        let stderr = "Warning: Permanently added '192.168.1.1' (ED25519) to the list of known hosts.\nReal warning";
        let result = strip_ssh_noise(stderr);
        assert_eq!(result, "Real warning");
    }

    #[test]
    fn test_strip_ssh_noise_connection_closed() {
        let stderr = "Some output\nConnection to example.com closed.";
        let result = strip_ssh_noise(stderr);
        assert_eq!(result, "Some output");
    }

    #[test]
    fn test_strip_ssh_noise_all_noise() {
        let stderr = "Pseudo-terminal will not be allocated because stdin is not a terminal.\nWarning: Permanently added 'host' (RSA) to the list of known hosts.\nConnection to host closed.\n";
        let result = strip_ssh_noise(stderr);
        assert_eq!(result, "");
    }

    #[test]
    fn test_strip_ssh_noise_preserves_real_errors() {
        let stderr = "bash: command not found: foo";
        let result = strip_ssh_noise(stderr);
        assert_eq!(result, "bash: command not found: foo");
    }

    // ── Format detection tests ──

    #[test]
    fn test_detect_psql_table() {
        let output = " id | name | email\n----+------+--------\n  1 | John | j@x.com\n  2 | Jane | jane@x.com";
        assert_eq!(detect_format(output), DetectedFormat::PsqlTable);
    }

    #[test]
    fn test_detect_psql_schema() {
        let output = r#"                               Table "public.utm_visits"
      Column      |           Type           | Collation | Nullable | Default
------------------+--------------------------+-----------+----------+---
 id               | uuid                     |           | not null | gen_random_uuid()"#;
        assert_eq!(detect_format(output), DetectedFormat::PsqlSchema);
    }

    #[test]
    fn test_detect_json_logs() {
        let output = r#"{"level":"info","msg":"started","ts":"2026-01-01"}
{"level":"error","msg":"failed","ts":"2026-01-01"}
{"level":"info","msg":"retrying","ts":"2026-01-01"}"#;
        assert_eq!(detect_format(output), DetectedFormat::JsonLogs);
    }

    #[test]
    fn test_detect_html() {
        let output = "<!DOCTYPE html>\n<html lang=\"en\">\n<head><title>Test</title></head>\n<body>Hello</body>\n</html>";
        assert_eq!(detect_format(output), DetectedFormat::Html);
    }

    #[test]
    fn test_detect_html_lowercase() {
        let output = "<html><head><title>Test</title></head><body>Hello</body></html>";
        assert_eq!(detect_format(output), DetectedFormat::Html);
    }

    #[test]
    fn test_detect_generic() {
        let output = "just some plain text\nnothing special here";
        assert_eq!(detect_format(output), DetectedFormat::Generic);
    }

    #[test]
    fn test_detect_empty() {
        assert_eq!(detect_format(""), DetectedFormat::Generic);
        assert_eq!(detect_format("   "), DetectedFormat::Generic);
    }

    #[test]
    fn test_detect_json_not_enough_json_lines() {
        // Only 1 out of 3 lines is JSON — should NOT detect as JSON
        let output = "plain text\n{\"key\":\"value\"}\nmore plain text";
        assert_ne!(detect_format(output), DetectedFormat::JsonLogs);
    }

    // Schema takes priority over table when both patterns present
    #[test]
    fn test_detect_schema_priority_over_table() {
        let output = r#"                               Table "public.users"
      Column      |           Type           | Collation | Nullable | Default
------------------+--------------------------+-----------+----------+---
 id               | uuid                     |           | not null | gen_random_uuid()"#;
        assert_eq!(detect_format(output), DetectedFormat::PsqlSchema);
    }

    #[test]
    fn test_detect_schema_not_triggered_by_header_only() {
        let output = r#"Table "notes"
this is not psql schema output"#;
        assert_eq!(detect_format(output), DetectedFormat::Generic);
    }

    #[test]
    fn test_detect_table_not_triggered_by_separator_only() {
        let output = r#"random text
----+----
still random"#;
        assert_eq!(detect_format(output), DetectedFormat::Generic);
    }

    // ── psql Table Filter tests ──

    #[test]
    fn test_filter_psql_table_basic() {
        let input = " name | age\n------+-----\n John | 30\n Jane | 25\n(2 rows)";
        let result = filter_psql_table(input, 0);
        assert!(result.contains("2 rows, 2 cols"));
        assert!(result.contains("John"));
        assert!(result.contains("Jane"));
    }

    #[test]
    fn test_filter_psql_table_hides_wide_columns() {
        // Simulate a column with very long UUID values
        let uuid = "ae0ee4f3-1234-5678-9abc-def012345678";
        let input = format!(
            " id | name\n----+------\n {} | John\n {} | Jane",
            uuid, uuid
        );
        let result = filter_psql_table(&input, 0);
        // id column has avg width > 30, should be hidden
        assert!(result.contains("Hidden cols: id"));
        assert!(result.contains("name"));
    }

    #[test]
    fn test_filter_psql_table_limits_rows() {
        let mut lines = vec![" id | val".to_string(), "----+----".to_string()];
        for i in 0..20 {
            lines.push(format!("  {} | x", i));
        }
        let input = lines.join("\n");
        let result = filter_psql_table(&input, 0);
        assert!(result.contains("20 rows, 2 cols"));
        assert!(result.contains("[+5 rows]"));
    }

    #[test]
    fn test_filter_psql_table_row_count_footer_stripped() {
        let input = " a | b\n---+---\n 1 | 2\n(1 row)";
        let result = filter_psql_table(input, 0);
        assert!(!result.contains("(1 row)"));
        assert!(result.contains("1 rows"));
    }

    #[test]
    fn test_filter_psql_table_zero_rows_keeps_header() {
        let input = " id | name | email\n----+------+-------\n(0 rows)";
        let result = filter_psql_table(input, 0);
        assert!(result.contains("0 rows, 3 cols"), "got: {}", result);
        assert!(result.contains("id | name | email"), "got: {}", result);
        assert!(!result.contains("Hidden cols:"), "got: {}", result);
    }

    // ── psql Schema Filter tests ──

    #[test]
    fn test_filter_psql_schema_basic() {
        let input = r#"                               Table "public.users"
      Column      |           Type           | Collation | Nullable |         Default
------------------+--------------------------+-----------+----------+---
 id               | uuid                     |           | not null | gen_random_uuid()
 name             | varchar(100)             |           | not null |
 email            | text                     |           |          |
Indexes:
    "users_pkey" PRIMARY KEY, btree (id)
    "users_email_key" UNIQUE, btree (email)
Foreign-key constraints:
    "users_org_fk" FOREIGN KEY (org_id) REFERENCES organizations(id)
Triggers:
    trg_audit AFTER INSERT ON users FOR EACH ROW EXECUTE FUNCTION audit_log()"#;

        let result = filter_psql_schema(input, 0);
        assert!(result.contains("users (3 cols, 2 idx, 1 FK, 1 trigger)"));
        assert!(result.contains("id uuid NOT NULL = gen_random_uuid()"));
        assert!(result.contains("name varchar(100) NOT NULL"));
        assert!(result.contains("email text"));
        assert!(result.contains("pkey(id)"));
        assert!(result.contains("UNIQUE(email)"));
        assert!(result.contains("org_id->organizations(id)"));
        assert!(result.contains("trg_audit"));
    }

    #[test]
    fn test_filter_psql_schema_no_indexes() {
        let input = r#"                               Table "public.simple"
      Column      |           Type           | Collation | Nullable | Default
------------------+--------------------------+-----------+----------+---
 id               | integer                  |           | not null |
 value            | text                     |           |          |"#;
        let result = filter_psql_schema(input, 0);
        assert!(result.contains("simple (2 cols)"));
        assert!(!result.contains("idx"));
    }

    #[test]
    fn test_compact_default_serial() {
        assert_eq!(compact_default("nextval('seq'::regclass)"), "serial");
    }

    #[test]
    fn test_compact_default_now() {
        assert_eq!(compact_default("now()"), "now()");
        assert_eq!(compact_default("timezone('utc'::text, now())"), "now()");
    }

    #[test]
    fn test_compact_default_uuid() {
        assert_eq!(compact_default("gen_random_uuid()"), "gen_random_uuid()");
    }

    // ── JSON logs filter test ──

    #[test]
    fn test_filter_json_logs_deduplicates() {
        let input = r#"{"level":"error","msg":"Connection failed","ts":"2026-01-01"}
{"level":"error","msg":"Connection failed","ts":"2026-01-02"}
{"level":"error","msg":"Connection failed","ts":"2026-01-03"}
{"level":"info","msg":"Started","ts":"2026-01-01"}"#;
        let result = filter_json_logs(input);
        // JSON-aware dedup: 3 identical errors collapsed to [x3]
        assert!(
            result.contains("ERR [x3] Connection failed"),
            "got: {}",
            result
        );
        assert!(result.contains("3 err"), "got: {}", result);
        assert!(result.contains("1 info"), "got: {}", result);
    }

    #[test]
    fn test_filter_json_logs_shows_other_count() {
        let input = r#"{"level":"error","msg":"x"}
{"level":"notice","msg":"y"}
{"msg":"z"}"#;
        let result = filter_json_logs(input);
        assert!(result.contains("2 other"), "got: {}", result);
    }

    // ── HTML filter tests ──

    #[test]
    fn test_filter_html_basic() {
        let input = "<!DOCTYPE html>\n<html lang=\"en\">\n<head><title>My Page</title></head>\n<body>content</body>\n</html>";
        let result = filter_html(input, 0);
        assert!(result.contains("HTML (5 lines"));
        assert!(result.contains("title=\"My Page\""));
        assert!(result.contains("[use -v for full output]"));
    }

    #[test]
    fn test_filter_html_with_analytics() {
        let input = "<html><head><title>Test</title><script>gtag('config', 'G-XXX')</script></head><body></body></html>";
        let result = filter_html(input, 0);
        assert!(result.contains("GA"));
    }

    // ── Generic fallback tests ──

    #[test]
    fn test_filter_generic_short_output() {
        let input = "line1\nline2\nline3";
        let result = filter_generic(input, 0);
        assert_eq!(result, input); // No truncation for short output
    }

    #[test]
    fn test_filter_generic_long_output() {
        let lines: Vec<String> = (0..30).map(|i| format!("line {}", i)).collect();
        let input = lines.join("\n");
        let result = filter_generic(&input, 0);
        assert!(result.contains("line 0"));
        assert!(result.contains("line 19"));
        assert!(result.contains("[... 10 more lines"));
        assert!(!result.contains("line 20")); // Not directly visible
    }

    // ── truncate_val tests ──

    #[test]
    fn test_truncate_val_short() {
        assert_eq!(truncate_val("hello", 10), "hello");
    }

    #[test]
    fn test_truncate_val_long() {
        assert_eq!(truncate_val("hello world foo bar", 10), "hello w...");
    }

    // ── compact_index tests ──

    #[test]
    fn test_compact_index_primary_key() {
        let input = "\"users_pkey\" PRIMARY KEY, btree (id)";
        assert_eq!(compact_index(input), "pkey(id)");
    }

    #[test]
    fn test_compact_index_unique() {
        let input = "\"users_email_key\" UNIQUE, btree (email)";
        assert_eq!(compact_index(input), "UNIQUE(email)");
    }

    #[test]
    fn test_compact_index_regular() {
        let input = "\"idx_utm\" btree (source, medium, campaign)";
        assert_eq!(compact_index(input), "idx_utm(source,medium,campaign)");
    }

    // ── compact_fk tests ──

    #[test]
    fn test_compact_fk() {
        let input = "\"fk_campaign\" FOREIGN KEY (campaign_id) REFERENCES marketing_campaigns(id)";
        assert_eq!(compact_fk(input), "campaign_id->marketing_campaigns(id)");
    }

    #[test]
    fn test_compact_trigger() {
        let input =
            "trg_link_visit AFTER INSERT ON utm_visits FOR EACH ROW EXECUTE FUNCTION link_visit()";
        assert_eq!(compact_trigger(input), "trg_link_visit");
    }

    // ── Docker PS detection tests ──

    #[test]
    fn test_detect_docker_ps() {
        // added: docker ps header detection
        let output = "CONTAINER ID   IMAGE                    COMMAND       CREATED       STATUS         PORTS                    NAMES\nabc123def456   nginx:latest             \"nginx -g…\"   2 hours ago   Up 2 hours     0.0.0.0:80->80/tcp       web-1";
        assert_eq!(detect_format(output), DetectedFormat::DockerPs);
    }

    #[test]
    fn test_detect_docker_images() {
        // added: docker images header detection
        let output = "REPOSITORY          TAG       IMAGE ID       CREATED        SIZE\nnginx               latest    abc123def456   2 weeks ago    187MB\npostgres            15        bcd234efg567   3 weeks ago    412MB";
        assert_eq!(detect_format(output), DetectedFormat::DockerImages);
    }

    // ── Docker PS filter tests ──

    #[test]
    fn test_filter_docker_ps_basic() {
        // added: basic docker ps filter test
        let output = "CONTAINER ID   IMAGE            COMMAND        CREATED       STATUS         PORTS                    NAMES\nabc123def456   nginx:latest     \"nginx -g…\"    2 hours ago   Up 2 hours     0.0.0.0:80->80/tcp       web-1\nbcd234efg567   postgres:15      \"postgres\"     3 hours ago   Up 3 hours     0.0.0.0:5432->5432/tcp   db-1";
        let result = filter_docker_ps(output, 0);
        assert!(result.contains("2 containers"), "got: {}", result);
        assert!(result.contains("web-1"), "got: {}", result);
        assert!(result.contains("db-1"), "got: {}", result);
        assert!(result.contains("nginx"), "got: {}", result);
    }

    #[test]
    fn test_filter_docker_ps_limits_rows() {
        // added: 15 row limit test
        let mut lines = vec![
            "CONTAINER ID   IMAGE            COMMAND   CREATED       STATUS       PORTS   NAMES"
                .to_string(),
        ];
        for i in 0..20 {
            lines.push(format!(
                "abc{:03}def456   nginx:latest     \"nginx\"   1h ago        Up 1h        80/tcp  container-{}",
                i, i
            ));
        }
        let output = lines.join("\n");
        let result = filter_docker_ps(&output, 0);
        assert!(result.contains("20 containers"), "got: {}", result);
        assert!(result.contains("[+5 containers]"), "got: {}", result);
    }

    // ── Docker Images filter tests ──

    #[test]
    fn test_filter_docker_images_basic() {
        // added: basic docker images filter with size aggregation
        let output = "REPOSITORY          TAG       IMAGE ID       CREATED        SIZE\nnginx               latest    abc123def456   2 weeks ago    187MB\npostgres            15        bcd234efg567   3 weeks ago    412MB";
        let result = filter_docker_images(output, 0);
        assert!(result.contains("2 images"), "got: {}", result);
        assert!(result.contains("599MB"), "got: {}", result);
        assert!(result.contains("nginx"), "got: {}", result);
        assert!(result.contains("postgres"), "got: {}", result);
    }

    // ── Parse SSH options tests ──

    #[test]
    fn test_parse_ssh_options_tail() {
        // added: --tail extraction
        let args: Vec<String> = vec!["--tail", "50", "host", "uptime"]
            .into_iter()
            .map(String::from)
            .collect();
        let opts = parse_ssh_options(&args);
        assert_eq!(opts.tail, Some(50));
        assert_eq!(opts.ssh_args, vec!["host", "uptime"]);
    }

    #[test]
    fn test_parse_ssh_options_format() {
        // added: --format extraction
        let args: Vec<String> = vec!["--format=psql", "host", "psql -c 'SELECT 1'"]
            .into_iter()
            .map(String::from)
            .collect();
        let opts = parse_ssh_options(&args);
        assert_eq!(opts.format, Some("psql".to_string()));
        assert_eq!(opts.ssh_args, vec!["host", "psql -c 'SELECT 1'"]);
    }

    #[test]
    fn test_parse_ssh_options_passthrough() {
        // added: flags not consumed should pass through to SSH
        let args: Vec<String> = vec!["-o", "StrictHostKeyChecking=no", "user@host", "ls -la"]
            .into_iter()
            .map(String::from)
            .collect();
        let opts = parse_ssh_options(&args);
        assert_eq!(opts.tail, None);
        assert_eq!(opts.format, None);
        assert_eq!(
            opts.ssh_args,
            vec!["-o", "StrictHostKeyChecking=no", "user@host", "ls -la"]
        );
    }

    // ── compact_ports tests ──

    #[test]
    fn test_compact_ports_empty() {
        assert_eq!(compact_ports(""), "-");
    }

    #[test]
    fn test_compact_ports_simple() {
        assert_eq!(compact_ports("0.0.0.0:80->80/tcp"), "80");
    }

    #[test]
    fn test_compact_ports_multiple() {
        assert_eq!(
            compact_ports("0.0.0.0:80->80/tcp, 0.0.0.0:443->443/tcp"),
            "80, 443"
        );
    }

    // ── parse_size_mb tests ──

    #[test]
    fn test_parse_size_mb() {
        assert!((parse_size_mb("187MB") - 187.0).abs() < 0.01);
        assert!((parse_size_mb("1.2GB") - 1228.8).abs() < 0.1);
        assert!((parse_size_mb("512KB") - 0.5).abs() < 0.01);
    }

    // ── Visual review test (prints all filter outputs for human/LLM inspection) ──

    #[test]
    #[ignore] // dev-only: run explicitly with `cargo test -- --ignored`
    fn visual_review_all_filters() {
        println!("\n{}", "=".repeat(60));
        println!("=== VISUAL REVIEW: SSH Filter Outputs ===");
        println!("{}\n", "=".repeat(60));

        // --- 1. Realistic psql SELECT (utm_visits with wide columns) ---
        let psql_select = r#" id                                   | utm_source | utm_medium | utm_campaign | utm_content | utm_term  | session_id                                                       | user_id                              | landing_page                                                                                | referrer                                              | user_agent                                                                                                                   | ip_address     | visited_at                 | registered_at | converted_at | campaign_id                          | created_at                 | was_pro_at_visit
--------------------------------------+------------+------------+--------------+-------------+-----------+------------------------------------------------------------------+--------------------------------------+---------------------------------------------------------------------------------------------+-------------------------------------------------------+------------------------------------------------------------------------------------------------------------------------------+----------------+----------------------------+---------------+--------------+--------------------------------------+----------------------------+------------------
 ae0ee4f3-1b4b-4a3d-8e4f-1234567890ab | telegram   | influencer | telegain     |             |           | 449df4b2e8a1c0d3f5e6b7a8c9d0e1f2a3b4c5d6e7f8a9b0c1d2e3f4a5b6c7d8 | b7c8d9e0-f1a2-b3c4-d5e6-f7a8b9c0d1e2 | https://minicoder.ru/?utm_source=telegram&utm_medium=influencer&utm_campaign=telegain        | https://t.me/coding_channel                           | Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36       | 185.220.101.42 | 2026-01-30 17:15:11.123456 |               |              | c3d4e5f6-a7b8-c9d0-e1f2-a3b4c5d6e7f8 | 2026-01-30 17:15:11.123456 | f
 bf1ff5g4-2c5c-5b4e-9f5g-2345678901bc | telegram   | influencer | telegain     |             |           | 550eg5c3f9b2d1e4g6f7c8b9d0e2g3h4i5j6k7l8m9n0o1p2q3r4s5t6u7v8w9x0 | c8d9e0f1-a2b3-c4d5-e6f7-a8b9c0d1e2f3 | https://minicoder.ru/?utm_source=telegram&utm_medium=influencer&utm_campaign=telegain        |                                                       | Mozilla/5.0 (iPhone; CPU iPhone OS 17_2 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.2 Mobile Safari  | 91.108.6.91    | 2026-01-30 17:11:05.654321 |               |              |                                      | 2026-01-30 17:11:05.654321 | f
 cg2gg6h5-3d6d-6c5f-0g6h-3456789012cd | google     | cpc        | brand_ru     | main        | миникодер | 661fh6d4g0c3e2f5h7g8d9c0e1f3g4h5i6j7k8l9m0n1o2p3q4r5s6t7u8v9w0x1 |                                      | https://minicoder.ru/?utm_source=google&utm_medium=cpc&utm_campaign=brand_ru&gclid=abc123   | https://www.google.com/                               | Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36             | 46.39.228.18   | 2026-01-30 16:45:22.789012 |               |              | d4e5f6a7-b8c9-d0e1-f2a3-b4c5d6e7f8a9 | 2026-01-30 16:45:22.789012 | f
(3 rows)"#;
        let raw_chars = psql_select.len();
        let result = filter_psql_table(psql_select, 0);
        println!("--- 1. psql SELECT (utm_visits, 3 rows x 18 cols) ---");
        println!("RAW: {} chars", raw_chars);
        println!(
            "FILTERED ({} chars, {:.0}% saved):",
            result.len(),
            (1.0 - result.len() as f64 / raw_chars as f64) * 100.0
        );
        println!("{}", result);

        // --- 2. Realistic psql \d schema ---
        let psql_schema = r#"                                          Table "public.utm_visits"
      Column      |           Type           | Collation | Nullable |              Default
------------------+--------------------------+-----------+----------+-----------------------------------
 id               | uuid                     |           | not null | gen_random_uuid()
 utm_source       | character varying(50)    |           |          |
 utm_medium       | character varying(50)    |           |          |
 utm_campaign     | character varying(100)   |           |          |
 utm_content      | character varying(100)   |           |          |
 utm_term         | character varying(100)   |           |          |
 session_id       | character varying(64)    |           | not null |
 user_id          | uuid                     |           |          |
 landing_page     | text                     |           |          |
 referrer         | text                     |           |          |
 user_agent       | text                     |           |          |
 ip_address       | inet                     |           |          |
 visited_at       | timestamp with time zone |           |          | now()
 registered_at    | timestamp with time zone |           |          |
 converted_at     | timestamp with time zone |           |          |
 campaign_id      | uuid                     |           |          |
 created_at       | timestamp with time zone |           |          | now()
 was_pro_at_visit | boolean                  |           |          | false
Indexes:
    "utm_visits_pkey" PRIMARY KEY, btree (id)
    "idx_utm_visits_session" UNIQUE, btree (session_id)
    "idx_utm_visits_utm" btree (utm_source, utm_medium, utm_campaign)
    "idx_utm_visits_visited_at" btree (visited_at)
    "idx_utm_visits_user" btree (user_id)
    "idx_utm_visits_campaign" btree (campaign_id)
Foreign-key constraints:
    "utm_visits_campaign_id_fkey" FOREIGN KEY (campaign_id) REFERENCES marketing_campaigns(id)
    "utm_visits_user_id_fkey" FOREIGN KEY (user_id) REFERENCES users(id)
Triggers:
    trg_link_visit_to_campaign AFTER INSERT ON utm_visits FOR EACH ROW EXECUTE FUNCTION link_visit_to_campaign()"#;
        let raw_chars = psql_schema.len();
        let result = filter_psql_schema(psql_schema, 0);
        println!("\n--- 2. psql \\d (utm_visits schema) ---");
        println!("RAW: {} chars", raw_chars);
        println!(
            "FILTERED ({} chars, {:.0}% saved):",
            result.len(),
            (1.0 - result.len() as f64 / raw_chars as f64) * 100.0
        );
        println!("{}", result);

        // --- 3. Docker JSON logs ---
        let docker_logs = r#"{"level":"info","ts":"2026-01-30T10:00:01.123Z","msg":"Server started on :3000","service":"api"}
{"level":"info","ts":"2026-01-30T10:00:02.456Z","msg":"Connected to database","service":"api","db":"postgres"}
{"level":"info","ts":"2026-01-30T10:00:03.789Z","msg":"Health check passed","service":"api"}
{"level":"warn","ts":"2026-01-30T10:05:11.111Z","msg":"Slow query detected","service":"api","duration_ms":2340,"query":"SELECT * FROM utm_visits"}
{"level":"error","ts":"2026-01-30T10:10:22.222Z","msg":"Connection pool exhausted","service":"api","active":50,"max":50}
{"level":"error","ts":"2026-01-30T10:10:23.333Z","msg":"Connection pool exhausted","service":"api","active":50,"max":50}
{"level":"error","ts":"2026-01-30T10:10:24.444Z","msg":"Connection pool exhausted","service":"api","active":50,"max":50}
{"level":"info","ts":"2026-01-30T10:10:25.555Z","msg":"Pool recovered","service":"api","active":12,"max":50}
{"level":"info","ts":"2026-01-30T10:15:00.000Z","msg":"Health check passed","service":"api"}
{"level":"info","ts":"2026-01-30T10:20:00.000Z","msg":"Health check passed","service":"api"}"#;
        let raw_chars = docker_logs.len();
        let result = filter_json_logs(docker_logs);
        println!("\n--- 3. docker logs (JSON, 10 lines) ---");
        println!("RAW: {} chars", raw_chars);
        println!(
            "FILTERED ({} chars, {:.0}% saved):",
            result.len(),
            (1.0 - result.len() as f64 / raw_chars as f64) * 100.0
        );
        println!("{}", result);

        // --- 4. HTML page ---
        let html = r#"<!DOCTYPE html>
<html lang="ru">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Minicoder — Развивашки для программистов</title>
<script type="application/ld+json">{"@type":"WebSite","name":"Minicoder"}</script>
<script type="application/ld+json">{"@type":"Organization","name":"Minicoder"}</script>
<script async src="https://www.googletagmanager.com/gtag/js?id=G-XXXXXXXXXX"></script>
<script>window.dataLayer=window.dataLayer||[];function gtag(){dataLayer.push(arguments);}gtag('js',new Date());gtag('config','G-XXXXXXXXXX');</script>
<script src="https://mc.yandex.ru/metrika/tag.js"></script>
<link rel="stylesheet" href="/styles.css">
</head>
<body>
<header><nav><a href="/">Home</a><a href="/courses">Courses</a><a href="/blog">Blog</a></nav></header>
<main>
<section class="hero"><h1>Научись программировать за 30 дней</h1><p>Современные курсы для начинающих и продвинутых разработчиков</p></section>
<section class="courses"><div class="course-card"><h2>React для начинающих</h2></div><div class="course-card"><h2>TypeScript Pro</h2></div></section>
</main>
<footer><p>&copy; 2026 Minicoder</p></footer>
</body>
</html>"#;
        let raw_chars = html.len();
        let result = filter_html(html, 0);
        println!("\n--- 4. HTML page (minicoder.ru) ---");
        println!("RAW: {} chars", raw_chars);
        println!(
            "FILTERED ({} chars, {:.0}% saved):",
            result.len(),
            (1.0 - result.len() as f64 / raw_chars as f64) * 100.0
        );
        println!("{}", result);

        // --- 5. Generic: docker ps text output ---
        let docker_ps = r#"CONTAINER ID   IMAGE                                    COMMAND                  CREATED       STATUS                 PORTS                                      NAMES
abc123def456   supabase/postgres:15.1.0.147              "docker-entrypoint.s…"   2 weeks ago   Up 2 weeks (healthy)   0.0.0.0:5432->5432/tcp                     supabase-db-abc123
bcd234efg567   supabase/gotrue:v2.132.3                  "gotrue"                 2 weeks ago   Up 2 weeks (healthy)   0.0.0.0:9999->9999/tcp                     supabase-auth-abc123
cde345fgh678   supabase/postgrest:v12.0.2                "/bin/postgrest"         2 weeks ago   Up 2 weeks             0.0.0.0:3000->3000/tcp                     supabase-rest-abc123
def456ghi789   supabase/realtime:v2.25.50                "/usr/bin/tini -s -…"    2 weeks ago   Up 2 weeks             0.0.0.0:4000->4000/tcp                     supabase-realtime-abc123
efg567hij890   supabase/storage-api:v0.43.11             "docker-entrypoint.s…"   2 weeks ago   Up 2 weeks             0.0.0.0:5000->5000/tcp                     supabase-storage-abc123
fgh678ijk901   supabase/studio:20240101-8f3a2b1          "docker-entrypoint.s…"   2 weeks ago   Up 2 weeks             0.0.0.0:8000->3000/tcp                     supabase-studio-abc123
ghi789jkl012   kong:2.8.1                                "/docker-entrypoint.…"   2 weeks ago   Up 2 weeks             0.0.0.0:8443->8443/tcp, 8001/tcp, 8444/tcp supabase-kong-abc123
hij890klm123   darthsim/imgproxy:v3.21                   "imgproxy"               2 weeks ago   Up 2 weeks             0.0.0.0:8002->8080/tcp                     supabase-imgproxy-abc123"#;
        let raw_chars = docker_ps.len();
        let result = filter_generic(docker_ps, 0);
        println!("\n--- 5. Generic: docker ps (8 containers, text) ---");
        println!("RAW: {} chars", raw_chars);
        println!(
            "FILTERED ({} chars, {:.0}% saved):",
            result.len(),
            (1.0 - result.len() as f64 / raw_chars as f64) * 100.0
        );
        println!("{}", result);

        // --- 6. Short generic: uptime ---
        let uptime = " 10:15:22 up 42 days, 3:12, 1 user, load average: 0.15, 0.22, 0.18";
        let result = filter_generic(uptime, 0);
        println!("\n--- 6. Generic: uptime (1 line) ---");
        println!("FILTERED:");
        println!("{}", result);

        // --- 7. SSH noise + psql result ---
        let stderr = "Pseudo-terminal will not be allocated because stdin is not a terminal.\nWarning: Permanently added '192.168.1.100' (ED25519) to the list of known hosts.";
        let noise_result = strip_ssh_noise(stderr);
        println!("\n--- 7. SSH noise stripping ---");
        println!("RAW stderr: {:?}", stderr);
        println!("FILTERED stderr: {:?}", noise_result);

        // --- 8. psql SELECT with NO wide columns (everything visible) ---
        let psql_narrow = r#" name     | age | city
----------+-----+----------
 Alice    |  30 | Moscow
 Bob      |  25 | SPb
 Charlie  |  35 | Kazan
 Diana    |  28 | Novosibirsk
 Eve      |  22 | Samara
(5 rows)"#;
        let raw_chars = psql_narrow.len();
        let result = filter_psql_table(psql_narrow, 0);
        println!("\n--- 8. psql SELECT (narrow cols, all visible) ---");
        println!("RAW: {} chars", raw_chars);
        println!(
            "FILTERED ({} chars, {:.0}% saved):",
            result.len(),
            (1.0 - result.len() as f64 / raw_chars as f64) * 100.0
        );
        println!("{}", result);

        println!("\n{}", "=".repeat(60));
        println!("=== END VISUAL REVIEW ===");
    }
}
