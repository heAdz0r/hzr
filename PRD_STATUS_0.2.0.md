# HZR 0.2.0 - current implementation status and rechecking of fixes

**Date of recheck:** 2026-08-01
**Base:** [PRD.md](PRD.md) 0.2.0 · [PRD_ADOPTION.md](PRD_ADOPTION.md) §16 · [FORK_PARITY.md](FORK_PARITY.md)
**Previous audit:** [PRD_STATUS.md](PRD_STATUS.md) and [REVIEW.md](REVIEW.md) - historical `v0.1.0` audit trail
**Method:** re-running **the same repro cases** that G1–G7 gave, end-to-end checking the adoption surface in an isolated `HOME` and auditing the self-contained release pipeline

> Source/release gates were executed in throwaway `HOME`, then published by Darwin ARM64
> artifact is installed globally. Repeated live audit confirmed bundle equality, active
> launchd service, no foreign engine owners and `hzr doctor --json` without errors.

---

## 1. Verdict

**G1–G7 are closed - all seven. Confirmed by launch, without reading diffs.** Not a single “cosmetic” fix: two of them were fixed more completely than I suggested.

**Adoption surface (§16) is fully implemented:** one dispatcher, durable PATH entry,
single HZR instruction contract for Claude/Codex, HZR-owned MCP registrations and production
user service. Legacy ICM memory and platform RTK history have separate snapshot-first,
manifested, idempotent migrations.

