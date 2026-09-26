//! 0.11.0: fixes ported from open upstream RTK pull requests (not in v0.50.0),
//! each reproduced on this engine first. PR numbers refer to rtk-ai/rtk.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use tempfile::{tempdir, TempDir};

fn git(cwd: &Path, args: &[&str]) -> Output {
    let out = Command::new("git").args(args).current_dir(cwd).output().expect("git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    out
}

fn rtk_in(cwd: &Path, home: &TempDir, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(args)
        .current_dir(cwd)
        .env("RTK_DB_PATH", home.path().join("history.sqlite"))
        .env("RTK_TEE", "0")
        .output()
        .expect("rtk")
}

fn repo() -> TempDir {
    let dir = tempdir().expect("temp dir");
    let r = dir.path();
    git(r, &["init", "-q", "-b", "main"]);
    git(r, &["config", "user.email", "t@example.com"]);
    git(r, &["config", "user.name", "t"]);
    fs::create_dir_all(r.join("d")).unwrap();
    fs::write(r.join("d/f.txt"), "1\n").unwrap();
    git(r, &["add", "."]);
    git(r, &["commit", "-qm", "init"]);
    dir
}

// #2542: from a subdirectory, paths are relative to it, as `git status` prints them.
#[test]
fn status_paths_are_relative_to_the_working_directory() {
    let dir = repo();
    fs::write(dir.path().join("d/f.txt"), "2\n").unwrap();
    let out = rtk_in(&dir.path().join("d"), &dir, &["git", "status"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.lines().any(|l| l.trim_end() == " M f.txt"), "{text}");
    assert!(!text.contains("d/f.txt"), "{text}");
}

// #3183 / #2573 / #3830: machine and explicitly formatted output is git's bytes.
#[test]
fn machine_and_formatted_outputs_are_exact() {
    let dir = repo();
    fs::write(dir.path().join("d/f.txt"), "2\n").unwrap();
    for args in [
        &["status", "--porcelain=v2"][..],
        &["status", "--porcelain", "-z"][..],
        &["branch", "--format=%(refname:short)"][..],
        &["branch", "--show-current"][..],
    ] {
        let native = git(dir.path(), args);
        let mut argv = vec!["git"];
        argv.extend_from_slice(args);
        let got = rtk_in(dir.path(), &dir, &argv);
        assert_eq!(got.stdout, native.stdout, "git {args:?}");
    }
}

// #4082 / #4146: git's own answer for a bare add and --dry-run; a no-op add says so.
#[test]
fn add_reports_what_this_invocation_did() {
    let dir = repo();
    fs::write(dir.path().join("d/f.txt"), "2\n").unwrap();
    // A bare `git add` stages nothing (rtk used to run `git add .`).
    let bare = rtk_in(dir.path(), &dir, &["git", "add"]);
    let staged = git(dir.path(), &["diff", "--cached", "--name-only"]);
    assert!(staged.stdout.is_empty(), "bare add staged {:?}", staged.stdout);
    assert_eq!(bare.status.code(), Command::new("git").arg("add").current_dir(dir.path()).status().unwrap().code());

    let dry = rtk_in(dir.path(), &dir, &["git", "add", "--dry-run", "d/f.txt"]);
    assert_eq!(String::from_utf8_lossy(&dry.stdout).trim(), "add 'd/f.txt'");

    let first = rtk_in(dir.path(), &dir, &["git", "add", "d/f.txt"]);
    assert!(String::from_utf8_lossy(&first.stdout).contains("1 file changed"));
    let again = rtk_in(dir.path(), &dir, &["git", "add", "d/f.txt"]);
    assert_eq!(String::from_utf8_lossy(&again.stdout).trim(), "ok (nothing new staged)");
}

// #3888: `grep -h` is --no-filename, not rtk's help.
#[test]
fn grep_h_reaches_grep() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("a.txt"), "foo 1\n").unwrap();
    let out = rtk_in(dir.path(), &dir, &["grep", "-h", "foo", "a.txt"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("foo 1"), "{text}");
    assert!(!text.contains("Usage"), "{text}");
}

// #3651: `ls link` lists the directory a command-line symlink points to.
#[cfg(unix)]
#[test]
fn ls_follows_a_command_line_directory_symlink() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("real")).unwrap();
    fs::write(dir.path().join("real/x.rs"), "").unwrap();
    std::os::unix::fs::symlink(dir.path().join("real"), dir.path().join("link")).unwrap();
    let out = rtk_in(dir.path(), &dir, &["ls", "link"]);
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("x.rs"), "{text}");
}

