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
