# High Critical review: exact search and full-read utilization

Date: 2026-08-21
Ledger cutoff: `commands.id <= 52917`
Last included record: `2026-08-21T07:48:19.189705+00:00`

## Verdict

HZR encouraged two avoidable high-output habits: a generic Grep example selected exact mode,
and the generated Codex bootstrap required a complete `HZR.md --level none` read before ordinary
work. Historical abuse cannot be counted exactly because old ledger rows collapse search and
read modes. This report therefore separates identifiable lower bounds from candidate exposure;
none of these figures are provider-billed savings.

## Measurement boundary

Older `rtk rgai` rows do not distinguish exact, builtin-ranked, semantic, or auto fallback.
Older `rtk read` rows do not distinguish smart, full, range, outline, or other bounded modes.
Fork JSON is also internal transport and is not necessarily identical to model-visible output.

| Candidate surface | Operations | Estimated delivered | Seven-day operations | Seven-day delivered |
|---|---:|---:|---:|---:|
| Search mode unknown (`rtk rgai`) | 5,081 | 1,163,672 | 2,358 | 387,879 |
| Read mode unknown (`rtk read`) | 11,632 | 18,230,797 | 2,037 | 3,994,208 |
| Identifiable exact-search lower bound | about 40 | about 30,000 | not reliable | not reliable |
| Identifiable `read --level none` lower bound | 134 | 190,621 | not reliable | not reliable |

Of the generic reads, 798 exceeded 4,096 estimated output tokens and produced 6,918,973 tokens,
or 38.0% of generic-read output. By comparison, 770 outline reads reduced a 3,996,067-token
baseline to 146,059 delivered tokens, a measured 96.34% reduction for that route.

The HZR-owned bootstrap was a directly actionable source: 114 complete installed-contract reads
delivered 384,950 estimated tokens. The final seven-day window contained 48 such reads and
176,297 tokens. Full reads of a selected `SKILL.md` remain outside this finding because host
policy requires the selected skill instructions to be read completely.

## Exactness policy

Use exact search for a known literal, symbol, configuration key, exact error, source-faithful
quote, or presence/absence audit. Constrain it to the smallest known path. Use semantic search or
context planning for an implementation or concept that cannot yet be named. When uncertain, omit
the mode and retain HZR's `auto` default.

For file reads:

- use `--outline` or `--symbols` when structure is unknown;
- use `--from` and `--to` for exact evidence or edits;
- use `--changed` or `--since` for working-tree review;
- do not append `--level none` to a range because a range is already exact;
- request the complete exact file only when the whole source is materially authoritative.

HZR never weakens a bounded or explicitly justified exact request. The
`HZR_EXACT_FIDELITY=1` marker is the intentional whole-file escape hatch.

## Implemented acceptance gates

1. Both generated agent surfaces reject automatic full-contract import/bootstrap and retain a
   canonical pointer with outline-then-range recovery.
2. An unbounded managed `read --level none` resolves to the smart default in core, hook, and
   daemon routing.
3. Ranges, numbered reads, heads, tails, outlines, symbols, changed reads, and explicit fidelity
   remain exact.
4. Generated search guidance defaults discovery to `auto`/semantic and reserves exact for known
   literals.
5. New rows carry typed, non-sensitive operation mode and accounting stage; query text, paths,
   file contents, and secrets are absent from the attribution schema.
6. Mode summaries distinguish internal transport from final delivery. Historical rows remain
   unattributed instead of being guessed.
7. `hzr doctor` rejects stale managed policy and a local legacy RTK/ICM override.
8. `hzr init --if-needed` refreshes owned regions idempotently and retires complete delimited
   legacy RTK blocks while preserving surrounding project rules.
9. The release workflow runs every `acceptance_gate_` test before building artifacts and
   publishes a stable tag explicitly as GitHub Latest.

## Local-project audit

The activation mode was `all`, so 114 discovered local Git roots inherit the two user-global
managed instruction surfaces and the single `~/.local/share/hzr/current` engine pointer. No root
contained an old local HZR managed block. Seven active project directories contained a delimited
legacy RTK instruction region; the lifecycle migration targets only those conflicting files.
The archived `/Users/andrew/Programming/rtk` source is intentionally not modified. HZR's owned
`fork-core/rtk/CLAUDE.md` was migrated as part of the current-engine delta and remains covered by
the full fork parity gate.
