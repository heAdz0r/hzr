# HZR documentation map

Use current contracts for implementation and release decisions. Historical requirements remain
available only when they preserve acceptance or provenance that is not captured elsewhere.

## Current contracts

- [`PRD_HZR_PROJECT_ACTIVATION.md`](PRD_HZR_PROJECT_ACTIVATION.md) — project identity,
  activation, ownership, and fail-closed boundaries.
- [`PRD_HZR_TOKEN_ECONOMY_UTILIZATION.md`](PRD_HZR_TOKEN_ECONOMY_UTILIZATION.md) — accounting
  integrity and the boundary between estimated output reduction and provider-billed economy.
- [`PRD_HZR_ANTI_EVASION_SOTA.md`](PRD_HZR_ANTI_EVASION_SOTA.md) — command normalization,
  bypass enforcement, and operational acceptance.
- [`PRD_HZR_SOTA_OBSERVATORY.md`](PRD_HZR_SOTA_OBSERVATORY.md) — current Observatory
  interaction, privacy, and authenticated project-scoping contract.
- [`PRD_HZR_UPSTREAM_RTK_SYNC_0_4_7.md`](PRD_HZR_UPSTREAM_RTK_SYNC_0_4_7.md) — per-item upstream
  RTK adoption and rejection record.
- [`SOTA_RELEASE_GATES.md`](SOTA_RELEASE_GATES.md) — deterministic, provider-paired, and
  distribution release gates.

## Historical requirements provenance

- [`PRD_HZR_HONEST_ACCOUNTING_AND_BOUNDS.md`](PRD_HZR_HONEST_ACCOUNTING_AND_BOUNDS.md) — HZR
  0.3.7 W1–W8 requirements; its banner maps every workstream to a current owner.
- [`PRD_HZR_MEMORY_OBSERVABILITY.md`](PRD_HZR_MEMORY_OBSERVABILITY.md) — HZR 0.3.1 FR and
  acceptance record; its banner maps the old surface to privacy-v2 and current release contracts.

## Evidence and history

- [`releases/`](releases/) preserves the notes shipped with each release; use
  [`../RELEASE_NOTES.md`](../RELEASE_NOTES.md) for the current release.
- [`review/`](review/) contains release-linked and current independent acceptance records. Review
  files are evidence, not product contracts.
- [`../FORK_PARITY.md`](../FORK_PARITY.md), [`../fork-core/README.md`](../fork-core/README.md), and
  `../fork-core/rtk/` preserve fork identity, imported audit history, and upstream provenance.
