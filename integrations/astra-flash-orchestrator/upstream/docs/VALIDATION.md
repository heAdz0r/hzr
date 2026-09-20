# Validation evidence

Public source version: 1.1.0. Checked September 19, 2026 on macOS with Python 3.14.3.

## Verified

- 46 offline tests passed. Coverage includes installation dry runs, idempotence, original configuration preservation, scoped policy handling, profile/collision/symlink checks, URL validation, fake-secret redaction, generated role TOML, rollback, guarded undo, plan validation and release-file filtering.
- Native `/v1` and capability-path Router configurations are accepted; non-loopback hosts and unsupported URL shapes are rejected.
- Existing backup-directory permissions are preserved.
- Backup files and caches are excluded from skill installation. Release tests also cover private artifact exclusion, symlink rejection and inventory changes.

All tests use synthetic configuration, temporary directories and a local HTTP fixture. They do not require a provider account or invoke model inference. Python 3.11 is the minimum supported syntax/runtime target, but this release's local suite was run on 3.14.3; other versions and operating systems have not been tested here.

## Prior local installation evidence

The preceding package revision was installed in a macOS Codex setup using an Astra root and the exact Flash worker route. Static configuration checks passed; root configuration and authentication bytes were preserved. Its optional unauthenticated local catalog GET was rejected. The public revision's installer is verified with synthetic homes; this report does not claim it was reapplied to that real installation.

## Still unverified

Actual Astra-to-Flash delegated inference, native role loading in a fresh session, provider request attribution, long-running build quality and cost savings remain unverified for this package. A static report, a model catalog entry, or a worker naming itself cannot establish these facts.

Validate real routing during the first authorized useful task, using host/router request metadata. Do not run an extra paid test as part of installation, and do not publish raw private logs or local configuration as evidence.
