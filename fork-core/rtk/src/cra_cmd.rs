use crate::utils::strip_ansi;
use regex::Regex;
use std::collections::HashMap;

/// Detect if output is from Create React App (react-scripts build)
pub fn is_cra_output(output: &str) -> bool {
    output.contains("react-scripts build")
        || output.contains("Creating an optimized production build")
        || (output.contains("File sizes after gzip") && output.contains("build/static/"))
}

/// Filter CRA build output to ~2 compact lines
/// Strips: browserslist warnings, deployment instructions, empty lines
/// Keeps: compilation status, eslint warnings (grouped by rule), file sizes (top bundle)
pub fn filter_cra_build(output: &str) -> String {
    let clean = strip_ansi(output);

    // Detect compilation result
    let has_errors = clean.contains("Failed to compile");
    let has_warnings = clean.contains("Compiled with warnings");

    // --- Extract eslint warnings by rule ---
    lazy_static::lazy_static! {
        // Pattern: "  Line N:C:  message  rule-name"
        static ref ESLINT_RULE: Regex = Regex::new(
            r"Line \d+:\d+:\s+.+?\s{2,}(\S+)$"
        ).unwrap();
        // Pattern for file sizes: "  182.11 kB (+650 B)  build/static/js/main.b4ebe182.js"
        static ref FILE_SIZE: Regex = Regex::new(
            r"^\s+(\d+(?:\.\d+)?)\s+(kB|B)(?:\s+\([^)]+\))?\s+(build/static/\S+)"
        ).unwrap();
        // Pattern for eslint file headers: "src/components/ImageUploader.tsx"
        static ref ESLINT_FILE: Regex = Regex::new(
            r"^(src/\S+\.\w+)$"
        ).unwrap();
    }

    let mut rule_counts: HashMap<String, usize> = HashMap::new();
    let mut warning_files = 0usize;
    let mut total_warnings = 0usize;
    let mut bundles: Vec<(String, f64, String)> = Vec::new(); // (path, size, unit)

    let mut in_eslint_block = false;
    let mut in_file_sizes = false;

    for line in clean.lines() {
        let trimmed = line.trim();

        // Detect eslint block start
        if trimmed == "[eslint]" {
            in_eslint_block = true;
            continue;
        }

        // Detect file sizes block
        if trimmed.starts_with("File sizes after gzip") {
            in_file_sizes = true;
            in_eslint_block = false;
            continue;
        }

        // Parse file sizes block (skip empty lines within the block)
        if in_file_sizes {
            if let Some(caps) = FILE_SIZE.captures(line) {
                let size: f64 = caps[1].parse().unwrap_or(0.0);
                let unit = caps[2].to_string();
                let path = caps[3].to_string();
                // Normalize to kB for sorting
                let size_kb = if unit == "B" { size / 1024.0 } else { size };
                bundles.push((path, size_kb, unit));
            } else if !trimmed.is_empty() {
                in_file_sizes = false; // end block on non-empty non-matching line
            }
        }

        // Parse eslint warnings
        if in_eslint_block {
            if ESLINT_FILE.is_match(trimmed) {
                warning_files += 1;
                continue;
            }
            if let Some(caps) = ESLINT_RULE.captures(trimmed) {
                let rule = caps[1].to_string();
                *rule_counts.entry(rule).or_insert(0) += 1;
                total_warnings += 1;
            }
        }
    }

    // --- Build compact output ---
    let mut result = String::new();

    if has_errors {
        result.push_str("✗ CRA Build: Failed to compile\n");
        // On error, pass through error lines (they're useful)
        let mut in_error = false;
        for line in clean.lines() {
            if line.contains("Failed to compile") {
                in_error = true;
                continue;
            }
            if in_error && !line.trim().is_empty() {
                result.push_str(&format!("  {}\n", line.trim()));
            }
        }
        return result.trim().to_string();
    }

    // Line 1: status + bundle summary
    let bundle_summary = if !bundles.is_empty() {
        bundles.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let top = &bundles[0];
        // Extract short filename from path
        let short_name = top.0.rsplit('/').next().unwrap_or(&top.0);
        // Truncate hash from filename: main.b4ebe182.js → main.js
        let display_name = simplify_bundle_name(short_name);
        format!(
            "{} files, {:.1} kB {} (gzip)",
            bundles.len(),
            top.1,
            display_name
        )
    } else {
        "built".to_string()
    };

    result.push_str(&format!("✓ CRA Build: {}", bundle_summary));

    // Line 2: eslint summary (if warnings present)
    if has_warnings && total_warnings > 0 {
        let mut rules: Vec<_> = rule_counts.iter().collect();
        rules.sort_by(|a, b| b.1.cmp(a.1));
        let top_rules: Vec<String> = rules
            .iter()
            .take(3)
            .map(|(rule, count)| format!("{} ({}x)", rule, count))
            .collect();
        result.push_str(&format!(
            "\n  eslint: {} warnings in {} files — {}",
            total_warnings,
            warning_files,
            top_rules.join(", ")
        ));
    }

    result
}

