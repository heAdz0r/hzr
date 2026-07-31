use regex::Regex;
use std::collections::BTreeSet;
use std::sync::OnceLock;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiagnosticSummary {
    pub warnings: usize,
    pub errors: usize,
    pub warning_files: Vec<String>,
    pub error_files: Vec<String>,
}

impl DiagnosticSummary {
    pub fn warnings_line(&self) -> String {
        format_diag_line("warnings", self.warnings, &self.warning_files)
    }

    pub fn errors_line(&self) -> String {
        format_diag_line("errors", self.errors, &self.error_files)
    }
}

pub fn analyze_output(output: &str) -> DiagnosticSummary {
    #[derive(Clone, Copy)]
    enum Level {
        Warning,
        Error,
    }

    static GENERATED_WARN_RE: OnceLock<Regex> = OnceLock::new();
    static ABORT_ERRORS_RE: OnceLock<Regex> = OnceLock::new();
    static RUST_ARROW_RE: OnceLock<Regex> = OnceLock::new();
    static FILE_LOC_RE: OnceLock<Regex> = OnceLock::new();
    static TS_ERR_RE: OnceLock<Regex> = OnceLock::new(); // changed: TypeScript error pattern

    let generated_warn_re = GENERATED_WARN_RE.get_or_init(|| {
        Regex::new(r"^warning: .* generated (\d+) warnings?$").expect("invalid GENERATED_WARN_RE")
    });
    let abort_errors_re = ABORT_ERRORS_RE.get_or_init(|| {
        Regex::new(r"^error: aborting due to (\d+) previous errors?$")
            .expect("invalid ABORT_ERRORS_RE")
    });
    // TypeScript/vue-tsc: "src/App.tsx:12:3 - error TS2345: msg" // changed: TS error detection
    let ts_err_re = TS_ERR_RE.get_or_init(|| {
        Regex::new(r"^[A-Za-z0-9_./@-]+\.[a-z]+:\d+:\d+\s+-\s+error\s+(TS\d+):").expect("TS_ERR_RE")
    });
    let rust_arrow_re =
        RUST_ARROW_RE.get_or_init(|| Regex::new(r"^-->\s+(.+?):\d+(?::\d+)?$").expect("invalid"));
    let file_loc_re = FILE_LOC_RE.get_or_init(|| {
        Regex::new(r"^([A-Za-z0-9_./\\-]+\.[A-Za-z0-9_+.-]+)(?::|\()").expect("invalid")
    });

    let mut warnings_explicit = 0usize;
    let mut warnings_generated = 0usize;
    let mut errors_explicit = 0usize;
    let mut errors_aborting = 0usize;
    let mut pending: Option<Level> = None;
    let mut warning_files = BTreeSet::new();
    let mut error_files = BTreeSet::new();

    for line in output.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() {
            continue;
        }

        if let Some(caps) = generated_warn_re.captures(trimmed) {
            let n = caps
                .get(1)
                .and_then(|m| m.as_str().parse::<usize>().ok())
                .unwrap_or(0);
            warnings_generated += n;
            pending = None;
            continue;
        }
        if let Some(caps) = abort_errors_re.captures(trimmed) {
            let n = caps
                .get(1)
                .and_then(|m| m.as_str().parse::<usize>().ok())
                .unwrap_or(0);
            errors_aborting += n;
            pending = None;
            continue;
        }

        if trimmed.starts_with("warning:") || trimmed.starts_with("warning[") {
            if trimmed.contains(" generated ") && trimmed.contains("warning") {
                pending = None;
                continue;
            }
            warnings_explicit += 1;
            pending = Some(Level::Warning);
            if let Some(file) = extract_inline_file(trimmed, rust_arrow_re, file_loc_re) {
                warning_files.insert(file);
            }
            continue;
        }

        // TypeScript compiler error: "src/file.tsx:12:3 - error TS2345: ..." // changed
        if ts_err_re.is_match(trimmed) {
            errors_explicit += 1;
            pending = Some(Level::Error);
            if let Some(file) = extract_inline_file(trimmed, rust_arrow_re, file_loc_re) {
                error_files.insert(file);
            }
            continue;
        }

        if trimmed.starts_with("error:") || trimmed.starts_with("error[") {
            if trimmed.contains("aborting due to")
                || trimmed.contains("could not compile")
                || trimmed.contains("test run failed")
            {
                pending = None;
                continue;
            }
            errors_explicit += 1;
            pending = Some(Level::Error);
            if let Some(file) = extract_inline_file(trimmed, rust_arrow_re, file_loc_re) {
                error_files.insert(file);
            }
            continue;
        }

        if let Some(file) = extract_inline_file(trimmed, rust_arrow_re, file_loc_re) {
            match pending {
                Some(Level::Warning) => {
                    warning_files.insert(file);
                }
                Some(Level::Error) => {
                    error_files.insert(file);
                }
                None => {}
            }
        }
    }

    DiagnosticSummary {
        warnings: warnings_explicit.max(warnings_generated),
        errors: errors_explicit.max(errors_aborting),
        warning_files: warning_files.into_iter().collect(),
        error_files: error_files.into_iter().collect(),
    }
}

fn extract_inline_file(
    trimmed: &str,
    rust_arrow_re: &Regex,
    file_loc_re: &Regex,
) -> Option<String> {
    if let Some(caps) = rust_arrow_re.captures(trimmed) {
        return caps.get(1).map(|m| m.as_str().to_string());
    }
    if let Some(caps) = file_loc_re.captures(trimmed) {
        return caps.get(1).map(|m| m.as_str().to_string());
    }
    None
}

fn format_diag_line(label: &str, count: usize, files: &[String]) -> String {
    if count == 0 {
        return format!("{label}: 0");
    }
    if files.is_empty() {
        return format!("{label}: {count}");
    }
    let max_files = 3usize;
    let shown: Vec<&str> = files.iter().take(max_files).map(|s| s.as_str()).collect();
    if files.len() > max_files {
        format!(
            "{label}: {count} ({} +{})",
            shown.join(", "),
            files.len() - max_files
        )
    } else {
        format!("{label}: {count} ({})", shown.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rust_warnings_and_files() {
        let output = r#"warning: unused variable: `start`
   --> src/init.rs:561:17
warning: constant `BILLION` is never used
  --> src/cc_economics.rs:17:7
warning: `rtk` (bin "rtk" test) generated 17 warnings
"#;
        let d = analyze_output(output);
        assert_eq!(d.warnings, 17);
        assert_eq!(d.errors, 0);
        assert!(d.warning_files.contains(&"src/init.rs".to_string()));
        assert!(d.warning_files.contains(&"src/cc_economics.rs".to_string()));
    }
}
