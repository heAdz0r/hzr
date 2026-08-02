# HZR SoTA release gates

HZR may call a release state-of-the-art only when functional correctness, retrieval utility,
provider economics, and distribution evidence all pass. Local token estimates alone are not a
quality or cost result.

## Deterministic gate

Every release candidate must pass the repository and immutable fork-core suites plus these
behavioral invariants:

- high and irreversible codec requests return input content unchanged;
- context selection stays inside the input budget after output reserve and safety margin;
- calibrated relevance preserves a large engine-score gap, weak relative candidates are rejected,
  memory cannot consume the code budget, and unlocatable artifacts are demoted;
- symbol-shaped intents add exact evidence, long memory is bounded, and protected technical spans
  remain exact;
- memory update, forget, and prune cannot select another project or the wrong global namespace,
  and threshold pruning never selects high or critical memories;
- public observatory routes expose no memory bodies, while full details require bearer auth;
- MCP cancellation stops in-flight work without a late response, and failed usage receipts survive
  in the managed-agent outbox until acknowledged.

The Rust, Node bridge, visualizer, installer, fork parity, MSRV, formatting, and clippy gates remain
mandatory. A passing deterministic gate proves implementation contracts, not model task quality.

## Provider-paired gate

The provider-backed gate compares baseline and HZR arms with the same repository snapshot, task,
model, model configuration, maximum turns, and trial count. Record each trial rather than only an
aggregate:

- accepted or rejected task outcome using a task-specific executable oracle;
- actual provider input, output, reasoning, cache-write, and cache-read tokens when supplied;
- retries, turns, latency, and provider-reported cost;
- HZR route coverage and any unaccounted operation;
- source revision, model identifier, date, trial count, and harness hash.

Report quality and economics together. HZR passes only if accepted-task rate is non-inferior and the
provider-reported economic result improves at the declared confidence level. Estimated counters stay
separate and cannot fill a missing provider field.

## Distribution gate

GA additionally requires signed native artifacts and clean-install smoke tests for every advertised
platform, an SBOM and license scan for each bundle, immutable engine provenance, checksum and
attestation verification, and an inspected CI run for the exact release commit. Windows remains
unadvertised until a native artifact and isolated installation/upgrade smoke test exist.
