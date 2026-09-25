use crate::tracking;
use crate::utils::truncate;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::process::Command;

#[derive(Debug, Deserialize)]
struct Position {
    #[serde(rename = "Filename")]
    filename: String,
    #[serde(rename = "Line")]
    line: usize,
    #[serde(rename = "Column")]
    column: usize,
}

#[derive(Debug, Deserialize)]
struct Issue {
    #[serde(rename = "FromLinter")]
    from_linter: String,
    #[serde(rename = "Text")]
    text: String,
    #[serde(rename = "Pos")]
    pos: Position,
}

#[derive(Debug, Deserialize)]
struct GolangciOutput {
    #[serde(rename = "Issues", default)]
    issues: Vec<Issue>,
}

/// Subcommands other than `run`: their output is not a lint report.
const NON_RUN_SUBCOMMANDS: &[&str] = &[
    "cache",
    "completion",
    "config",
    "custom",
    "fmt",
    "formatters",
    "help",
    "linters",
    "migrate",
    "version",
];

/// Issues listed verbatim before the per-linter tally; the rest are counted.
const MAX_LISTED_ISSUES: usize = 10;

/// Parse the major version from `golangci-lint --version`; 1 when unknown.
///
/// 0.10.0 (upstream v0.50.0): v2 removed `--out-format`, so treating a v2 binary as v1
/// made every run print nothing but "JSON parse failed: EOF". The `v` prefix depends on
/// how the binary was built and must not decide the major version.
pub(crate) fn parse_major_version(version_output: &str) -> u32 {
    for word in version_output.split_whitespace() {
        let version = word.strip_prefix('v').unwrap_or(word);
        if !version.contains('.') {
            continue;
        }
        if let Some(major) = version.split('.').next().and_then(|s| s.parse::<u32>().ok()) {
            return major;
        }
    }
    1
}

fn detect_major_version() -> u32 {
    match Command::new("golangci-lint").arg("--version").output() {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let text = if stdout.trim().is_empty() {
                String::from_utf8_lossy(&output.stderr).into_owned()
            } else {
                stdout.into_owned()
            };
            parse_major_version(&text)
        }
        Err(_) => 1,
    }
}

/// Split `[global flags] run [run args]`. The hook rewrites `golangci-lint run ./...` to
/// `rtk golangci-lint run ./...`, and 0.44 prepended its own `run`, so golangci received
/// `run --out-format=json run ./...` and linted a package literally named `run`.
fn split_run_invocation(args: &[String]) -> Option<(Vec<String>, Vec<String>)> {
    match args.iter().position(|arg| !arg.starts_with('-')) {
        Some(index) if args[index] == "run" => {
            Some((args[..index].to_vec(), args[index + 1..].to_vec()))
        }
        Some(index) if NON_RUN_SUBCOMMANDS.contains(&args[index].as_str()) => None,
        _ => Some((Vec::new(), args.to_vec())),
    }
}

fn has_output_flag(args: &[String]) -> bool {
    args.iter().any(|arg| {
        let flag = arg.split('=').next().unwrap_or(arg);
        flag == "--out-format"
            || (flag.starts_with("--output.") && flag.ends_with(".path"))
            || flag == "--output.json.path"
    })
}

fn filtered_args(global: &[String], run_args: &[String], version: u32) -> (Vec<String>, bool) {
    let mut args = global.to_vec();
    args.push("run".to_string());
    let inject = !has_output_flag(run_args);
    if inject {
        if version >= 2 {
            args.push("--output.json.path".to_string());
            args.push("stdout".to_string());
        } else {
            args.push("--out-format=json".to_string());
        }
    }
    args.extend(run_args.iter().cloned());
    (args, inject)
}

