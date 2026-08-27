# HZR 0.6.4 — the numbers explain themselves

0.6.3 made HZR's efficiency measurable. 0.6.4 fixes the part that was easiest to miss and
mattered most: **the surface that reports it.**

Upgrading to 0.6.3 moved the accounting-policy version forward. That silently pushed every
previously recorded operation out of the default view. On a workspace holding 252.65 million
avoided tokens, `hzr stats` printed this:

```text
TOOL OUTPUT BEFORE    DELIVERED TO MODEL    OPERATIONS
33.4K                 33.4K                 9

0 TOKENS AVOIDED                                 0.0% of tool output
```

Nothing was lost. Nothing was miscounted. The evidence was in the ledger the whole time and the
renderer never mentioned that it had narrowed the scope by 76,682 operations. A working install
read as a dead one.

That is the defect class this release closes, and it runs in both directions: **numbers that
exist and are not shown, and answers that exist and are not carried to the process that needs
them.**

## A zero now says which zero it is

```text
│  0 TOKENS AVOIDED                                 0.0% of tool output  │
│  ░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░░  │
│                                                                        │
│  0.0% is a scope artifact, not a measurement.                          │
│  76.8K operation(s) were recorded under an earlier accounting           │
│  policy than privacy_typed_v2 and sit outside this view.                │
│  recover them with: hzr stats --accounting-version all                  │
```

Three zeros are three different claims, and 0.6.3 rendered all of them identically:

| Cause | What it means |
|---|---|
| scope artifact | history exists, one flag brings it back |
| zero-credit scope | every operation in scope earns no credit by policy — a clean zero |
| empty ledger | nothing has been recorded yet |

## The panels reconcile

`OPERATION MODES` counted delivery and control-plane stages that the reduction ratio deliberately
excludes, so a table reporting 202 searches sat directly beneath a headline claiming 10 operations
with nothing connecting them. Both queries were right. Neither said which denominator it used.

Each row now carries whether it is inside the ratio, the excluded totals are reported, and the
reason is stated where the numbers disagree: a delivery stage must not double-count the
`internal_transport` row that measured it.

Underneath, the three counters now partition every recorded row in scope — measured stages,
stage-excluded, earlier-policy — enforced by a property gate. A future predicate cannot narrow one
bucket without a row falling out of all of them.

## Money, for the two scopes you actually compare

```text
ECONOMICS  estimated potential · public list price · never an invoice
╭───────────────────┬────────────────┬─────────────────┬─────────────────╮
│ SCOPE             │ AVOIDED TOKENS │ POTENTIAL SAVED │ BILLED (ACTUAL) │
├───────────────────┼────────────────┼─────────────────┼─────────────────┤
│ this project      │         68.94M │      USD 344.70 │    not measured │
│ global lifetime   │        252.66M │     USD 1263.28 │    not measured │
╰───────────────────┴────────────────┴─────────────────┴─────────────────╯
```

0.6.3 could price exactly one scope, at the bottom of the output, only if an opt-in flag was
already set — so the release that introduced money shipped a surface on which money was, in
practice, never visible.

Both rows always render. Potential and billed are adjacent columns and **never a sum**: one is
public-list arithmetic on an estimate, the other an imported receipt. A scope with no receipt reads
`not measured`, never a currency zero. When pricing is not configured, the block states the reason
*and* the steps that enable it, and still shows the token figure.

## Tables that stay inside their columns

The mode panel formatted its family cell without truncation, so a 13-character `observability`
pushed every following column out of the frame. Columns are now declared once — a width and an
alignment — and every cell renders through that declaration. A cell wider than its column is no
longer expressible.

Closed vocabularies render as stable short labels instead of mid-word ellipsis: `standalone` told
a reader more than `standalone_delive…` ever did.

## One intent, one verdict

HZR evaluated the same command twice, in two processes, with two different amounts of information.

```text
hooks dispatch  (sees permission_mode)   → allow, returns:  … hzr exec run '…'
        └── hzr exec run  (sees nothing) → approval required … exit 77
```

The host approved. HZR's own child then refused the thing the host approved. An operator running
Claude Code in `bypassPermissions` was prompted for approvals they had already granted.

The host's execution grant is now typed and propagates to every descendant of an approved command,
over the same channel that already carried session identity. The reconciliation logic moved out of
the hook and next to the decision type — a private copy in one caller is precisely what let two
callers disagree.

It is fail-closed on every axis: honoured only for the session it names, within a bounded
lifetime, from a mode that actually grants execution. A malformed grant is refused, not partially
trusted. A grant copied into a script or left in a stale shell approves nothing.

And it removes the prompt, never the guarantees: the managed route still applies, an explicit deny
still stands, and a grant-approved operation is recorded as such and still counts as avoidable
leakage with zero savings credit. `hzr doctor` fails when propagation drifts, because managed prose
is regenerated on every reconciliation pass and the only durable place to state an invariant is a
check that can fail.

