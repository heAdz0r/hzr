use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde_json::{Map, Value};

use crate::fidelity::{self, FidelityReason};
use crate::tracking;

const MAX_ROWS: usize = 500;
const MAX_OUTPUT_TOKENS: usize = 8_192;
const MAX_STRING_CHARS: usize = 240;
const EXACT_REASONS: &[FidelityReason] = &[FidelityReason::MachineProtocol];

pub fn run(
    database: &Path,
    query: &str,
    columns: &[String],
    max_rows: usize,
    max_tokens: usize,
    verbose: u8,
) -> Result<()> {
    validate_bounds(max_rows, max_tokens)?;
    let exact = fidelity::exact_requested(EXACT_REASONS)?;
    let sql = if exact {
        if !columns.is_empty() {
            bail!("--columns is incompatible with HZR_RAW_FIDELITY=1; project in the SQL query");
        }
        validate_single_select(query)?.to_owned()
    } else {
        bounded_query(query, columns, max_rows)?
    };

    if verbose > 0 {
        eprintln!("sqlite3 bounded read");
    }
    let timer = tracking::TimedExecution::start();
    let output = Command::new("sqlite3")
        .args(["-readonly", "-json"])
        .arg(database)
        .arg(&sql)
        .output()
        .context("Failed to run sqlite3. Is sqlite3 available?")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = crate::utils::make_raw(&stdout, &stderr);
    if !output.status.success() {
        timer.track(
            "sqlite3 <database omitted> <query omitted>",
            "rtk sqlite3",
            &raw,
            "FAILED",
        );
        if !stderr.trim().is_empty() {
            eprintln!("{}", stderr.trim());
        }
        std::process::exit(output.status.code().unwrap_or(1));
    }

    let shown = if exact {
        stdout.into_owned()
    } else {
        render_rows(&stdout, max_rows, max_tokens)?
    };
    print!("{shown}");
    if !shown.ends_with('\n') {
        println!();
    }
    timer.track(
        "sqlite3 <database omitted> <query omitted>",
        "rtk sqlite3",
        &raw,
        &shown,
    );
    Ok(())
}

fn validate_bounds(max_rows: usize, max_tokens: usize) -> Result<()> {
    if !(1..=MAX_ROWS).contains(&max_rows) {
        bail!("--max-rows must be between 1 and {MAX_ROWS}");
    }
    if !(64..=MAX_OUTPUT_TOKENS).contains(&max_tokens) {
        bail!("--max-tokens must be between 64 and {MAX_OUTPUT_TOKENS}");
    }
    Ok(())
}

pub(crate) fn validate_single_select(query: &str) -> Result<&str> {
    let query = query.trim();
    let query = query.strip_suffix(';').unwrap_or(query).trim_end();
    if query.is_empty() || query.contains(';') {
        bail!("sqlite3 route accepts exactly one SELECT statement");
    }
    let Some(first) = query.split_ascii_whitespace().next() else {
        bail!("sqlite3 route accepts exactly one SELECT statement");
    };
    if !first.eq_ignore_ascii_case("select") {
        bail!("sqlite3 route is read-only and accepts SELECT statements only");
    }
    let lowercase = query.to_ascii_lowercase();
    if lowercase.contains("--") || lowercase.contains("/*") {
        bail!("sqlite3 route rejects SQL comments; submit one explicit SELECT");
    }
    if [
        "load_extension",
        "writefile",
        "readfile",
        "fts3_tokenizer",
        "zipfile",
        "fsdir",
        "edit",
    ]
    .iter()
    .any(|function| contains_function_call(&lowercase, function))
    {
        bail!("sqlite3 route rejects filesystem, extension, and process-capable SQL functions");
    }
    Ok(query)
}

fn contains_function_call(query: &str, function: &str) -> bool {
    query.match_indices(function).any(|(start, _)| {
        let before_is_identifier = query[..start]
            .chars()
            .next_back()
            .is_some_and(|ch| ch == '_' || ch.is_ascii_alphanumeric());
        let after = &query[start + function.len()..];
        !before_is_identifier && after.trim_start().starts_with('(')
    })
}

fn bounded_query(query: &str, columns: &[String], max_rows: usize) -> Result<String> {
    let query = validate_single_select(query)?;
    let projection = if columns.is_empty() {
        "*".to_owned()
    } else {
        columns
            .iter()
            .map(|column| quote_identifier(column))
            .collect::<Result<Vec<_>>>()?
            .join(", ")
    };
    Ok(format!(
        "SELECT {projection} FROM ({query}) AS hzr_query LIMIT {}",
        max_rows.saturating_add(1)
    ))
}

