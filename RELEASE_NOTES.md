# HZR 0.8.2

HZR 0.8.2 makes session accounting and the Stop scorecard behave the way an operator expects:
in-flight commands are no longer reported as accounting gaps, a degraded interval no longer
hides the measured part of the session, and the public price estimate uses the model the host
actually reports, priced from a catalog refreshed on 2026-09-05.

## In flight is not degraded

Every managed command registers a fork-producer context before it runs and recovers it when its
receipts drain. Until 0.8.1 that registration counted as an open gap from the first millisecond,
so the prompt hook, the status line and the scorecard flipped to `ACCOUNTING: DEGRADED` whenever
the previous command was still finishing; sessions accumulated hours of "degraded" time while
the daemon was healthy. Registrations younger than ten minutes are now `pending_producer_operations`
and are excluded from `live_complete`; they become gaps only when receipts fail to arrive.

## A lower bound beats "unknown"

The scorecard used to withhold every total once the session had any degraded interval, even a
ten-second daemon restart. It now prints the measured savings, marks them `PARTIAL` with the
unmeasured seconds and operation count, labels the public-list value a lower bound over measured
operations only, and keeps leakage figures with the same qualifier. Unknown stays unknown only
when the ledger itself is unavailable.

## Priced at the model in use

The Claude Code status line hook records `model.id`; the scorecard prices avoided tokens at that
model when the catalog knows it and otherwise falls back to `[billing].model`. The embedded
catalog `hzr-public-api-pricing-2026-09-05-v1` adds Claude Fable 5.1, GPT-6 Astra (short/long
context), GPT-5.5, GPT-5.4 and GPT-5.4 mini, Gemini 3.8 Flash and Grok 4.5, each with its
provider source URL; all earlier rows were re-verified unchanged. Estimates remain public-list
potentials, never invoices.

## Upgrade

`hzr update` installs 0.8.2 and runs the reference-state pass; the first session on the new
version schedules it otherwise. Reconnect open MCP sessions after the upgrade. Full details:
`docs/releases/v0.8.2.md` and `CHANGELOG.md`.
