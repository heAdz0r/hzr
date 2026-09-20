# Benchmark methodology

The README graphic reports one local field benchmark captured on September 19,
2026. It is evidence from a real build, not a guarantee that another repository
will produce the same savings.

## Results

| Workflow | Implementation and test lines | Astra input | Flash input | API-equivalent compute per 1K lines |
| --- | ---: | ---: | ---: | ---: |
| All Astra | 34,425 | 294,531,195 | — | $11.32 |
| Original orchestration | 41,255 | 124,784,625 | 878,386,985 | $3.88–$3.99 |
| Thin orchestration | 47,949+ | 4,600,297 | 1,001,537,994 | $0.26–$0.34 |

Compared with the all-Astra phase, thin orchestration used **98.9% less Astra
input per 1,000 implementation lines** and had **97.0–97.7% lower total
API-equivalent compute cost per 1,000 lines**. It produced 39% more measured
implementation and test lines in the observed phase.

## Measurement

- Token usage came from exact local Codex session records, grouped by root
  session and worker descendants.
- Product source and test lines came from Git and restorable repository
  baselines. Generated files, migrations, documentation and evidence artifacts
  were excluded.
- Thin-phase line count is a conservative floor because baseline contents for
  some modified files were unavailable.
- DeepSeek V4.1 Flash cost uses the published cache-hit, cache-miss and output
  rates, shown as off-peak to peak.
- Astra cost is an API-equivalent estimator used consistently across all three
  phases. Astra does not have a public API SKU, so these values are not Codex or
  ChatGPT subscription charges.

## Pricing used

| Cost per 1M tokens | Astra estimator | DeepSeek V4.1 Flash | Astra premium |
| --- | ---: | ---: | ---: |
| Uncached input | $10.00 | $0.15–$0.30 | 33–67× |
| Cached input | $1.00 | $0.003–$0.006 | 167–333× |
| Output | $50.00 | $0.60–$1.20 | 42–83× |

DeepSeek prices are sourced from its [official pricing
page](https://api-docs.deepseek.com/quick_start/pricing/). Provider prices and
Codex product behavior may change after this measurement.

## Limits

The phases had different task mixes. The all-Astra phase included research,
browser, deployment and account work that produces little or no source code.
Some thin-phase packets were submitted rather than finally accepted at the
measurement cutoff. Lines of code are an imperfect work measure, but they are
more informative here than elapsed time, task counts or prompts alone.
