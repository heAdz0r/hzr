//! Executable proof of the completeness contract declared in `hzr-protocol`.
//!
//! "Command families need explicit completeness contracts" was prose, and prose does not fail.
//! These tests run the real filtered routes against output that carries every class the contract
//! calls undroppable, and assert each one survives. A filter that starts swallowing a compiler
//! error, a failing test, or a non-zero exit status turns this suite red instead of turning a
//! red run green.

use std::process::Command;

/// One marker per must-keep class, chosen to be unmistakable in a diff.
const FAILURE_MARKER: &str = "error[E0599]: MUSTKEEPFAILURE no method named `frobnicate`";
const WARNING_MARKER: &str = "warning: MUSTKEEPWARNING unused variable `x`";
const CHANGED_FILE_MARKER: &str = "MUSTKEEPCHANGED src/lib.rs";

/// Noise a filter is expected and encouraged to remove, so a test that passes because the filter
/// did nothing at all is distinguishable from one that passes because it kept the right lines.
fn noise(lines: usize) -> String {
    (0..lines)
        .map(|index| format!("   Compiling filler-crate-{index} v0.1.0 (/tmp/filler-{index})\n"))
        .collect()
}

/// Routes that run a child. `log` is deliberately absent: it filters a stream someone else
/// produced, so it is exercised through stdin instead.
const CHILD_ROUTES: [&str; 3] = ["test", "err", "summary"];

fn run_route(route: &str, script: &str) -> (String, Option<i32>) {
    let output = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args([route, "sh", "-c", script])
        .output()
        .unwrap_or_else(|error| panic!("run `rtk {route}`: {error}"));
    let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&output.stderr));
    (combined, output.status.code())
}

fn run_log(input: &str) -> String {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["log"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn `rtk log`");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(input.as_bytes())
        .expect("write stdin");
    let output = child.wait_with_output().expect("wait `rtk log`");
    let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(&output.stderr));
    combined
}

/// Every failure-carrying route preserves the child's exit status.
///
/// This is the single most damaging thing a filter can get wrong: a summarized failure that
/// reports success is worse than no summary, because it is believed.
#[cfg(unix)]
#[test]
fn must_keep_exit_status_survives_every_failure_carrying_route() {
    for route in CHILD_ROUTES {
        let script = format!("printf '%s' \"{}\"; exit 3", noise(40).replace('\n', "\\n"));
        let (_, code) = run_route(route, &script);
        assert_eq!(
            code,
            Some(3),
            "`rtk {route}` reported a different exit status than the child"
        );
    }
}

/// Failure lines survive routes whose contract lists `Failures`.
#[cfg(unix)]
#[test]
fn must_keep_failures_survive_the_routes_that_promise_them() {
    for route in CHILD_ROUTES {
        let script = format!(
            "printf '%s' \"{}\"; echo '{FAILURE_MARKER}'; exit 1",
            noise(60).replace('\n', "\\n")
        );
        let (rendered, _) = run_route(route, &script);
        assert!(
            rendered.contains("MUSTKEEPFAILURE"),
            "`rtk {route}` dropped a failure line it contracted to keep\n--- output ---\n{rendered}"
        );
    }
}

/// Warnings survive the routes whose contract lists `Warnings`.
///
/// Warnings look like the safest thing to drop and are not: a project with a warning ratchet
/// fails its own gate on evidence a filter deleted.
#[cfg(unix)]
#[test]
fn must_keep_warnings_survive_the_routes_that_promise_them() {
    let script = format!(
        "printf '%s' \"{}\"; echo '{WARNING_MARKER}'; echo '{FAILURE_MARKER}'; exit 1",
        noise(60).replace('\n', "\\n")
    );
    let (rendered, _) = run_route("test", &script);
    assert!(
        rendered.contains("MUSTKEEPWARNING"),
        "`rtk test` dropped a warning it contracted to keep\n--- output ---\n{rendered}"
    );

    // `log` reads a stream rather than running anything, so it is fed the same evidence directly.
    let streamed = format!("{}{WARNING_MARKER}\n{FAILURE_MARKER}\n", noise(60));
    let rendered = run_log(&streamed);
    assert!(
        rendered.contains("MUSTKEEPWARNING"),
        "`rtk log` dropped a warning it contracted to keep\n--- output ---\n{rendered}"
    );
    assert!(
        rendered.contains("MUSTKEEPFAILURE"),
        "`rtk log` dropped a failure it contracted to keep\n--- output ---\n{rendered}"
    );
}

/// A changed-file list is the whole result of a write-shaped command.
#[cfg(unix)]
#[test]
fn must_keep_changed_files_survive_a_build_shaped_route() {
    let script = format!(
        "printf '%s' \"{}\"; echo '{CHANGED_FILE_MARKER}'; exit 0",
        noise(60).replace('\n', "\\n")
    );
    let (rendered, code) = run_route("summary", &script);
    assert_eq!(code, Some(0));
    assert!(
        rendered.contains("MUSTKEEPCHANGED"),
        "a changed-file line was dropped\n--- output ---\n{rendered}"
    );
}
