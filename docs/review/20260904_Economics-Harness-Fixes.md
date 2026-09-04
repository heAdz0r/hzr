# Economics harness corrections — 0.8.0

## Result

The legacy benchmark can no longer attribute cumulative provider totals to each CLI case. A new runnable task-evidence harness binds provider receipts and independently evaluated outcomes to immutable task/run identities. It is an implementation of evidence collection and validation, not proof that HZR reduces billed task cost.

## Legacy defects corrected

`benchmarks/hzr-billed-input-prefix-cache-v0.6.4/benchmark.py` now rejects aggregate stats as case receipts, preserves nonzero command exit codes in unmeasured reasons, labels repetition deltas as totals, handles an incomplete arm set safely, and removes unsupported prefix-cache causal conclusions. Its README now describes what the executable actually does. It always returns `not_measured` economics because local CLI execution has no provider request capture channel.

## New harness

`benchmarks/hzr-task-economics-v0.8.0/task_economics.py` provides:

- A clean-checkout, fixed-commit task plan with task hashes, tree digest, exact provider/model, configuration digests, deterministic randomized order and paired trials across native, RTK-only, HZR execution, HZR retrieval, HZR memory, and full HZR.
- A direct-argv adapter protocol and a separate acceptance evaluator. The evaluator's retained evidence body must match its digest and exact task/run ID. Adapter failures and evaluator failures remain explicit records. Existing run files cannot be silently replayed or overwritten.
- Per-request raw provider response verification: exact run/model/request bindings, unique IDs across the experiment, request-list coverage, nonnegative usage, and capture interval checks. OpenAI response creation time must belong to the run. Missing, foreign, duplicate, old or aggregate receipts cannot establish measured usage.
- Correct normalization of OpenAI inclusive cached-input counts and Anthropic disjoint fresh/cache-read/cache-write counts. Output reasoning is not charged twice.
- Provider-billed monetary values only when each request has an explicit billing reference, currency and amount. Token totals do not automatically become monetary savings.
- Cost per accepted task with rejected attempts retained in its numerator, plus accepted-pair token differences labeled as conditional on both tasks succeeding. Incomplete receipt coverage suppresses complete cost totals.
- Source-hash/range-aware signals for full reads, partial reads and overlap. These are descriptive signals, not automatically deducted causal token penalties.
- Explicitly synthetic adapters and three small full/range/repeated-read protocol fixtures. Offline results always remain `not_measured`; the harness never automatically sets `economic_claim_ready=true`.

## Validation

Command:

```sh
python3 -B -m unittest discover -s benchmarks/hzr-task-economics-v0.8.0 -p 'test_*.py' -v
```

**10 tests passed in 2.439 seconds** after the final validator changes. Coverage includes an 18-run offline adapter/evaluator round trip, clean-checkout and plan tampering, both provider usage models, prior/foreign/duplicate receipts, missing evidence, failed tasks in cost denominators, full/range overlap, command failures without raw stderr disclosure, and the legacy aggregate-attribution regression. No API, credentials, paid model or installed agent configuration was used. Temporary Git commits exist only inside test-owned temporary fixture directories.

## Limits that remain explicit

Provider capture adapters and acceptance evaluators are trusted instrumentation; structural JSON validation does not cryptographically authenticate a provider response or prove a host omitted no request. Adapter/evaluator identity fields currently hash their argv; pin executable/script versions or hashes in the reviewed toolchain manifest as well. The current usage normalizer supports fixed-model OpenAI Responses and Anthropic Messages experiments; mixed-provider/model subagents need explicit additional normalization rather than silently omitted receipts. Embedding, machine runtime, and external-tool expenses are not automatically inferred from model usage. Do not call model-request cost a complete infrastructure bill.

A representative real task corpus, reviewed arm configurations, cache-state controls, actual trusted capture adapters, task-quality review and sufficiently powered uncertainty estimates are still needed before an economic or SoTA claim. The three tiny protocol fixtures are not a representative benchmark and are not presented as one.