// #1661: `gh … --help` is gh's own text, never a fabricated confirmation.
#[cfg(unix)]
#[test]
fn gh_help_is_passed_through() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempdir().unwrap();
    let bin = dir.path().join("bin");
    fs::create_dir_all(&bin).unwrap();
    fs::write(bin.join("gh"), "#!/bin/sh\necho \"gh help for: $*\"\n").unwrap();
    fs::set_permissions(bin.join("gh"), fs::Permissions::from_mode(0o755)).unwrap();
    for sub in ["comment", "edit", "create"] {
        let out = Command::new(env!("CARGO_BIN_EXE_rtk"))
            .args(["gh", "pr", sub, "--help"])
            .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
            .env("RTK_DB_PATH", dir.path().join("h.sqlite"))
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains(&format!("gh help for: pr {sub} --help")), "{text}");
        assert!(!text.contains("ok "), "{text}");
    }
}

// #4085: `curl -d @-` sends the caller's stdin as the body.
#[test]
fn curl_forwards_stdin_bodies() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buf = vec![0u8; 4096];
        let mut request = Vec::new();
        loop {
            let n = stream.read(&mut buf).unwrap();
            request.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&request);
            if let Some(head_end) = text.find("\r\n\r\n") {
                let len = text
                    .lines()
                    .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length: ").map(|v| v.trim().parse::<usize>().unwrap()))
                    .unwrap_or(0);
                if request.len() >= head_end + 4 + len {
                    let body = request[head_end + 4..head_end + 4 + len].to_vec();
                    let reply = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
                    stream.write_all(reply.as_bytes()).unwrap();
                    stream.write_all(&body).unwrap();
                    return body;
                }
            }
            if n == 0 {
                return Vec::new();
            }
        }
    });
    let dir = tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["curl", "-d", "@-", &format!("http://127.0.0.1:{port}/")])
        .env("RTK_DB_PATH", dir.path().join("h.sqlite"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"stdin-body").unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(server.join().unwrap(), b"stdin-body");
    assert!(String::from_utf8_lossy(&out.stdout).contains("stdin-body"));
}

// #4264 / #4184 and the injected-JSON guard: vitest 4 (report on stdout) and vitest 5
// (report in the --outputFile.json file) both render a summary, never the raw report.
#[cfg(unix)]
#[test]
fn vitest_reports_render_as_summaries() {
    use std::os::unix::fs::PermissionsExt;
    let report = r#"{"numTotalTests":2,"numPassedTests":1,"numFailedTests":1,"numPendingTests":0,"testResults":[{"name":"/p/a.test.ts","status":"failed","assertionResults":[{"fullName":"adds","status":"passed","failureMessages":[]},{"fullName":"fails","status":"failed","failureMessages":["AssertionError: expected 2 to be 3"]}]}]}"#;
    let v4 = format!("#!/bin/sh\necho '{report}'\nexit 1\n");
    let v5 = format!(
        "#!/bin/sh\nfor a in \"$@\"; do case \"$a\" in --outputFile.json=*) out=\"${{a#--outputFile.json=}}\";; esac; done\necho '{report}' > \"$out\"\necho \"JSON report written to $out\"\nexit 1\n"
    );
    for (label, script) in [("vitest 4", v4), ("vitest 5", v5)] {
        let dir = tempdir().unwrap();
        let bin = dir.path().join("node_modules/.bin");
        fs::create_dir_all(&bin).unwrap();
        fs::write(bin.join("vitest"), script).unwrap();
        fs::set_permissions(bin.join("vitest"), fs::Permissions::from_mode(0o755)).unwrap();
        let out = Command::new(env!("CARGO_BIN_EXE_rtk"))
            .args(["vitest", "run"])
            .current_dir(dir.path())
            .env("PATH", "/usr/bin:/bin")
            .env("RTK_DB_PATH", dir.path().join("h.sqlite"))
            .env("RTK_TEE", "0")
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert_eq!(out.status.code(), Some(1), "{label}");
        assert!(text.contains("PASS (1) FAIL (1)"), "{label}: {text}");
        assert!(text.contains("expected 2 to be 3"), "{label}: {text}");
        assert!(!text.contains("numTotalTests"), "{label} leaked the raw report: {text}");
    }
}
