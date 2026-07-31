# Third-party notices

HZR 0.1.0 preserves the complete heAdz0r RTK fork as its runtime execution core and composes
additional pinned engines without removing their provenance.

| Component | Version | License | Source | Role |
|---|---:|---|---|---|
| heAdz0r RTK fork-core | 0.44.1-fork.1 | MIT | https://github.com/heAdz0r/rtk | runtime, immutable worktree snapshot |
| upstream RTK | 0.44.1 | Apache-2.0 | https://github.com/rtk-ai/rtk | provenance/reference pin only; never built as HZR runtime |
| ICM | 0.10.61 | Apache-2.0 | https://github.com/rtk-ai/icm | lockfile-corrected runtime |
| grepai | 0.35.0 | MIT | https://github.com/yoanbernabeu/grepai | patched runtime |
| Caveman design-derived codec | 1.9.1 reference | MIT | https://github.com/JuliusBrussee/caveman | design reference |
| caveman-code managed SDK | 0.65.2 | MIT | https://github.com/JuliusBrussee/caveman-code | managed runtime |

The RTK runtime is the exact 516-entry snapshot recorded in `fork-core/SNAPSHOT.toml`. Its canonical
v2 SHA-256 is `f4296ec404f461d6fc03c966c0dc79caee6c3118a73d1ed1a078ded5529f0a16`;
the preserved v1 content-manifest SHA-256 is
`072a62adc754b728ec99a507d2c1a223d83077d067a9249a26d357eec890b4cc`. The v2 identity covers
entry paths, types, Git-portable modes, sizes and content digests, ordered deletions, source identity
and dirty-state hashes, plus explicit exclusion rules and reasons. It was captured from `heAdz0r/rtk`
at HEAD `5f403c465cbdbe148e9ca03e0ac8e856eef0bfee` together with its recorded working-tree state. Its MIT
license remains at `fork-core/rtk/LICENSE` and is copied into every HZR bundle. HZR verifies this
snapshot before compilation and does not fetch or substitute stock RTK.

Exact source tags, commits, snapshot identity and package integrity are recorded in
`engines.lock.toml`. Distribution bundles must retain this notice and the applicable full license
texts. HZR bundles carry the full pinned runtime license texts as `grepai-MIT.txt`,
`ICM-Apache-2.0.txt`, `rtk-fork-core-MIT.txt`, and `caveman-code-MIT.txt`; licenses already present
inside Caveman's production `node_modules` remain alongside their packages. Caveman 1.9.1 is a
design reference only and its source is not included in the runtime bundle.

HZR applies `patches/grepai/0.35.0-disable-worktree-discovery.patch` to the pinned grepai source.
The patch adds an opt-out for upstream watcher's automatic linked-worktree discovery so HZR remains
the sole index owner. Modified-source distributions must retain grepai's MIT license and identify
this change. Patch SHA-256:
`55535352bc9f4837198c652b8c44ec54a0a7ef82fbd81e11b4ec11f4c4082991`.

HZR applies `patches/icm/0.10.61-refresh-workspace-lock.patch` to the pinned ICM source. The patch
only brings the `icm-cli` package version in upstream's committed `Cargo.lock` from `0.10.54` to the
source package version `0.10.61`, allowing the otherwise unchanged source to build under `--locked`.
Patch SHA-256: `cd38e20e32f352bfde93a4ce297799ef8b5f984f8af928409ef0f3e47102e586`.