fn quote_identifier(identifier: &str) -> Result<String> {
    let mut chars = identifier.chars();
    let valid_start = chars
        .next()
        .is_some_and(|ch| ch == '_' || ch.is_ascii_alphabetic());
    if !valid_start || !chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric()) {
        bail!("invalid projected column name");
    }
    Ok(format!("\"{identifier}\""))
}

fn render_rows(raw: &str, max_rows: usize, max_tokens: usize) -> Result<String> {
    let json = if raw.trim().is_empty() { "[]" } else { raw };
    let Value::Array(mut rows) =
        serde_json::from_str(json).context("sqlite3 returned invalid JSON")?
    else {
        bail!("sqlite3 JSON output was not an array");
    };
    let row_cap_hit = rows.len() > max_rows;
    rows.truncate(max_rows);
    let columns = rows
        .first()
        .and_then(Value::as_object)
        .map(|row| row.keys().cloned().collect::<Vec<_>>().join(","))
        .unwrap_or_default();
    let fetched = if row_cap_hit {
        format!("{}+", rows.len())
    } else {
        rows.len().to_string()
    };
    let mut output = if columns.is_empty() {
        format!("rows_fetched={fetched}\n")
    } else {
        format!("rows_fetched={fetched} columns={columns}\n")
    };
    let mut token_cap_hit = false;
    for (index, row) in rows.into_iter().enumerate() {
        let row = bounded_row(row);
        let line = format!("{} {}\n", index + 1, serde_json::to_string(&row)?);
        if tracking::estimate_tokens(&format!("{output}{line}")) > max_tokens {
            token_cap_hit = true;
            break;
        }
        output.push_str(&line);
    }
    if row_cap_hit || token_cap_hit {
        let recovery = "... bounded; recovery: narrow --columns/WHERE, raise caps within limits, or use HZR_RAW_FIDELITY=1 for an exact SELECT\n";
        while tracking::estimate_tokens(&format!("{output}{recovery}")) > max_tokens
            && output.lines().count() > 1
        {
            let end = output[..output.trim_end().rfind('\n').unwrap_or(0)]
                .trim_end()
                .len();
            output.truncate(end);
            output.push('\n');
        }
        output.push_str(recovery);
    }
    Ok(output)
}

fn bounded_row(row: Value) -> Value {
    let Value::Object(values) = row else {
        return row;
    };
    Value::Object(
        values
            .into_iter()
            .map(|(key, value)| (key, bounded_scalar(value)))
            .collect::<Map<_, _>>(),
    )
}

fn bounded_scalar(value: Value) -> Value {
    match value {
        Value::String(value) if value.chars().count() > MAX_STRING_CHARS => {
            let mut bounded = value.chars().take(MAX_STRING_CHARS).collect::<String>();
            bounded.push('…');
            Value::String(bounded)
        }
        value => value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_is_single_select_with_projection_and_probe_row() {
        assert_eq!(
            bounded_query(
                "SELECT id, name, secret FROM users ORDER BY id;",
                &["id".into(), "name".into()],
                20,
            )
            .unwrap(),
            "SELECT \"id\", \"name\" FROM (SELECT id, name, secret FROM users ORDER BY id) AS hzr_query LIMIT 21"
        );
        assert!(bounded_query("DELETE FROM users", &[], 20).is_err());
        assert!(bounded_query("SELECT 1; SELECT 2", &[], 20).is_err());
        assert!(bounded_query("SELECT writefile('/tmp/out', value) FROM users", &[], 20).is_err());
        assert!(bounded_query("SELECT * FROM users", &["id, secret".into()], 20).is_err());
    }

    #[test]
    fn rows_are_capped_projected_and_token_bounded() {
        let raw = serde_json::to_string(
            &(0..80)
                .map(|id| serde_json::json!({"id": id, "text": "x".repeat(500)}))
                .collect::<Vec<_>>(),
        )
        .unwrap();
        let shown = render_rows(&raw, 20, 256).unwrap();

        assert!(shown.starts_with("rows_fetched=20+ columns=id,text\n"));
        assert!(shown.contains("... bounded; recovery:"));
        assert!(tracking::estimate_tokens(&shown) <= 256);
        assert!(!shown.contains(&"x".repeat(300)));
    }

    #[test]
    fn exact_sqlite_requires_machine_protocol() {
        assert!(fidelity::validate_request(
            Some(std::ffi::OsStr::new("1")),
            Some(std::ffi::OsStr::new("machine_protocol")),
            EXACT_REASONS,
        )
        .unwrap());
    }
}