/// Simplify CRA bundle name: main.b4ebe182.js → main.js
fn simplify_bundle_name(name: &str) -> String {
    lazy_static::lazy_static! {
        static ref HASH_RE: Regex = Regex::new(r"\.[a-f0-9]{6,12}\.").unwrap();
    }
    HASH_RE.replace(name, ".").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CRA_OUTPUT: &str = r#"$ react-scripts build
Creating an optimized production build...
Browserslist: browsers data (caniuse-lite) is 16 months old. Please run:
  npx update-browserslist-db@latest
  Why you should do it regularly: https://github.com/browserslist/update-db#readme
Browserslist: browsers data (caniuse-lite) is 16 months old. Please run:
  npx update-browserslist-db@latest
  Why you should do it regularly: https://github.com/browserslist/update-db#readme
Compiled with warnings.

[eslint]
src/components/ImageUploader.tsx
  Line 5:10:   'cropImage' is defined but never used                @typescript-eslint/no-unused-vars
  Line 15:15:  'Grid' is defined but never used                     @typescript-eslint/no-unused-vars
  Line 16:10:  'toast' is defined but never used                    @typescript-eslint/no-unused-vars
  Line 45:5:   'updateCropArea' is assigned a value but never used  @typescript-eslint/no-unused-vars

src/components/image/CropOverlay.tsx
  Line 209:9:  'handlePresetSelect' is assigned a value but never used                                                                   @typescript-eslint/no-unused-vars
  Line 241:6:  React Hook useEffect has a missing dependency: 'handleInteractionMove'. Either include it or remove the dependency array  react-hooks/exhaustive-deps
  Line 266:6:  React Hook useEffect has a missing dependency: 'handleInteractionMove'. Either include it or remove the dependency array  react-hooks/exhaustive-deps

src/components/subscription/tokenContext.tsx
  Line 24:28:  'setSubscriptionType' is assigned a value but never used  @typescript-eslint/no-unused-vars

src/contexts/PipelineContext.tsx
  Line 281:6:  React Hook useCallback has a missing dependency: 'STEP_ORDER'. Either include it or remove the dependency array  react-hooks/exhaustive-deps

src/hooks/useGrid.ts
  Line 2:16:  'GridLine' is defined but never used  @typescript-eslint/no-unused-vars

src/locales/en.ts
  Line 2:1:  Assign object to a variable before exporting as module default  import/no-anonymous-default-export

src/locales/ru.ts
  Line 2:1:  Assign object to a variable before exporting as module default  import/no-anonymous-default-export

src/pages/CarbonitePage.tsx
  Line 108:8:  React Hook useCallback has a missing dependency: 't'. Either include it or remove the dependency array  react-hooks/exhaustive-deps

src/pages/SaberPage.tsx
  Line 40:10:  'blobStore' is defined but never used                                                                   @typescript-eslint/no-unused-vars
  Line 647:8:  React Hook useCallback has a missing dependency: 't'. Either include it or remove the dependency array  react-hooks/exhaustive-deps

Search for the keywords to learn more about each warning.
To ignore, add // eslint-disable-next-line to the line before.

File sizes after gzip:

  182.11 kB (+650 B)  build/static/js/main.b4ebe182.js
  28.13 kB            build/static/js/159.6244a4fc.chunk.js
  13.24 kB (+122 B)   build/static/css/main.8d1e29d9.css
  11.9 kB             build/static/js/875.aac6d8d4.chunk.js
  8.64 kB (-2.93 kB)  build/static/js/338.86aa5e14.chunk.js
  5.17 kB             build/static/js/844.c7a15596.chunk.js
  5.16 kB (-2.54 kB)  build/static/js/628.9960fa3b.chunk.js
  3.3 kB              build/static/js/248.3965168a.chunk.js
  795 B               build/static/js/864.2996d099.chunk.js
  412 B               build/static/js/885.ec0bfbb4.chunk.js
  257 B               build/static/js/863.02956dff.chunk.js

The project was built assuming it is hosted at /.
You can control this with the homepage field in your package.json.

The build folder is ready to be deployed.
You may serve it with a static server:

  npm install -g serve
  serve -s build

Find out more about deployment here:

  https://cra.link/deployment"#;

    #[test]
    fn test_is_cra_output_positive() {
        assert!(is_cra_output(CRA_OUTPUT));
        assert!(is_cra_output("Creating an optimized production build..."));
        assert!(is_cra_output(
            "File sizes after gzip:\n  100 kB  build/static/js/main.js"
        ));
    }

    #[test]
    fn test_is_cra_output_negative() {
        assert!(!is_cra_output("next build"));
        assert!(!is_cra_output("vite build"));
        assert!(!is_cra_output("cargo build"));
    }

    #[test]
    fn test_filter_cra_build_compact() {
        let result = filter_cra_build(CRA_OUTPUT);
        let lines: Vec<&str> = result.lines().collect();
        // Must be <= 3 lines (status + eslint summary)
        assert!(
            lines.len() <= 3,
            "Expected <=3 lines, got {}: {:?}",
            lines.len(),
            lines
        );
        assert!(result.contains("✓ CRA Build"));
    }

    #[test]
    fn test_filter_cra_build_bundles() {
        let result = filter_cra_build(CRA_OUTPUT);
        // Must mention file count and largest bundle
        assert!(
            result.contains("11 files"),
            "Missing file count in: {}",
            result
        );
        assert!(
            result.contains("182.1"),
            "Missing largest bundle size in: {}",
            result
        );
        assert!(result.contains("main.js"), "Missing main.js in: {}", result);
    }

    #[test]
    fn test_filter_cra_build_eslint_summary() {
        let result = filter_cra_build(CRA_OUTPUT);
        // Must show eslint summary with rule counts
        assert!(
            result.contains("eslint:"),
            "Missing eslint summary in: {}",
            result
        );
        assert!(
            result.contains("@typescript-eslint/no-unused-vars"),
            "Missing rule name in: {}",
            result
        );
        assert!(
            result.contains("warnings"),
            "Missing warning count in: {}",
            result
        );
    }

    #[test]
    fn test_filter_cra_build_strips_noise() {
        let result = filter_cra_build(CRA_OUTPUT);
        // Must NOT contain noise
        assert!(
            !result.contains("Browserslist"),
            "Should strip browserslist warning"
        );
        assert!(
            !result.contains("caniuse-lite"),
            "Should strip caniuse-lite warning"
        );
        assert!(
            !result.contains("serve -s build"),
            "Should strip deployment instructions"
        );
        assert!(!result.contains("cra.link"), "Should strip deployment URL");
        assert!(
            !result.contains("Search for the keywords"),
            "Should strip eslint help text"
        );
    }

    #[test]
    fn test_filter_cra_build_no_warnings() {
        let output = r#"Creating an optimized production build...
Compiled successfully.

File sizes after gzip:

  182.11 kB  build/static/js/main.b4ebe182.js
  28.13 kB   build/static/js/159.6244a4fc.chunk.js

The build folder is ready to be deployed."#;
        let result = filter_cra_build(output);
        let lines: Vec<&str> = result.lines().collect();
        assert_eq!(lines.len(), 1, "No warnings = 1 line. Got: {:?}", lines);
        assert!(result.contains("✓ CRA Build"));
        assert!(!result.contains("eslint"));
    }

    #[test]
    fn test_filter_cra_build_error() {
        let output = r#"Creating an optimized production build...
Failed to compile.

src/App.tsx(10,5): error TS2322: Type 'string' is not assignable to type 'number'."#;
        let result = filter_cra_build(output);
        assert!(
            result.contains("✗ CRA Build: Failed to compile"),
            "Got: {}",
            result
        );
        assert!(result.contains("TS2322"));
    }

    #[test]
    fn test_simplify_bundle_name() {
        assert_eq!(simplify_bundle_name("main.b4ebe182.js"), "main.js");
        assert_eq!(
            simplify_bundle_name("159.6244a4fc.chunk.js"),
            "159.chunk.js"
        );
        assert_eq!(simplify_bundle_name("main.css"), "main.css"); // no hash
    }
}
