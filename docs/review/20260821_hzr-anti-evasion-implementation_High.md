# High review: HZR 0.4.4 anti-evasion implementation

Date: 2026-08-21
Probed version: HZR 0.4.4 (fork-core rtk 0.44.1-fork.1), installed bundle
Scope: the implementation of `docs/PRD_HZR_ANTI_EVASION_SOTA.md`
Method: 41-case live probe through `hzr exec rewrite` and through the real `hzr hooks dispatch`
PreToolUse path; the committed 85-case matrix; repository quality gates.

## Verdict

The implementation is substantially complete and closes every hole the 0.4.3 audit proved. All
41 probe cases resolve to a managed route or an explicit `ask`; none reach silent raw proxy.
Quality gates pass clean. Two defects remain, both in the same place: the `ask` decision path
carries no guidance, and the acceptance matrix cannot detect that it does not.

## Quality gates

| Gate | Result |
|---|---|
| `cargo fmt --all --check` | PASS |
| `cargo clippy --all-targets --all-features -- -D warnings` | PASS, clean |
| `cargo test --all-targets --all-features` | PASS — 20 suites, 478 passed, 0 failed, 1 ignored |
| Committed acceptance matrix | 85 unique cases, no duplicate ids |

Matrix composition: 42 rewrite, 25 ask, 9 allow, 6 deny, 2 proxy, 1 raw; 70 shell and 15 native
surfaces; 65 fork-layer and 20 root-layer. The fixture is included by `#[path]` into
`crates/hzr-cli/src/hook_runner.rs` and `crates/hzr-daemon/src/api.rs`, so one fixture genuinely
gates the fork, hook and daemon paths as required.

## Re-probe against 0.4.4

The 0.4.3 audit found 20 of 41 cases reaching silent raw proxy. Against 0.4.4:

| Outcome | Cases |
|---|---:|
| Optimized managed route | 30 |
| HZR policy rewrite | 1 |
| Explicit `ask` | 10 |
| Silent raw proxy | **0** |

Constructs that were bypasses in 0.4.3 and are now normalized:

| Construct | 0.4.3 | 0.4.4 |
|---|---|---|
| `nl -ba 'README.md'` | raw proxy | `rtk read 'README.md' -n` |
| `sed -n '10,20p' README.md` | raw proxy | `rtk read README.md --from 10 --to 20` |
| `cat README.md \| head -50` | raw proxy | `rtk read README.md --max-lines 50` |
| `/bin/sh -c 'git blame README.md'` | raw proxy | `/bin/sh -c 'rtk git blame README.md'` |
| `sh -lc 'cat README.md'` | raw proxy | `sh -lc 'rtk read README.md'` |
| `bash -c 'nl -ba README.md'` | raw proxy | `bash -c 'rtk read README.md -n'` |
| `zsh -lc 'sed -n 1,50p README.md'` | raw proxy | `zsh -lc 'rtk read README.md --from 1 --to 50'` |
| `/bin/cat README.md` | raw proxy | `rtk read README.md` |
| `git blame README.md` | raw proxy | `rtk git blame README.md` |
| `sqlite3 /tmp/x.db 'select 1'` | raw proxy | `rtk sqlite3 … --max-rows 50 --max-tokens 2048` |
| `ssh host docker logs foo` | raw proxy | `rtk logs host foo --tail 100` |

Native-tool enforcement is live and is the best-executed part of the release. A host `Read` is
refused with:

```
T1 native-tool correction E8 (Read); use `hzr read '<path>'`; session avoidable-bypass count=3
```

Class, ready-to-run replacement and running session count, in one line. This is the reference
pattern for every other enforcement message.

`hzr doctor` reports `native_tool_mode: steer` and
`fidelity_allowance: 5 operations or 100000 delivered tokens`. `hzr stats --evasion` renders
bounded per-class aggregates and keeps policy attempts separate from executed operations.
Unbounded `hzr stats --json --all` is refused with its bounded alternative named.

## Findings

### F1 — `ask` decisions carry no class, no replacement and no count (High)

Every `ask` outcome, on both the CLI and the real hook path, is:

```
permissionDecision: ask
permissionDecisionReason: fork-core canonical policy requires approval
proposed: none
```

Verified for `python3 -c 'print(open("README.md").read())'`, `awk '{print}' README.md`,
`find . -name '*.rs' -exec cat {} +`, `tar cf - README.md`, and
`/usr/bin/sqlite3 -readonly -json …`. This affects 25 of 85 matrix cases and 10 of 41 probe cases.

The reason is generic where the classifier already holds specific knowledge. `python-read`,
`node-read`, `ruby-read` and `perl-read` are recognized *as read idioms* — that recognition is
why they Ask instead of proxying — yet the equivalent `hzr read <file>` is never offered.

This contradicts the design constraint the rest of the release follows, and which the native path
implements correctly: a refusal without a ready-to-run alternative produces a retry loop, and a
retry loop costs more than the bypass it replaced. It also converts an agent-facing correction
into a human-facing permission prompt, which is where prompt fatigue comes from.

Fix: reuse the native path's message shape for shell `ask` decisions — class, concrete
replacement when one is known, and session count. Where no replacement is known, say which
construct was ambiguous rather than naming the policy engine.

