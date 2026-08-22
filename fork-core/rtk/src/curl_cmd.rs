use crate::tracking;
use crate::utils::truncate;
use anyhow::{Context, Result};
use std::io::Write;
use std::process::Command;

pub fn run(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();
    let mut cmd = Command::new("curl");
    // `-sS`, not a bare `-s`: `-s` also suppresses curl's *error* messages, so a
    // DNS, connection or TLS failure printed only "FAILED: curl " with no reason
    // and the agent had to re-run the command outside the filter to learn why.
    // `-S` restores the error text while `-s` still hides the progress meter.
    cmd.arg("-sS");

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: curl -sS {}", args.join(" "));
    }

    let output = cmd.output().context("Failed to run curl")?;
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let msg = if stderr.trim().is_empty() {
            stdout.trim().to_string()
        } else {
            stderr.trim().to_string()
        };
        eprintln!("FAILED: curl {}", msg);
        std::process::exit(output.status.code().unwrap_or(1));
    }

    if is_binary(&output.stdout) {
        let stdout = std::io::stdout();
        stdout
            .lock()
            .write_all(&output.stdout)
            .context("Failed to write binary response to stdout")?;
        timer.track_passthrough(
            &format!("curl {}", args.join(" ")),
            &format!("rtk curl {}", args.join(" ")),
        );
        return Ok(());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let raw = stdout.to_string();

    // Auto-detect JSON and pipe through filter
    let filtered = filter_curl_output(&stdout);
    let shown = crate::guard::never_worse(&raw, &filtered);
    println!("{}", shown);

    timer.track(
        &format!("curl {}", args.join(" ")),
        &format!("rtk curl {}", args.join(" ")),
        &raw,
        shown,
    );

    Ok(())
}

fn is_binary(bytes: &[u8]) -> bool {
    std::str::from_utf8(bytes).is_err()
}

fn filter_curl_output(output: &str) -> String {
    let trimmed = output.trim();

    // CHANGED: value-preserving compact_json instead of schema-only
    if (trimmed.starts_with('{') || trimmed.starts_with('['))
        && (trimmed.ends_with('}') || trimmed.ends_with(']'))
    {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
            return compact_json(&v, 0, 5, 5);
        }
    }

    let lines: Vec<&str> = trimmed.lines().collect();
    if lines.len() > 30 {
        let mut result: Vec<&str> = lines[..30].to_vec();
        result.push("");
        let msg = format!(
            "... ({} more lines, {} bytes total)",
            lines.len() - 30,
            trimmed.len()
        );
        return format!("{}\n{}", result.join("\n"), msg);
    }

    lines
        .iter()
        .map(|l| truncate(l, 200))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Compact JSON with actual values: truncates strings/arrays, limits depth.
