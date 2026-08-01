# HZR LLM utility result

| Metric | Result |
|---|---:|
| Contract gates | **9/9 PASS** |
| Read clarity signals | **6/6** |
| Single write operations | **4/4** |
| Batch operations | **4/4** |
| Exact read SHA-256 parity | **PASS** |
| Identical create preserves content + mtime | **PASS** |

| Gate | Status |
|---|---|
| `read_contract_6_of_6` | PASS |
| `read_exact_sha256_match` | PASS |
| `read_range_exact` | PASS |
| `single_write_4_of_4` | PASS |
| `create_idempotent_content_and_mtime` | PASS |
| `batch_write_4_of_4` | PASS |
| `batch_dry_run_unchanged` | PASS |
| `write_json_schema_v1` | PASS |
| `write_is_hzr_only_vs_upstream_v0_44_1` | PASS |

This verifies deterministic output and mutation contracts, not semantic comprehension by a
particular LLM or accepted-task quality. Batch atomicity is per file, not across the whole plan.
