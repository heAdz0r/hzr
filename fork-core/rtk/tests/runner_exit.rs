use std::process::Command;

#[cfg(unix)]
#[test]
fn generic_test_and_err_preserve_child_argv_and_exit_code() {
    let rtk = env!("CARGO_BIN_EXE_rtk");

    for route in ["test", "err"] {
        let output = Command::new(rtk)
            .args([route, "sh", "-c", "exit 7"])
            .output()
            .expect("run generic filtered route");
        assert_eq!(
            output.status.code(),
            Some(7),
            "{route} must preserve child argv and exit code"
        );
    }
}
