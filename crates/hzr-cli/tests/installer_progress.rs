use std::fs;
use std::process::{Command, Output};

use tempfile::tempdir;

fn run_fixture(animated: bool, commands: &str) -> Output {
    let temp = tempdir().expect("fixture home");
    let source = include_str!("../../../install.sh");
    let definitions = source
        .split("HZR_ARCHIVE=\"${HZR_INSTALL_TEMP}/${HZR_ARTIFACT}\"")
        .next()
        .expect("installer definitions");
    assert!(!definitions.contains("hzr_step \"Using the local release archive\""));
    let script = temp.path().join("fixture.sh");
    fs::write(
        &script,
        format!(
            "{definitions}\nHZR_ANIMATED={}\n{commands}\n",
            u8::from(animated)
        ),
    )
    .expect("fixture script");
    Command::new("/bin/sh")
        .arg(script)
        .env("HOME", temp.path())
        .env("TMPDIR", temp.path())
        .env("TERM", "dumb")
        .output()
        .expect("run isolated installer helpers")
}

#[test]
fn animation_stops_and_preserves_subprocess_failure() {
    let output = run_fixture(
        true,
        "hzr_run 'Checking fixture' /bin/sh -c 'sleep 0.3; echo precise-failure >&2; exit 7'",
    );
    assert_eq!(output.status.code(), Some(7));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.matches("\r\u{1b}[2K").count() >= 2, "{stderr}");
    assert!(stderr.contains("Failed: Checking fixture"), "{stderr}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("precise-failure"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains('✓'));
}

#[test]
fn redirected_install_output_stays_plain() {
    let output = run_fixture(
        false,
        "hzr_run 'Checking fixture' /bin/sh -c 'echo fixture-ready'",
    );
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Checking fixture"));
    assert!(stdout.contains("fixture-ready"));
    assert!(!stdout.contains('\u{1b}'));
    assert!(!output.stderr.contains(&b'\r'));
}

#[test]
fn shell_download_retries_partial_transfers() {
    let output = run_fixture(
        false,
        r#"
HZR_FIXTURE_ATTEMPTS=0
curl() {
  HZR_FIXTURE_ATTEMPTS=$((HZR_FIXTURE_ATTEMPTS + 1))
  case " $* " in *" --continue-at - "*) ;; *) return 99 ;; esac
  if [ "${HZR_FIXTURE_ATTEMPTS}" = 1 ]; then
    printf abc >"${HZR_DOWNLOAD_DESTINATION}"
    return 18
  fi
  printf def >>"${HZR_DOWNLOAD_DESTINATION}"
}
download_hzr_file https://example.invalid/archive "${HZR_INSTALL_TEMP}/archive"
[ "${HZR_FIXTURE_ATTEMPTS}" = 2 ]
[ "$(cat "${HZR_INSTALL_TEMP}/archive")" = abcdef ]
"#,
    );
    assert!(output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stdout).contains("retry 1/3"));
}
