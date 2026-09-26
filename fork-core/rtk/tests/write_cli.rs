use std::fs;
use std::process::Command;

fn rtk_bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_rtk"))
}

#[test]
fn write_replace_updates_file() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let file = tmp.path().join("a.txt");
    fs::write(&file, "hello world").expect("seed file");

    let out = rtk_bin()
        .args([
            "write",
            "replace",
            file.to_str().unwrap(),
            "--from",
            "world",
            "--to",
            "rtk",
        ])
        .output()
        .expect("run rtk write replace");

    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), "hello rtk");
}

#[test]
fn write_replace_dry_run_keeps_file() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let file = tmp.path().join("b.txt");
    fs::write(&file, "hello world").expect("seed file");

    let out = rtk_bin()
        .args([
            "write",
            "replace",
            file.to_str().unwrap(),
            "--from",
            "world",
            "--to",
            "rtk",
            "--dry-run",
        ])
        .output()
        .expect("run rtk write replace dry-run");

    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), "hello world");
}

#[test]
fn write_set_json_nested_key() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let file = tmp.path().join("c.json");
    fs::write(&file, "{\"hooks\":{}}").expect("seed file");

    let out = rtk_bin()
        .args([
            "write",
            "set",
            file.to_str().unwrap(),
            "--key",
            "hooks.enabled",
            "--value",
            "true",
            "--value-type",
            "bool",
        ])
        .output()
        .expect("run rtk write set");

    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
    assert_eq!(json["hooks"]["enabled"], serde_json::Value::Bool(true));
}

#[test]
fn write_patch_missing_hunk_fails() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let file = tmp.path().join("d.txt");
    fs::write(&file, "line one").expect("seed file");

    let out = rtk_bin()
        .args([
            "write",
            "patch",
            file.to_str().unwrap(),
            "--old",
            "does-not-exist",
            "--new",
            "new text",
        ])
        .output()
        .expect("run rtk write patch");

    assert!(!out.status.success(), "expected non-zero exit status");
    assert_eq!(fs::read_to_string(&file).unwrap(), "line one");
}

#[test]
fn write_patch_accepts_old_with_leading_dash() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let file = tmp.path().join("dash.txt");
    fs::write(&file, "- headline\nnext\n").expect("seed file");

    let out = rtk_bin()
        .args([
            "write",
            "patch",
            file.to_str().unwrap(),
            "--old",
            "- headline",
            "--new",
            "* headline",
        ])
        .output()
        .expect("run rtk write patch with leading dash");

    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), "* headline\nnext\n");
}

#[test]
fn write_set_path_conflict_keeps_file_unchanged() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let file = tmp.path().join("e.json");
    fs::write(&file, "{\"a\":1}").expect("seed file");

    let out = rtk_bin()
        .args([
            "write",
            "set",
            file.to_str().unwrap(),
            "--key",
            "a.b",
            "--value",
            "2",
            "--format",
            "json",
            "--value-type",
            "number",
        ])
        .output()
        .expect("run rtk write set with conflict");

    assert!(!out.status.success(), "expected non-zero exit status");
    assert_eq!(fs::read_to_string(&file).unwrap(), "{\"a\":1}");
}

// 0.11.2: a new file takes the mode any other tool would give it (0666 & !umask), not the
// 0600 of the temp file it was persisted from.
#[cfg(unix)]
#[test]
fn write_create_new_file_honours_the_umask() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().expect("tempdir"); // 0.11.2
    let file = tmp.path().join("fresh.txt"); // 0.11.2
    let out = Command::new("/bin/sh") // 0.11.2
        .arg("-c")
        .arg("umask 022 && exec \"$0\" write create \"$1\" --content hi")
        .arg(env!("CARGO_BIN_EXE_rtk"))
        .arg(&file)
        .output()
        .expect("run rtk write create");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr)); // 0.11.2
    let mode = fs::metadata(&file).expect("created").permissions().mode() & 0o777; // 0.11.2
    assert_eq!(mode, 0o644, "new file mode {mode:o}"); // 0.11.2
}