pub fn run(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let Some((global, run_args)) = split_run_invocation(args) else {
        // Not a lint report: hand the invocation to golangci-lint untouched.
        let status = Command::new("golangci-lint")
            .args(args)
            .status()
            .context("Failed to run golangci-lint")?;
        timer.track_passthrough(
            &format!("golangci-lint {}", args.join(" ")),
            &format!("rtk golangci-lint {}", args.join(" ")),
        );
        if !status.success() {
            std::process::exit(crate::stream::status_to_exit_code(status));
        }
        return Ok(());
    };

    let version = detect_major_version();
    let (command_args, injected) = filtered_args(&global, &run_args, version);
    if verbose > 0 {
        eprintln!("Running: golangci-lint {}", command_args.join(" "));
    }

    let output = Command::new("golangci-lint")
        .args(&command_args)
        .output()
        .context(
            "Failed to run golangci-lint. Is it installed? Try: go install github.com/golangci/golangci-lint/v2/cmd/golangci-lint@latest",
        )?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = crate::utils::make_raw(&stdout, &stderr);
    let exit_code = crate::stream::status_to_exit_code(output.status);

    // 0.10.0: an unparsable report (config error, crash, unknown flag) is shown as golangci
    // printed it. 0.44 replaced it with "JSON parse failed" and hid stderr unless -v.
    let summary = if injected {
        filter_golangci_json(json_payload(&stdout))
    } else {
        None
    };
    let shown = match summary {
        Some(summary) => {
            // golangci-lint exit 1 means issues were found — never render a green verdict.
            let guarded = crate::guard::guard_exit(&raw, exit_code, "golangci-lint", &summary);
            crate::guard::never_worse_rendered(&raw, &guarded).to_string()
        }
        None => raw.trim_end().to_string(),
    };
    println!("{}", shown);

    timer.track(
        &format!("golangci-lint {}", args.join(" ")),
        &format!("rtk golangci-lint {}", args.join(" ")),
        &raw,
        &shown,
    );

    // Exit 1 is "issues found"; anything else is a tool failure the caller must see.
    if exit_code > 1 {
        std::process::exit(exit_code);
    }
    Ok(())
}

/// v2 may print a text summary after the JSON document on the same stream; the report is
/// the first line that parses as a JSON object.
fn json_payload(stdout: &str) -> &str {
    stdout
        .lines()
        .find(|line| line.trim_start().starts_with('{'))
        .unwrap_or(stdout)
}

