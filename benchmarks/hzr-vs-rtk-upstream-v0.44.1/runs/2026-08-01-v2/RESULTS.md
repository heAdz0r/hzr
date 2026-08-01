# Recorded result: 2026-08-01

Five repetitions per case, rotating order. Tokens are estimated as 
`ceil(UTF-8 bytes / 4)` and are not provider-billed tokens.

| Aggregate | RAW | Upstream RTK | HZR | HZR vs RAW | HZR vs upstream |
|---|---:|---:|---:|---:|---:|
| 14 cases | 284,996 | 58,107 | **44,400** | **−84.4%** | **−23.6%** |

| Case | RAW | Upstream | HZR | HZR vs upstream |
|---|---:|---:|---:|---:|
| `read README.md` | 6,046 | 6,046 | 265 | −95.6% |
| `read src/main.rs` | 30,457 | 30,457 | 29,280 | −3.9% |
| `read src/core/filter.rs` | 4,471 | 4,471 | 3,298 | −26.2% |
| `read Cargo.toml` | 426 | 426 | 426 | parity |
| `ls src` | 16 | 20 | 20 | parity |
| `grep -rn "fn run" src` | 5,175 | 4,435 | 4,435 | parity |
| `find . -name "*.rs" -type f` | 847 | 184 | 184 | parity |
| `git status` | 16 | 14 | 6 | −57.1% |
| `git log -30` | 2,881 | 1,276 | 593 | −53.5% |
| `git diff HEAD~5` | 185,931 | 10,325 | 5,540 | −46.3% |
| `git show HEAD` | 76 | 76 | 76 | parity |
| `git branch -a` | 1,561 | 100 | 100 | parity |
| `cargo check` | 18 | 25 | 9 | −64.0% |
| `cargo test` | 47,075 | 252 | 168 | −33.3% |

Inspect [`summary.json`](summary.json) for every repetition and [`outputs/`](outputs) for canonical full output. Verify all artifacts with:

```bash
shasum -a 256 -c checksums.sha256
```
