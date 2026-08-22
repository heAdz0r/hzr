use crate::tracking;
use anyhow::{Context, Result};
use std::process::Command;

pub fn run(args: &[String], verbose: u8, skip_env: bool) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("npm");
    cmd.arg("run");

    for arg in args {
        cmd.arg(arg);
    }

    if skip_env {
        cmd.env("SKIP_ENV_VALIDATION", "1");
    }

    if verbose > 0 {
        eprintln!("Running: npm run {}", args.join(" "));
    }

    // Streamed, not buffered until exit. A long-running script (dev server,
    // watcher, daemon) emits its readiness line and then keeps running; with a
    // capture-until-exit read the parent never saw that line, which broke any
    // orchestration that starts a service before launching a dependent step.
    // The npm filter is line-oriented, so it maps onto the streaming filter
    // without having to guess which script names are long-running.
    let result = crate::stream::run_streaming(
        &mut cmd,
        crate::stream::StdinMode::Inherit,
        crate::stream::FilterMode::Streaming(Box::new(NpmStreamFilter::default())),
    )
    .context("Failed to run npm run")?;

    let raw = format!("{}\n{}", result.raw_stdout, result.raw_stderr);
    timer.track(
        &format!("npm run {}", args.join(" ")),
        &format!("rtk npm run {}", args.join(" ")),
        &raw,
        &result.filtered,
    );

    if result.exit_code != 0 {
        std::process::exit(result.exit_code);
    }

    Ok(())
}

/// Line-oriented streaming form of [`filter_npm_output`]: identical predicate,
/// emitted as each line arrives instead of after the child exits.
#[derive(Default)]
struct NpmStreamFilter {
    emitted: bool,
}

impl crate::stream::StreamFilter for NpmStreamFilter {
    fn feed_line(&mut self, line: &str) -> Option<String> {
        if !keep_npm_line(line) {
            return None;
        }
        self.emitted = true;
        Some(format!("{}\n", line))
    }

    fn flush(&mut self) -> String {
        // Preserve the existing contract: a run whose every line was filtered
        // still reports success rather than printing nothing.
        if self.emitted {
            String::new()
        } else {
            "ok ✓\n".to_string()
        }
    }
}

/// Filter npm run output - strip boilerplate, progress bars, npm WARN
/// Buffered form of the filter, kept as the reference the streaming path is
/// asserted against. Production output goes through [`NpmStreamFilter`].
#[cfg(test)]
fn filter_npm_output(output: &str) -> String {
    let result: Vec<&str> = output.lines().filter(|line| keep_npm_line(line)).collect();

    if result.is_empty() {
        "ok ✓".to_string()
    } else {
        result.join("\n")
    }
}

/// Single source of truth for what npm output is worth showing, shared by the
/// buffered and streaming paths so the two can never disagree.
fn keep_npm_line(line: &str) -> bool {
    // npm boilerplate
    if line.starts_with('>') && line.contains('@') {
        return false;
    }
    // npm lifecycle noise
    if line.trim_start().starts_with("npm WARN") {
        return false;
    }
    if line.trim_start().starts_with("npm notice") {
        return false;
    }
    // progress indicators
    if line.contains("⸩") || line.contains("⸨") || line.contains("...") && line.len() < 10 {
        return false;
    }
    !line.trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::StreamFilter;

    /// Drive the streaming filter the way `run_streaming` does, so the two paths
    /// are asserted to agree instead of assumed to.
    fn stream_filter(input: &str) -> String {
        let mut filter = NpmStreamFilter::default();
        let mut out = String::new();
        for line in input.lines() {
            if let Some(emitted) = filter.feed_line(line) {
                out.push_str(&emitted);
            }
        }
        out.push_str(&filter.flush());
        out.trim_end().to_string()
    }

    #[test]
    fn streaming_and_buffered_filters_agree() {
        for input in [
            "> app@1.0.0 dev\nnpm WARN deprecated\nready on :3000\n",
            "npm notice new version\n\n",
            "",
            "listening\nnpm WARN x\ncompiled\n",
        ] {
            assert_eq!(
                stream_filter(input),
                filter_npm_output(input),
                "streaming and buffered filters diverged on {input:?}"
            );
        }
    }

    #[test]
    fn streaming_emits_a_readiness_line_before_the_child_exits() {
        let mut filter = NpmStreamFilter::default();
        assert_eq!(
            filter.feed_line("ready - started server on :3000"),
            Some("ready - started server on :3000\n".to_string()),
            "a readiness line must be observable while the script is still alive"
        );
    }

    #[test]
    fn test_filter_npm_output() {
        let output = r#"
> project@1.0.0 build
> next build

npm WARN deprecated inflight@1.0.6: This module is not supported
npm notice

   Creating an optimized production build...
   ✓ Build completed
"#;
        let result = filter_npm_output(output);
        assert!(!result.contains("npm WARN"));
        assert!(!result.contains("npm notice"));
        assert!(!result.contains("> project@"));
        assert!(result.contains("Build completed"));
    }

    #[test]
    fn test_filter_npm_output_empty() {
        let output = "\n\n\n";
        let result = filter_npm_output(output);
        assert_eq!(result, "ok ✓");
    }
}