/// Keeps scalar values visible so LLMs can reason without Python workarounds. // ADDED
fn compact_json(
    value: &serde_json::Value,
    depth: usize,
    max_depth: usize,
    max_array: usize,
) -> String {
    use serde_json::Value::{Array, Bool, Null, Number, Object, String as JString};
    let indent = "  ".repeat(depth);

    if depth > max_depth {
        return format!("{}...", indent);
    }

    match value {
        Null => format!("{}null", indent),
        Bool(b) => format!("{}{}", indent, b),
        Number(n) => format!("{}{}", indent, n),
        JString(s) => {
            if s.len() > 80 {
                format!(
                    "{}\"{}…\" ({})",
                    indent,
                    s.chars().take(77).collect::<String>().as_str(),
                    s.len()
                )
            } else {
                format!("{}\"{}\"", indent, s)
            }
        }
        Array(arr) => {
            if arr.is_empty() {
                return format!("{}[]", indent);
            }
            let shown = arr.len().min(max_array);
            let mut lines = vec![format!("{}[", indent)];
            for (i, item) in arr[..shown].iter().enumerate() {
                // fix #15: no trailing comma on last
                let rendered = compact_json(item, depth + 1, max_depth, max_array);
                let comma = if i + 1 < shown || arr.len() > shown {
                    ","
                } else {
                    ""
                };
                lines.push(format!("{}{}", rendered, comma));
            }
            if arr.len() > shown {
                lines.push(format!("{}  ... ({} total)", indent, arr.len()));
            }
            lines.push(format!("{}]", indent));
            lines.join("\n")
        }
        Object(map) => {
            if map.is_empty() {
                return format!("{}{{}}", indent);
            }
            let mut lines = vec![format!("{}{{", indent)];
            let keys: Vec<_> = map.keys().collect(); // fix #16: preserve insertion order
            let show_keys = keys.len().min(20);
            for key in &keys[..show_keys] {
                let val = &map[*key];
                let is_simple = matches!(val, Null | Bool(_) | Number(_) | JString(_));
                if is_simple {
                    let rendered = compact_json(val, depth + 1, max_depth, max_array); // fix #7: depth+1 not 0
                    lines.push(format!("{}  {}: {},", indent, key, rendered.trim_start()));
                } else {
                    lines.push(format!("{}  {}:", indent, key));
                    lines.push(compact_json(val, depth + 1, max_depth, max_array));
                }
            }
            if keys.len() > show_keys {
                lines.push(format!("{}  ... +{} keys", indent, keys.len() - show_keys));
            }
            lines.push(format!("{}}}", indent));
            lines.join("\n")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curl_failure_keeps_the_reason() {
        // `-sS` must survive: with a bare `-s` this run reports no reason at all.
        let output = std::process::Command::new("curl")
            .arg("-sS")
            .arg("http://127.0.0.1:1/definitely-not-listening")
            .output();
        let Ok(output) = output else {
            return; // curl not installed on this host
        };
        assert!(!output.status.success());
        assert!(
            !String::from_utf8_lossy(&output.stderr).trim().is_empty(),
            "curl -sS must still explain why it failed"
        );
    }

    #[test]
    fn test_filter_curl_json() {
        let output = r#"{"name": "test", "count": 42, "items": [1, 2, 3]}"#;
        let result = filter_curl_output(output);
        assert!(result.contains("name"));
        // CHANGED: compact_json shows actual values, not types
        assert!(result.contains("\"test\""));
        assert!(result.contains("42"));
    }

    #[test]
    fn test_filter_curl_json_values_visible() {
        // ADDED: verify chunk_type-style fields are readable (no Python workaround needed)
        let output = r#"{"result": {"points": [{"chunk_type": "feature_list", "score": 0.95}]}}"#;
        let result = filter_curl_output(output);
        assert!(result.contains("feature_list"));
        assert!(result.contains("0.95"));
    }

    #[test]
    fn test_filter_curl_json_array() {
        let output = r#"[{"id": 1}, {"id": 2}]"#;
        let result = filter_curl_output(output);
        assert!(result.contains("id"));
    }

    #[test]
    fn test_filter_curl_non_json() {
        let output = "Hello, World!\nThis is plain text.";
        let result = filter_curl_output(output);
        assert!(result.contains("Hello, World!"));
        assert!(result.contains("plain text"));
    }

    #[test]
    fn test_filter_curl_long_output() {
        let lines: Vec<String> = (0..50).map(|i| format!("Line {}", i)).collect();
        let output = lines.join("\n");
        let result = filter_curl_output(&output);
        assert!(result.contains("Line 0"));
        assert!(result.contains("Line 29"));
        assert!(result.contains("more lines"));
    }

    #[test]
    fn test_filter_curl_cyrillic_no_panic() {
        // Regression: byte-slicing multi-byte chars (Cyrillic) caused panic
        let long_cyrillic =
            "Реализует Service Mesh для централизованного управления сервисами и трафиком";
        let output = format!(r#"{{"description": "{}"}}"#, long_cyrillic);
        let result = filter_curl_output(&output);
        assert!(result.contains("description"));
        // must not panic; value is truncated but valid
    }

    #[test]
    fn test_is_binary_detects_invalid_utf8() {
        assert!(is_binary(&[0x1f, 0x8b, 0x08, 0x00]));
    }

    #[test]
    fn test_is_binary_accepts_utf8_text() {
        assert!(!is_binary("Héllo — ✓".as_bytes()));
        assert!(!is_binary(&[]));
    }
}