**Self-contained distribution published:** [`v0.2.0`](https://github.com/heAdz0r/hzr/releases/tag/v0.2.0) installs one versioned platform bundle with full fork-core, patched grepai, ICM, exact caveman-code production tree and bundled Node.js 22.17.1. Same-version roots undergo repeated full certification; production `hzrd` is managed by launchd/systemd user service. [Release workflow 30676586828](https://github.com/heAdz0r/hzr/actions/runs/30676586828) confirmed native package/install/upgrade smoke for `darwin-arm64`, `darwin-x64`, `linux-arm64` and `linux-x64` before publishing artifacts.

|Region|Was (0.1.0)|Now (0.2.0)|
|---|---|---|
| G1 codec `shadow` |❌ inverted|✅ completely fixed|
| G2 protected spans |🟡 4 classes not covered|✅ all 4 are covered|
| G3 fork regression gate |🟡 non-deterministic|✅ deterministic 4/4|
| G4 codec overpromise | 🟡 |✅ claims now match the implementation; trailing newline preserved|
| G5 bridge invariants |🟡 text search|✅ runtime test in CI|
|G6 duplicate error| 🟡 |✅ fixed|
|G7 lock file rights| 🟡 `0644` |✅ all `0600`|
| Adoption / hooks / instructions (§16) |⚪ not implemented|✅ fully implemented|
| Self-contained release installer |⚪ was absent| ✅ public `v0.2.0`; native 4-platform matrix green |
| Economic KPI (§4.2) | 📊 0/9 |📊 **0/9 - no changes**|

---

## 2. Recheck G1–G7

### G1 — codec `shadow` ✅ completely fixed

Same input as the original violation (3 paragraphs, 1st and 3rd identical):

```json
{
  "content": "The budget is exhausted.\n\nSecond distinct paragraph here.\n\nThe budget is exhausted.\n",
  "changed": false,
  "counterfactual": { "input_bytes": 84, "output_bytes": 58, "saved_bytes": 26, "would_change": true }
}
```

Both requirements PRD §6.6 are now completed simultaneously:

- **content not changed** - byte-for-byte equal to the input, including the trailing `\n`;
- **counterfactual size recorded** — previously absent (0 matches for `counterfactual|shadow_size`).

The fix in [hzr-codec/src/lib.rs:116](crates/hzr-codec/src/lib.rs#L116) is **stronger** than my recommendation: `Shadow` short-circuits *first*, before checking `Exact|Off`, and computes `candidate_transform` only for measurement without returning it. I proposed one short-circuit line; the implementation provides a complete `CounterfactualSize`.

### G2 - protected spans ✅ fixed

Same input as previously given by `protected_spans: []`:

|Fragment|Earlier|Now|
|---|---|---|
| `src/main.rs` |not covered| ✅ `path` (5–16) |
| `MAX_RETRIES` |not covered| ✅ `enum` (25–36) |
| `handle_budget_overflow` |not covered| ✅ `identifier` (41–63) |
| `{"k":1}` |not covered| ✅ `structured` (69–76) |

The regex is expanded with relative paths (`(?:[\w.@%+~-]+/)+[\w.@%+~-]+`), snake/UPPER_SNAKE identifiers, `{...}` and `[...]`. Added taxonomy `kind`, which was not there before.

**Accepted boundary:** identifiers are recognized by the presence of `_`. A standalone CamelCase identifier (`MyStruct`) and a single word such as `budget` are still not protected. This is a deliberate constraint, not a current defect: the paragraph-level transformation cannot exploit it.

### G3 — fork regression gate ✅ deterministic

The reason is essentially fixed: [tracking.rs:1365](fork-core/rtk/src/tracking.rs#L1365) now creates `tempfile::tempdir()` + `Tracker::open(isolated)` + `track_with(&tracker, …)` instead of writing to the shared DB and reading the window `get_recent(5)`.

```
tracking::tests, parallel ×5           → ok 11 passed  (previously FAILED 3/3)
scripts/verify-fork-core.sh --test ×4  → exit 0, 0, 0, 0  (previously flaky)
```

**Important clarification:** running `cargo test` **directly inside** `fork-core/rtk` still fails, but in a different test: `memory_layer::git_churn::tests::test_load_churn_real_repo`. It requires a real `.git` in `CARGO_MANIFEST_DIR`, while snapshot rule §1.1.4 prohibits nested history; the `fork-core/rtk` snapshot intentionally has no nested `.git`. The official gate creates synthetic Git history in a temporary copy, so it passes. This documented `FORK_PARITY.md` mechanism is not a regression. The supported command is `scripts/verify-fork-core.sh --test`; direct `cargo test` is unsupported in the snapshot directory.

### G4 - codec claims ✅ aligned with the implementation

PRD §14 now says "protected **paragraph** codec with shadow counterfactual" instead of the vague "adaptive codec". This is exactly what the code does. The trailing `\n` is no longer removed, as demonstrated by G1. The transformation remains paragraph deduplication, but the documentation no longer overpromises.

### G5 - bridge invariants ✅ replaced by runtime test

[bridge.test.mjs](integrations/caveman-code/bridge.test.mjs) appeared - a real JS test with mock objects instead of `BRIDGE.contains(...)`:

```
✔ bridge import is side-effect free
✔ production preparation owns tools and disables duplicate subsystems in order
```

The test checks **actual post-execution state**, not whether a line exists in the source: `prepared.session._repomapEnabled === false`, `prepared.session.memoryEnabled === false`, `getRtkEnabled() === false`, `getDisableAllHooks() === true`, `sessionOptions.tools === []`, empty extensions/skills/prompts/agentsFiles, and the required environment variables. Ordering is proven separately by `assert.deepEqual(order, ["/v1/health", "/v1/context/plan", "create_session"])`: health and context planning occur **before** session creation.

Included in CI: `node --test integrations/caveman-code/bridge.test.mjs` ([ci.yml:68](.github/workflows/ci.yml#L68)).

This satisfies G5. The residual limitation is explicit: the test uses fakes and therefore verifies the bridge contract, not the real SDK's behavior.

### G6 - error message ✅ fixed

```
hzr: failed to read daemon token <path>; run `hzr daemon serve`: No such file or directory (os error 2)
```

The reason is printed once (was twice).

### G7 - file permissions ✅ fixed

`hzrd.token`, `hzrd.lock`, `hzrd.token.lock`, `config.toml` - all `-rw-------` (`0600`). Previously `hzrd.token.lock` was `0644`.

---

## 3. Quality gates 0.2.0

| Gate |Result|
|---|---|
| `cargo fmt --all --check` | ✅ exit 0 |
| `cargo clippy --workspace --all-targets --all-features -D warnings` | ✅ exit 0 |
| `cargo test --workspace --all-targets --all-features` |✅ **224 passed, 0 failed** (fresh run)|
| `rustup run 1.85.0 cargo check` (MSRV) | ✅ green |
| `node --test bridge.test.mjs` | ✅ 2/2 |
| `scripts/verify-fork-core.sh --test` |✅ baseline `f4296ec4…` + current engine `a8512845…`; 1699 passed, 1 documented ignored, remaining suites green|
|`FORK_PARITY.md` without `missing`/`reimplemented`|✅ (the only occurrence is legend)|
| assembled bundle smoke |✅ versions, ownership, provenance, daemon and bundled Node 22.17.1 are covered|
| outer `package-release.sh` + `smoke-install.sh` |✅ clean runtime, reinstall, tamper/missing/symlink rejection and upgrade green for 4 native runners|

**Notable 0.2.0 architecture decision:** the verifier now maintains **dual identity**: an immutable `v0.1.0` baseline (`f4296ec4…`) and an evolvable current engine (`a8512845…`). This permits fork-core development without losing provenance: §1.1 no longer treats the snapshot as frozen forever, while the original import remains verifiable.

---

## 4. New adoption surface (§16) - tested end-to-end

CLI surface from PRD §6.8 implemented; production service lifecycle and
explicit `migrate history|memory` with preview/confirmation contract.

|Requirement §16 / §13|Examination|Status|
|---|---|---|
|§16.2 Exactly one interceptor; RTK-records **replaced**|3 rtk records → `rtk_entries: 0`, `hzr_entries: 2`| ✅ |
|§16.2 Coexistence = `error`, no warning| `ERROR hook_ownership: HZR=2 RTK=1; exactly two HZR handlers and zero RTK handlers are allowed`, exit 1 | ✅ |
|§16.3 **Managed and degraded give the same verdict**|dispatcher output when alive and dead `hzrd` - **byte-identical**, exit 0 in both| ✅ |
|§16.3 Shell doesn't break without a daemon|degraded branch responds with a valid hook-JSON, not an error| ✅ |
|§16.3 Degradation visible|`hzr stats`: `degraded rewrites: 1` + incomplete accounting for daemon-free rewrite; `doctor`: `WARN degraded_rewrites`| ✅ |
|§16.3 Fallback in pinned fork, not stock RTK|the same `0.44.1-fork.1` adapter| ✅ |
| §16.4 `init --if-needed` — filesystem no-op |recursive SHA-256 data root before/after **identical**| ✅ |
|§16.5 `--dry-run` does not write|SHA settings.json before/after is identical; gives `before_sha256`/`after_sha256`/`backup_path`| ✅ |
|§16.5 Confirmation required|without `--force`: "inspect `hzr install --dry-run`, then rerun with `--force`"| ✅ |
| §16.5 Idempotent |2nd `install --force` → `changed: false`, the second backup is not created| ✅ |
|§16.5 Full-SHA backup (pattern §11)| `settings.json.hzr-backup-<full-sha256>` | ✅ |
| §16.5 Centralized ICM ownership |known external `icm hook` entries are removed by default; `--keep-external-icm` leaves them explicitly| ✅ |
|§16.5 Unknown handlers are saved|records not belonging to HZR/RTK/ICM are not overwritten| ✅ |
| §6.8 matcher `Bash\|Agent\|Task`, one handler|confirmed in the installed config| ✅ |
| §6.8 `SessionStart` → `init --if-needed --quiet` |confirmed| ✅ |
|§6.8 CAS under filesystem lock|`settings.json.hzr.lock` present| ✅ |
|§16.5 `uninstall` does not restore RTK silently|HZR-records were removed, the attached rtk-record and ICM remained as they were| ✅ |
|Rewrite actually works via HZR| `cat main.rs` → `permissionDecision: "ask"` + `updatedInput: … rtk read main.rs` | ✅ |

The actual verdict of the dispatcher is a valid Claude hook protocol: `hookSpecificOutput.permissionDecision` with exit 0, that is, fork exits `0/1/2/3` are converted into a typed decision, and do not flow outward as a process exit. This corresponds to §6.8.

### 4.1 Agent instructions - A1 closed

`hzr install` is now one confirmed operation:

- atomically places durable `hzr`/`hzrd` in PATH prefix;
- installs one managed block in Claude `CLAUDE.md` and Codex `AGENTS.md`;
- refers to the canonical bundled [HZR.md](HZR.md), rather than copying the divergent prompt;
- removes only machine-owned legacy `@RTK.md` imports and saves custom text;
- applies the same `--dry-run` → `--force`, full-SHA backup, filesystem lock and compare-and-swap contract as the settings installer;
- deletes managed blocks at `hzr uninstall`, without restoring RTK silently.

Thus, the agent receives not only shell interception, but also an explicit contract to use HZR as the only control plane for read/search/write/memory/context.

### 4.2 Self-contained release distribution

The release pipeline now includes four separate levels:

1. `build-bundle.sh` builds HZR, full fork-core, patched grepai, patched ICM, exact caveman-code production dependencies and checksum-pinned Node.js 22.17.1;
2. `smoke-bundle.sh` checks versions, engine ownership, current/baseline provenance, bundled Node, daemon auth/singleton and compatibility alias;
3. `package-release.sh` creates a platform archive and an internal `BUNDLE_MANIFEST.sha256`;
4. `install.sh` checks release `SHA256SUMS` and manifest, sets a separate `versions/v0.2.0-<platform>` root and atomically switches `current`; `smoke-install.sh` proves clean install with stripped down `PATH` without external Node/RTK/grepai/ICM.

Artifact mappings are implemented for macOS/Linux arm64/x64. The release workflow assembled each artifact on its native runner and ran the same outer package/install smoke before publication. The matrix is fully green in [run 30676586828](https://github.com/heAdz0r/hzr/actions/runs/30676586828); the release contains four platform archives, one `SHA256SUMS` file and GitHub build-provenance attestations. No Windows artifact is provided in 0.2.0.

### 4.3 Final live audit of published `v0.2.0` — 2026-08-01

- release tag points to commit `551e91f2445cd51f52ff22052cc1d5592fd96475`;
source CI [30675757188](https://github.com/heAdz0r/hzr/actions/runs/30675757188) and native
  release matrix [30676586828](https://github.com/heAdz0r/hzr/actions/runs/30676586828) — green;
- the published `darwin-arm64` artifact is installed in `~/.local/share/hzr/current`;
Claude/Codex hooks, instructions and MCP registrations use only stable
`~/.local/bin/hzr` and `~/.local/share/hzr/current/...`, without `versions/`/`target/` paths;
- launchd service is active and uses only stable `current/bin/hzrd`;
- bundle attestation: `hzr`, `hzrd`, RTK, grepai, ICM, Node, Caveman bridge and `HZR.md`
- all `pass`; installed RTK SHA matches current-engine release binary;
- `hzr doctor --json`: `healthy=true`, `errors=[]`; only explicit warnings remain for
FTS-only memory, 59 historical daemon-free rewrites and two host-global codec paths,
marked `unintercepted` without accrual of savings;
- legacy ICM: 141 memories / 148 durable rows imported into repository namespace;
legacy RTK source: 23,228 operations + 725 parse failures; final delta-import added
246 previously missing commands, after which repeated run became no-op;
both repeated migration runs returned zero imports and `changed=false`;
- process audit after production restart: exactly one `hzrd`, one HZR-owned `icm serve`
and one managed `grepai watch`; doctor does not find foreign owners, wrappers or duplicate indexes;
- centralized memory contains critical handoff `hzr-release-0-2-0` for the following LOOP agents;
- cumulative `hzr stats`: 24 156 operations, 188 782 166 estimated net avoided tokens
(`89.6355%`); provider-observed tasks/accepted remain `0/0`, so
`economic_claim_ready=false` and cost reduction are not stated as measured results.

---

## 5. Registry of current gaps

| ID | Section | Severity | Summary |
|---|---|---|---|
| **A3** | §6.6 / RB-08 | ACCEPTED LIMIT | Claude/Codex do not provide a secure global response hook. Doctor marks the path `unintercepted`, the managed codec remains at `hzr agent`, and savings for host-global responses are not calculated |
| **A4** | §11 / §16.5 | LOW | Content-addressed backups of modified `settings.json`, `CLAUDE.md` and `AGENTS.md` are retained; automatic deletion is correctly disabled, but a long-term rotation policy is not defined |
| **A5** | §6.6 | LOW | Identifiers are protected only when `_` is present; CamelCase and single-word identifiers are not. This remains safe with the paragraph-level transform |
| **KPI** | §4.2 |**OPEN**|**0 out of 9 product metrics measured - no change from 0.1.0**|

R1 is closed by the public native matrix. A3/A4/A5 are documented constraints,
and the KPI remains unmeasured; none permits publishing forecast savings as an achieved result.

---

## 6. What's next

`v0.2.0` is published and globally accepted as an active bundle. The next release LOOP starts with
paired provider benchmark, extension MCP schema/accounting coverage and available only through
official host hooks request/response interception; exact/shadow safety and double ban
accounting remain non-negotiable invariants.

The main open product question has not changed: §4.2 remains 0/9. The functional prerequisites are ready, and `hzr stats` honestly separates actual usage, estimated local effect and incomplete accounting. You can convert target −30% cost / −20% turns / −35% uncached input from a hypothesis into a result only after paired benchmark from §14.
