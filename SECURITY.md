# Security policy

## Reporting a vulnerability

Please do not open a public issue for a suspected vulnerability. Use GitHub's
private vulnerability reporting for `heAdz0r/hzr` so details can be investigated
before disclosure.

Include the affected HZR version, platform, reproduction steps, impact, and any
suggested mitigation. Reports involving hook ownership, command rewriting,
filesystem boundaries, daemon authentication, archive verification, or secret
exposure are especially useful with a minimal proof of concept.

## Supported versions

HZR is currently pre-release software. Security fixes are applied to the latest
published `0.x` release and `main`; older pre-release lines may require an upgrade.

Release archives are accompanied by SHA-256 checksums. The installer verifies the
downloaded archive and the bundle's internal manifest before changing the active
version.
