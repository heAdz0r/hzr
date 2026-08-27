# Paired billed-input benchmark — prefix-cache aware (v0.6.4)

**Status: harness landed, not yet run. No numbers are claimed here.**

## Why this exists

Every HZR benchmark to date measures **delivered command-output size** —
`ceil(UTF-8 bytes / 4)` before and after filtering. The README says so explicitly, and that
metric is honest about what it is. It is also not the number an operator pays.

A harness that caches the request prefix bills a cached input read far below a fresh one. A
filter that fires in the middle of a turn rewrites content the prefix already carries, which
invalidates every cached token after the rewrite point. So the two axes can move in opposite
directions:

```
delivered bytes    ↓ 84%     (what we measure today)
billed input       ↑  ?      (what the operator pays)
```

That is a falsifiable claim, and nobody has tested it. If the numbers agree, the efficiency
story gets much stronger because it survives the strongest available objection. If they
disagree, we find out first — which is strictly better than a skeptic finding out for us.

## Hypothesis under test

> A filter firing mid-turn invalidates the cached request prefix, so billed input can rise even
> while delivered bytes fall sharply.

## Metric

**Provider-billed input tokens per run**, taken from imported provider receipts, not from
`bytes / 4`. Cache dimensions are reported separately and never merged:

| Reported | Source |
|---|---|
| `billed_input_fresh` | receipt `input_tokens` |
| `billed_input_cache_read` | receipt `cache_read_tokens` |
| `billed_input_cache_write` | receipt `cache_write_tokens` |
| `delivered_tokens_estimated` | HZR ledger, for comparison with the existing benchmark |

Delivered bytes are still recorded, so the two axes can be compared directly on the same run
instead of across incomparable experiments.

## Arms

The same 14 cases from
[`hzr-vs-rtk-upstream-v0.44.1`](../hzr-vs-rtk-upstream-v0.44.1/README.md), run under both
values of the `[policy] filter_placement` dimension introduced in 0.6.4:

| Arm | `filter_placement` | Expectation |
|---|---|---|
| A | `anywhere` | maximum delivered-byte reduction; prefix may be invalidated mid-turn |
| B | `turn_boundary` | filter only on a turn's first operation; prefix stays stable |

Arm A is the shipped default, so arm A is also a regression check on the existing claim.

Arm B additionally reports `placement_deferred_operations` — the reduction deliberately given up
to keep the prefix stable. Without it, arm B would look like a free improvement rather than a
trade.

## What the harness refuses to do

- **It does not estimate billed input.** Without paired provider receipts for a run, that run
  reports `not_measured`. A modelled billed-input figure would be exactly the kind of number
  this benchmark exists to replace.
- **It does not mix arms.** A comparison is emitted only when both arms completed the same case
  list on the same fixture commit.
- **It does not silently drop cases.** A case that fails to produce a receipt is listed as
  unmeasured in the output, with its reason.

## Running it

```bash
python3 benchmarks/hzr-billed-input-prefix-cache-v0.6.4/benchmark.py \
  --repo-root . \
  --hzr-binary ./target/release/hzr \
  --work-root /tmp/hzr-billed-bench \
  --output benchmarks/hzr-billed-input-prefix-cache-v0.6.4/runs/<date>/RESULTS.json
```

Provider receipts are imported per run with `hzr billing receipt`. The runner reads them back
through `hzr stats --json` and attributes them to the arm and case that produced them.
