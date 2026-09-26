//! 0.11.0 (heAdz0r/hzr#22): `grepai init`'s append to the tracked `.gitignore` is
//! undone; any other concurrent edit is kept.

use std::fs;

use hzr_index::restore_gitignore_if_only_grepai_added;

#[test]
fn grepai_gitignore_append_is_undone_and_only_that() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join(".gitignore");
    for before in [&b"target/\n"[..], b"target/", b""] {
        // grepai appends a separating newline when needed, then `.grepai/\n`.
        let mut after = before.to_vec();
        if !after.is_empty() && !after.ends_with(b"\n") {
            after.push(b'\n');
        }
        after.extend_from_slice(b".grepai/\n");
        fs::write(&path, &after).expect("write gitignore");
        restore_gitignore_if_only_grepai_added(&path, before);
        assert_eq!(
            fs::read(&path).expect("read gitignore"),
            before,
            "{before:?}"
        );
    }
    // Someone else's edit during the init is not reverted.
    fs::write(&path, b"target/\nnode_modules/\n.grepai/\n").expect("write gitignore");
    restore_gitignore_if_only_grepai_added(&path, b"target/\n");
    assert_eq!(
        fs::read(&path).expect("read gitignore"),
        b"target/\nnode_modules/\n.grepai/\n"
    );
    // Unchanged file stays as it is.
    fs::write(&path, b"target/\n").expect("write gitignore");
    restore_gitignore_if_only_grepai_added(&path, b"target/\n");
    assert_eq!(fs::read(&path).expect("read gitignore"), b"target/\n");
}