## Fail-open is visible while it still matters

Bypass was already typed, credited zero, and reported. What was missing was timing. If the daemon
stopped at minute 20, you learned it from `hzr doctor` or the end-of-session card — after the
session whose numbers were now partial.

The transition from complete to unknown accounting is now announced in-session, once, when it
happens, with the recovery command. A status-line fragment keeps `ACCOUNTING: DEGRADED` on screen
for as long as it is true. Recovery is announced once too — a banner that only ever appears is one
operators learn to dismiss. A session that spent time degraded says so and withholds partial
totals instead of presenting them as complete.

This is a visibility change, deliberately. Daemon fault tolerance is out of scope: `hzrd` is one
supervised local process, and real fault tolerance would be a different architecture rather than
more supervision patches. 0.6.4 detects the flip and says so; it does not try to prevent it.

## The session card tells the same truth as `hzr stats`

A zero explains its cause. The `Saved` line carries the priced value inline. `Measured commands`
no longer sits beside a larger hook-event count with nothing accounting for the difference — the
card itemizes stage-excluded, unmeasured/native, earlier-policy and hook-only events, and the
figures partition the total. An ask recorded in a session that carries a host grant is reported as
a propagation failure, because under a grant the expected count is zero.

## Completeness contracts now fail

"Command families need explicit completeness contracts" was a sentence in the README. Sentences
do not fail. Every filtered route now declares, in code, what it will never drop:

| Route | Undroppable |
|---|---|
| `test` | exit status, failures, warnings |
| `err`, `summary` | exit status, failures |
| `log` | failures, warnings (it filters a stream, so there is no child status to keep) |
| `build` | exit status, failures, warnings, changed files |
| `write` | exit status, failures, changed files |
| `read`, `search` | no failure semantics; bound instead to an explicit pointer back to raw output |

And the contract is proved by running the filters, not by reading them. That found two live
defects on the first execution:

- **`rtk summary` returned exit `0` for a child that exited `3`.** A filter that turns a red run
  green is worse than no filter, because the summary is believed. It now propagates the child's
  status, as `rtk test` and `rtk err` always did.
- **`write` promised its exit status and changed-file list but not its failure lines**, so a
  refused patch could read as a completed one.

## The re-run tax is a number now

A filtered result can make the model re-issue the command it just ran. Those tokens are a real
cost of filtering, and nothing counted them — the second run looked like ordinary traffic, so the
tax was reported as zero by omission rather than by evidence.

`RERUN TAX` reports repeats of an already-filtered command within a bounded window of the same
session, de-duplicated, next to the pessimistic net the headline would show if the cost were
subtracted. The headline keeps its own definition: silently redefining a metric that shipped a
release ago would be a worse fault than the one being fixed, and a repeat has causes other than
filtering.

## Where a filter fires is now a policy question

Delivered bytes and provider-billed input are not the same axis. A harness that caches the request
prefix bills a cached read far below a fresh one, and a filter firing mid-turn rewrites content the
prefix already carries — invalidating everything after it. A route can cut delivered bytes hard
and still raise the bill.

```toml
[policy]
filter_placement = "anywhere"       # shipped default, unchanged
# filter_placement = "turn_boundary"  # filter only a turn's first operation
```

Under `turn_boundary` the reduction given up is counted, so protecting the prefix reads as a trade
rather than a free improvement.

## A benchmark for what is actually paid

Every HZR benchmark so far measures delivered command-output size. That is honest about what it is
and it is not the number an operator pays.
[`hzr-billed-input-prefix-cache-v0.6.4`](benchmarks/hzr-billed-input-prefix-cache-v0.6.4/README.md)
runs the same 14 cases with the metric changed to **provider-billed input**, cache dimensions
reported separately, across both placement arms.

**No numbers are claimed yet.** The harness refuses to emit a comparison without a paired provider
receipt for every case in both arms, lists every unmeasured case with its reason, and exits
non-zero. A modelled billed-input figure is exactly what it exists to replace.

The hypothesis is stated and falsifiable: mid-turn filtering may raise billed input even while
delivered bytes fall sharply. If the numbers agree, the efficiency claim survives its strongest
objection. If they disagree, we find out before a skeptic does.

## Also fixed

`hzr exec run` and both MCP surfaces resolved the session from the harness-native variables only,
ignoring the `HZR_SESSION_ID` the hook exports. Operations started *by* an approved command
recorded no session, so per-session figures read zero for exactly the traffic HZR had just routed.

Managed prose is word-wrapped after interpolation, so a long install path stays intact but can
land on the line after the phrase introducing it. The contract-path parser read one line at a time
and therefore reported *no contract at all* — a presentation layer silently destroying a
machine-readable reference, which is this release's theme in miniature.

## Upgrading

No migration. If your reduction figures dropped to zero after 0.6.3, they were never gone:

```bash
hzr stats --accounting-version all
```

0.6.4 tells you that on its own, in the panel where the zero appears.
