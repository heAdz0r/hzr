#![cfg(unix)]

use std::process::Command;

#[test]
fn internal_evasion_metadata_is_not_inherited_by_native_children() {
    let attribution = r#"{"class":"e10_capability_gap","wrapper_depth":0,"path_form":"bare","stage_count":1,"hatch_marker":false,"avoidable":false,"tier":"t0_transparent_rewrite","fidelity_validation":"not_requested"}"#;
    let status = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .env("HZR_INTERNAL_EVASION_JSON", attribution)
        .args([
            "raw",
            "/bin/sh",
            "-c",
            "test -z \"${HZR_INTERNAL_EVASION_JSON+x}\"",
        ])
        .status()
        .expect("run managed fork child");

    assert!(status.success(), "internal attribution leaked to child");
}

#[test]
fn host_grant_marker_is_consumed_and_persisted_on_the_operation() {
    let directory = tempfile::tempdir().expect("temporary database directory");
    let database = directory.path().join("tracking.sqlite");
    let status = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .env("RTK_DB_PATH", &database)
        .env("HZR_INTERNAL_HOST_GRANT_APPLIED", "1")
        .args([
            "raw",
            "/bin/sh",
            "-c",
            "test -z \"${HZR_INTERNAL_HOST_GRANT_APPLIED+x}\"; printf recorded",
        ])
        .status()
        .expect("run grant-approved managed fork child");

    assert!(status.success(), "host grant marker leaked to child");
    let connection = rusqlite::Connection::open(database).expect("tracking database");
    let applied: bool = connection
        .query_row(
            "SELECT host_grant_applied FROM commands ORDER BY id DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .expect("persisted host grant marker");
    assert!(applied);
}