### F2 — The acceptance matrix cannot detect F1 (High)

`fork-core/rtk/tests/fixtures/anti_evasion_policy.json` fields are `id`, `layer`, `surface`,
`command`, `decision`, `route`, `tool`, `tool_input`, `mode`, `class`, `avoidable`. There is no
field asserting the reason text, the offered replacement, or the reported class.

So all 25 `ask` cases pass while conveying nothing actionable. The gate tests the verdict and not
the guidance — and guidance is the mechanism by which enforcement is supposed to change agent
behavior. A matrix that cannot fail on an empty prescription cannot protect the property the PRD
called non-negotiable.

Fix: add optional `expect_class`, `expect_replacement` and `expect_reason_contains` assertions,
and make them required for every `ask` and `deny` case.

### F3 — `bypass.by_tool` emits duplicate keys and non-tool labels (Medium)

`hzr stats --json --since 7d` returns:

```
tool='other'  exec=39  delivered=8814
tool='rust'   exec=6   delivered=0
tool='search' exec=23  delivered=0
tool='search' exec=55  delivered=0
```

Two distinct rows share the key `search`, so the aggregate is not keyed by the field it displays.
`rust` and `other` are not tool names. The human view then renders, for `search`,
"no first-class equivalent; raw is correct here" — which is false for the one family that most
obviously has a first-class route.

Fix: group by the same identity that is displayed, and derive the advice line from the
classifier rather than from a default applied to an unrecognized label.

### F4 — WITHDRAWN: upgrade behavior matches its documentation

This was reported as a defect on the strength of one observation — this machine upgraded
0.4.3 → 0.4.4 and reports `native_tool_mode: steer` while `CHANGELOG.md` promises that "legacy
upgrades retain `observe`". Reading the code before changing it showed the diagnosis was wrong.

`installed_native_tool_mode` treats a pre-enforcement dispatcher — an installed `hooks dispatch`
command with no `--native-mode` flag — as an existing installation and returns `Observe`, and
`acceptance_gate_upgrade_observes_while_new_install_steers` asserts exactly that. The gate was
verified to pass against the unmodified tree, so the documented rule is the implemented rule.

The `steer` on this host therefore reflects how this installation was performed, not a policy
regression: a settings file whose HZR hooks already carry `--native-mode`, or an explicit
`--native-tool-mode`, is a current installation rather than a legacy one.

What survives is smaller and is not a defect: `hzr doctor` reports the effective mode but not how
it was chosen, so an operator cannot distinguish "upgraded and preserved" from "installed fresh".
Recording that provenance would make the same question answerable without reading the settings
file.

## Measurement integrity

`hzr stats --since 7d` now reports 1,718 operations against the 10,239 measured for the same
nominal window before the upgrade — 16.8% of the earlier population, because 0.4.4 excludes
unattributed legacy rows from current-policy reporting. Excluding them is the correct decision;
mixing incompatible accounting models would be worse.

The consequence must be stated wherever the improvement is quoted: the move from 50.3% to 6.9%
bypassed tokens is a change of denominator, not a measured effect. `docs/releases/v0.4.4.md`
handles this correctly — it uses a fresh 0.4.3 window as its baseline and states that the 2% and
60% targets "remain a measured seven-day post-release acceptance window, not a release-time
savings claim". That discipline should survive into any later summary of the release.

## Resolution — HZR 0.4.5

F1, F2 and F3 are fixed; F4 is withdrawn as a misdiagnosis.

| Finding | Resolution |
|---|---|
| F1 | `EvasionClass::construct()` and `EvasionClass::prescription()` give the closed taxonomy one authority for agent-facing wording. `policy_reason` in the adapter builds every Ask and Deny reason from the attribution the plan already carried, and `attach_policy_feedback` appends the running session count to Ask and Deny as well as corrections. A command with no attribution keeps the original wording, because a prescription invented for an unclassified construct would be a guess presented as guidance. |
| F2 | The fixture gains `expect_reason_contains`, required for every Ask and Deny case; the loader fails on an empty expectation. All 31 cases were populated from ground truth captured through `rtk rewrite-plan`, and a gate asserts each expectation matches a prescription the taxonomy actually emits, so wording drift breaks the test instead of passing silently. |
| F3 | `bypass_report` merges on the privacy-safe label it displays instead of the ledger identity, sums the merged traffic, and re-sorts so the costliest leak stays first. The advice column was the second half: the per-row flag records whether a replacement could be reconstructed from one recorded invocation, not whether the family has a route, so a bypassed `search` was told it had no managed equivalent. HZR's own subsystems — `read`, `search`, `write`, `memory`, `codec` — now report a route by construction. |
| F4 | Withdrawn. See above. |

An E10 Ask still carries `tier=t2_deny_with_prescription`, which reads as an enforcement action
against a command that has no managed route. The behavior is correct — `avoidable` is false and
the budget is untouched — but the label is not. Correcting it means either a new
`EnforcementTier` variant or reusing `t0`, and `EnforcementTier` is persisted in the ledger, so
this is a schema decision rather than a hotfix. It is recorded here as the next item.

## Recommendation

Ship 0.4.5 before the seven-day acceptance window opens, so the window measures the enforcement
design as intended rather than a version of it that could not tell agents what to do instead.
