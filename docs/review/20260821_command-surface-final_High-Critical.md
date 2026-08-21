# High Critical review: command-surface utilization

Date: 2026-08-21
Ledger cutoff: `commands.id <= 53511`
Last included record: `2026-08-21T08:42:19.782207+00:00`

## Verdict

The dominant problem is routing, not a lack of specialized engines. Of the estimated tokens
delivered through RAW routes, 75.3% lifetime and 83.2% in the latest seven-day window came from
families that already have a dedicated first-class HZR route. Another 15.5% of recent RAW output
is a candidate for the existing generic `test`, `err`, `summary`, or `log` filters when the caller
has explicitly selected that output contract.

These figures are UTF-8-bytes-divided-by-four estimates, not provider billing. No provider usage
receipts were available, so the audit makes no economic savings claim.

## Observed traffic

| Route | Lifetime operations | Lifetime delivered | Seven-day operations | Seven-day delivered |
|---|---:|---:|---:|---:|
| RAW/bypass | 12,868 | 27,961,942 | 1,684 | 7,319,643 |
| Optimized | 38,320 | 29,562,314 | 5,779 | 5,110,864 |
| Native host | 2,323 | 6,858,141 | 3 | 19,061 |

The largest irrational RAW families already have dedicated filters: `bun`, `ps`, `cargo`, `ssh`,
`wget`, `git`, `rg`, `gh`, `find`, `grep`, `curl`, and bounded read equivalents for `nl`, `sed`,
and `head`. Their observed optimized counterparts reduced output substantially, but that observed
ratio is not used as a counterfactual because the optimized and RAW invocation mixes differ.

## Real gaps

The audit and live rewrite matrix identified these control-plane gaps:

1. Recent/family diagnostics required direct SQLite queries because `hzr stats` had no time window
   or privacy-safe family view.
2. Requested and effective search modes were not separated, and internal transport could be added
   to model-visible delivery totals.
3. `npm test` and `pnpm test` fell through even though their explicit `run test` forms were already
   optimized.
4. Managed instructions did not expose the existing generic filters, omitted three memory MCP
   tools, and described native measurement inconsistently with the PostToolUse observer.
5. Common read/write operations remain adjacent to the low-level `rtk -- raw` spelling; top-level
   HZR aliases are a justified usability improvement.

`bunx` is not a gap: the live rewrite matrix already routes it through the appropriate specialized
filter. `tar`, `rustup`, `yarn`, `dotnet`, `deno`, `podman`, and `fd` remain candidates, not release
requirements. Their observed volume is low or their semantics require dedicated fixtures before a
lossy filter can be safe.

## Fidelity boundary

“Never RAW” means never RAW when a demonstrably more efficient route preserves the requested
contract. It does not authorize changing output format, order, completeness, exit status, shell
grammar, or side effects. `HZR_RAW_FIDELITY=1` remains the byte-for-byte escape hatch: native `rg`
cannot be replaced by bounded search under that marker, and arbitrary `cat` cannot yet be replaced
because binary detection precedes fork-core's exact-text path.

Shell pipelines, redirects, checksums, parser input, machine-readable output, and requested full
logs therefore retain tracked fallback unless a typed route proves the required fidelity.

## Acceptance gates

- Dedicated families retry the pinned fork before any manual RAW decision in both daemon and
  degraded-hook paths.
- `npm test` and `pnpm test` preserve argv and exit status while selecting the same specialized
  filters as their `run test` forms.
- Stats can reproduce a bounded seven-day family/route audit without SQLite or RAW and never emit
  arguments, queries, paths, content, or secrets in the family view.
- Requested search mode, effective mode, strategy, and bounded fallback code are typed separately.
- Internal transport remains stage-visible but is not counted as final model-visible delivery when
  a final row exists.
- Generated Claude and Codex contracts enumerate all eight MCP tools, describe native observation
  as measurement-only, and contain no mandatory full-contract bootstrap.
- `hzr read` and `hzr write` forward argv unchanged to the inherited typed read/write engines;
  existing `hzr rtk -- read|write` invocations remain compatible.
- RAW fidelity markers remain RAW whenever an optimized route cannot prove byte-for-byte parity.

## Operational target

After one complete post-release seven-day window:

- dedicated-first-class RAW output is below 5% of all RAW delivered estimates;
- dedicated-first-class RAW output is below 500,000 estimated tokens per week;
- native Edit output remains zero and native Write remains below 1% of observed write traffic;
- every new attributed search/read row has typed mode and stage fields, with no query, path, content,
  or secret stored in attribution.

The pre-release baseline for the first target is 6,093,344 estimated tokens per week. Historical
rows remain legacy data and are not rewritten to manufacture compliance.
