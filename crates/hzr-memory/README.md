# hzr-memory

`hzr-memory` is the only HZR owner of the ICM process. It pins the executable
contract to ICM `0.10.61` (`icm-v0.10.61`, commit
`c3a1bac7cfe401b55fd66af16dfc0c774c02167a`) and rejects a different
`icm --version`. Installers can additionally supply the SHA-256 of the
installed executable.

The HZR daemon passes its canonical `Config.data_dir` to
`IcmConfig::from_data_root`. `IcmConfig::discover` uses the same
`ProjectDirs("dev", "headz0r", "hzr").data_local_dir()` fallback as
`hzr-core`.

## Canonical transport

The default is one supervised MCP process, launched without a shell:

```text
icm --db <HZR_DATA_ROOT>/memory/icm/memories.db serve --compact
```

`--no-embeddings` is inserted before `serve` when embeddings are disabled. HZR
defaults to this FTS-only mode so a clean install cannot turn its first durable
write into an implicit model download. Set `engines.icm_embeddings = true` in
the HZR config only after provisioning ICM's embedding runtime/model; health
then reports full readiness. FTS-only operation is reported as a degraded
capability warning, not a failed memory store.
HZR initializes JSON-RPC/MCP `2024-11-05`, sends
`notifications/initialized`, and serializes every request through one bounded
stdio connection. Process, request, and framing failures have typed errors and
timeouts; a broken stream is discarded instead of risking response-ID drift.

The pinned executable reports `0.10.61`, while its MCP subcrate reports
`serverInfo.version = 0.10.34`. This is an upstream release-version skew at the
pinned commit. HZR verifies both exact values rather than treating the MCP
subcrate version as an executable downgrade.

Store uses typed `tools/call` with `icm_memory_store`. This upstream path
performs near-duplicate updates, auto-linking, backrefs, and configured
auto-consolidation. ICM exposes the result only as MCP text content, without
`structuredContent`; HZR checks the structured `isError` flag but deliberately
keeps successful text opaque and returns a receipt without parsing an ID.

Recall uses the pinned one-shot CLI against the same database:

```text
icm --db <DB> recall <QUERY> --limit <N> --format json
```

This is the only ICM `0.10.61` interface combining a machine-readable result
with hybrid/FTS fallback, graph-neighbor expansion, post-expansion filters, and
access bookkeeping. It reloads the embedding runtime, but preserves the full
recall semantics without parsing human output.

The supervisor lock prevents a second long-lived ICM process for the same HZR
data root. Owned stdio processes receive EOF and termination on stop, restart,
or drop. The PID and authenticated readiness recovery path prevents duplicate
HTTP compatibility processes after an unclean daemon exit.

## HTTP compatibility mode

`IcmTransport::Http` is retained for clients prioritizing warm typed JSON reads:

```text
icm --db <DB> serve --http 127.0.0.1:11435 --token <random-token>
```

Readiness requires both `GET /health` and authenticated JSON
`GET /stats?format=json`; recall/store use `POST /recall?format=json` and
`POST /store?format=json`. The upstream HTTP handlers do not provide full
MCP/CLI parity: store omits near-dup update, auto-link/backrefs and
auto-consolidation, while recall omits graph-neighbor expansion. For that
reason HTTP is not the default correctness path.

ICM `0.10.61` supports HTTP authentication only through `serve --token`; it has
no environment-variable or token-file alternative. The token can therefore be
visible to a same-host user allowed to inspect process command lines. HZR
minimizes this upstream limitation by binding only to loopback, generating a
persistent 64-character token from two UUIDv4 values, storing it with mode
`0600` on Unix, redacting it from Rust `Debug`, and never logging it.

## Failure policy

The circuit breaker serializes half-open probes and reports its state. Recall
can safely retry through the JSON CLI after an availability failure. Store
falls back to CLI only when MCP/HTTP was unavailable before a write could be
sent; it never retries an ambiguous timeout that may already have committed.
CLI store success text remains opaque, so its receipt also has no parsed ID.