/// Summarise a golangci-lint JSON report, or `None` when it is not one.
///
/// 0.10.0: the first issues are listed as `path:line:col linter: text` — the 0.44 summary
/// named only linters and files, so fixing anything took a second, unfiltered run.
fn filter_golangci_json(output: &str) -> Option<String> {
    let report: GolangciOutput = serde_json::from_str(output.trim()).ok()?;
    let issues = report.issues;
    if issues.is_empty() {
        return Some("✓ golangci-lint: No issues found".to_string());
    }

    let total_files = issues
        .iter()
        .map(|issue| issue.pos.filename.as_str())
        .collect::<std::collections::HashSet<_>>()
        .len();
    let mut by_linter: HashMap<&str, usize> = HashMap::new();
    for issue in &issues {
        *by_linter.entry(issue.from_linter.as_str()).or_insert(0) += 1;
    }
    let mut linter_counts: Vec<_> = by_linter.into_iter().collect();
    linter_counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let tally = linter_counts
        .iter()
        .map(|(linter, count)| format!("{linter} {count}"))
        .collect::<Vec<_>>()
        .join(", ");

    let mut result = format!(
        "golangci-lint: {} issues in {} files ({})\n",
        issues.len(),
        total_files,
        tally
    );
    for issue in issues.iter().take(MAX_LISTED_ISSUES) {
        // Typecheck issues arrive as ": # pkg\nfile.go:4:9: cannot use …"; the package
        // header line carries nothing, the next one is the diagnosis.
        let text = issue
            .text
            .lines()
            .map(str::trim)
            .filter(|line| {
                let bare = line.trim_start_matches(':').trim_start();
                !bare.is_empty() && !bare.starts_with('#')
            })
            .collect::<Vec<_>>()
            .join(" | ");
        result.push_str(&format!(
            "{}:{}:{} {}: {}\n",
            issue.pos.filename,
            issue.pos.line,
            issue.pos.column,
            issue.from_linter,
            truncate(&text, 160)
        ));
    }
    if issues.len() > MAX_LISTED_ISSUES {
        result.push_str(&format!(
            "… +{} more issues\n",
            issues.len() - MAX_LISTED_ISSUES
        ));
    }
    Some(result.trim_end().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_golangci_no_issues() {
        let output = r#"{"Issues":[]}"#;
        let result = filter_golangci_json(output).expect("report");
        assert!(result.contains("✓ golangci-lint"));
        assert!(result.contains("No issues found"));
    }

    #[test]
    fn test_filter_golangci_with_issues() {
        let output = r#"{
  "Issues": [
    {
      "FromLinter": "errcheck",
      "Text": "Error return value not checked",
      "Pos": {"Filename": "main.go", "Line": 42, "Column": 5}
    },
    {
      "FromLinter": "errcheck",
      "Text": "Error return value not checked",
      "Pos": {"Filename": "main.go", "Line": 50, "Column": 10}
    },
    {
      "FromLinter": "gosimple",
      "Text": "Should use strings.Contains",
      "Pos": {"Filename": "utils.go", "Line": 15, "Column": 2}
    }
  ]
}"#;

        let result = filter_golangci_json(output).expect("report");
        assert!(result.contains("3 issues"));
        assert!(result.contains("main.go:42:5 errcheck: Error return value not checked"));
        assert!(result.contains("2 files"));
        assert!(result.contains("errcheck"));
        assert!(result.contains("gosimple"));
        assert!(result.contains("main.go"));
        assert!(result.contains("utils.go"));
    }

    // 0.10.0: version detection, run splitting and the parse fallback
    #[test]
    fn parses_v1_and_v2_versions_with_or_without_prefix() {
        assert_eq!(parse_major_version("golangci-lint has version 1.59.1 built with go1.22"), 1);
        assert_eq!(
            parse_major_version("golangci-lint has version 2.8.0 built with go1.25.5 from e2e40021"),
            2
        );
        assert_eq!(parse_major_version("golangci-lint has version v2.13.2 built"), 2);
        assert_eq!(parse_major_version(""), 1);
    }

    #[test]
    fn v2_gets_the_json_sink_and_run_is_not_duplicated() {
        let args = vec!["run".to_string(), "./...".to_string()];
        let (global, run_args) = split_run_invocation(&args).expect("run");
        let (command, injected) = filtered_args(&global, &run_args, 2);
        assert!(injected);
        assert_eq!(command, ["run", "--output.json.path", "stdout", "./..."]);
        let (command, _) = filtered_args(&global, &run_args, 1);
        assert_eq!(command, ["run", "--out-format=json", "./..."]);
        assert!(split_run_invocation(&["linters".to_string()]).is_none());
        let user = vec!["run".to_string(), "--output.text.path=stdout".to_string()];
        let (global, run_args) = split_run_invocation(&user).expect("run");
        assert!(!filtered_args(&global, &run_args, 2).1);
    }

    #[test]
    fn multiline_typecheck_text_keeps_the_diagnosis() {
        let report = r#"{"Issues":[{"FromLinter":"typecheck","Text":": # redmod/c\nc/c.go:4:9: cannot use \"x\" as int value","Pos":{"Filename":"c/c.go","Line":1,"Column":0}}]}"#;
        let summary = filter_golangci_json(report).expect("report");
        assert!(summary.contains("typecheck: c/c.go:4:9: cannot use"), "{summary}");
        assert!(!summary.contains("# redmod/c"), "{summary}");
    }

    #[test]
    fn unparsable_report_is_not_replaced_by_a_parse_error() {
        assert_eq!(filter_golangci_json(""), None);
        assert_eq!(filter_golangci_json("level=error msg=\"unknown flag\""), None);
    }
}
