# 0.8.0 verification transcripts

- `source-gate-final.log`: complete source gate after the final Git diff correction; runtime
  commit `34a99fb`, 834 workspace tests and 1,962 fork tests passed. Current-engine manifest
  `a44fc6a256b36d71f09134ac4c66b4954400b4e6f2f1fa01e133b65ddcc9fc0d`.
- `bundle-final.log`: final macOS ARM64 bundle build and assembled-bundle smoke, including
  21 UI tests and the production UI build.
- `install-upgrade.log`: full archive install/upgrade smoke with corrected host-event fixtures;
  isolated HOME and service-manager stubs. The runtime/archive did not change with fixture fixes.
- `SHA256SUMS`: final packaged archive checksum. The local artifact is
  `/tmp/hzr-0.8.0-release.qBfn3s/hzr-v0.8.0-darwin-arm64.tar.gz`.
- `long-job.log`: explicit real 90-second job probe passed with a one-second request budget.
- `source-gate.log` and `fork-gate.log`: earlier passing checks before the additional Git diff
  correction; their 1,959-test fork result is historical and superseded by the final transcript.

See [the verification report](../20260905_0.8.0-Verification.md) for skipped helpers,
initial red runs, fixture corrections and limits. These are local source/artifact checks,
not evidence of published releases, real host replacement acceptance or provider-billed savings.
