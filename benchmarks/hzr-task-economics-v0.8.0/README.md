# HZR 0.8.0 task economics

This is an executable evidence harness, not measured savings. It runs an explicitly supplied real-agent adapter and a separate acceptance evaluator. Offline fixtures validate the protocol and never produce an economic claim.

A plan fixes task text/hash, repository commit and tracked-tree hash, provider/model, toolchain identity, arms, seed and repetitions. Native, RTK-only, HZR execution, HZR retrieval, HZR memory, and complete HZR are distinct arms. The adapter must honor the specified arm; the harness records its resolved configuration and requires its digest to match the plan. Different agents should use isolated equivalent worktrees and the same task/model settings.

Every provider request must appear once, inside its run's capture interval, with its raw response ID, exact model and usage. OpenAI cached input is a subset of total input; Anthropic fresh, cache-read and cache-write inputs are separate. Output already includes reported reasoning tokens; reasoning is not added twice. Aggregate HZR statistics cannot substitute for request receipts. Retries and failures remain in the denominator. Provider tokens are not converted to money: cost is available only when each request includes an explicit provider-billed amount, currency and billing reference.

The evaluator is a separate command from the agent adapter. It returns a run-bound accepted/rejected result and evidence hash. The harness cannot authenticate a forged provider response or dishonest evaluator; capture adapters and evaluators are trusted instrumentation and their identities must be reviewed. Passing structural validation is not a proof of provider authenticity or scientific validity.

## Protocol and commands

The CLI has three commands:

- `plan --manifest tasks.json --repo REPO --provider PROVIDER --model MODEL --toolchain-json toolchain.json --output plan.json` fixes a randomized paired schedule. The repository must be clean; use a dedicated evaluation checkout.
- `run --plan plan.json --adapter-json '["/absolute/agent-adapter"]' --evaluator-json '["/absolute/evaluator"]' --output-dir runs/` sends each run as one JSON stdin object to the adapter, then sends its result to the evaluator. Arguments are executed directly, without a shell. This may invoke paid models only when the operator chooses a paid adapter; tests do not.
- `report --plan plan.json --runs runs/ --output report.json` validates all expected runs, rejects duplicate or foreign request receipts, and reports task acceptance, provider token dimensions, latency, read overlap signals and cost per accepted task when available.

An adapter response must contain `run_id`, `task_sha256`, `repo_commit`, `repo_tree_sha256`, `arm`, `provider`, `model`, `toolchain_sha256`, `evidence_origin` (`provider_response` or `offline_fixture`), `request_ids`, `receipts`, `events`, and `answer`. Each receipt contains `run_id`, `request_id`, `observed_at`, `raw_response`, and optionally `billed_cost`. Raw response usage must use the provider's actual schema. Receipt IDs are unique across the complete experiment. An event has unique `event_id`, `kind`, `status`, and optional read fields `source_sha256`, `from_line`, `to_line`, `total_lines`, `delivered_bytes`.

Evaluator input contains the immutable run, task, and adapter result. Its output contains `run_id`, `task_sha256`, `accepted`, `evidence`, and `evidence_sha256`. The hash must match the canonical JSON evidence body retained in the run record. Include independent test/review evidence, not the agent's own success assertion.

The report requires the full expected matrix; missing/invalid runs are retained as failures of evidence coverage. It reports paired token differences only for pairs where both tasks were accepted, and labels that selection explicitly. Cost per accepted task includes all completed attempts, including rejected outcomes. No automatic SoTA verdict is emitted. A real task suite, adequate sample size, quality non-inferiority, cache state control, confidence intervals and human review are still necessary before an economic claim.

Provider usage normalization follows the [OpenAI Responses reference](https://developers.openai.com/api/reference/cli/resources/responses/methods/create) and [Anthropic prompt caching documentation](https://platform.claude.com/docs/en/build-with-claude/prompt-caching). OpenAI response creation time is also checked against the captured run interval.

The three small fixtures are protocol scenarios: a targeted read, a full-file invariant review, and repeated partial reads. They are not a representative benchmark and must not be multiplied into a fake 60-task suite.

Run offline validation:

```sh
python3 -m unittest discover -s benchmarks/hzr-task-economics-v0.8.0 -p 'test_*.py'
```
