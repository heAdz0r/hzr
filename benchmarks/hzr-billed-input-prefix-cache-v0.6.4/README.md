# Legacy filter-placement probe (v0.6.4)

This runner executes local CLI commands under two filter-placement settings. It does not launch LLM tasks and has no per-case provider request channel. Its economic comparison therefore always reports `not_measured` and exits 2.

The original implementation reused cumulative `observed_model_usage` from `hzr stats`, incorrectly assigning earlier usage to later cases. Aggregate usage is now rejected, command exit codes are retained in failure reasons, and delivered ledger deltas are labeled as repetition totals rather than medians.

Filtering a newly appended result does not itself alter an existing cached request prefix. Neither a lower fresh-input total nor a higher local-output total establishes cache invalidation causality. Such a claim requires actual request-prefix evidence.

For correlated agent-task evidence, use the [0.8.0 task economics harness](../hzr-task-economics-v0.8.0/README.md). It requires immutable run/task bindings, actual per-request provider usage, and an independent acceptance evaluator. No provider-billed savings are asserted by either harness without evidence.

The legacy local-output probe remains runnable for diagnostic compatibility:

```sh
python3 benchmarks/hzr-billed-input-prefix-cache-v0.6.4/benchmark.py \
  --repo-root . \
  --hzr-binary ./target/release/hzr \
  --work-root /tmp/hzr-local-placement-probe \
  --output /tmp/hzr-local-placement-probe/result.json
```

It changes only its explicitly selected probe config. Use an isolated fixture and work directory. It does not silently turn existing aggregate provider usage into a measured result.
