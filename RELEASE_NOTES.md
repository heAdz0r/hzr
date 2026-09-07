# HZR 0.8.603

Adds the opt-in agtx Agent Observatory and hardens its initial integration.

The Agents dashboard observes an explicitly enrolled agtx project through a
pinned read-only helper. It shows board, runtime and hook evidence separately,
dependencies, observed transitions, linked sessions and imported usage.
Reported amounts, current-catalog API estimates and HZR output estimates remain
separate. No provider invoice or whole-task saving is inferred.

This release fixes defects found in the implementation review:

- Disabled installations no longer fail doctor for an absent optional helper.
- Workspace names, memory content and task titles require explicit publication.
- Helper output is bounded while reading; exit status, producer identity,
  deadlines and per-project observation exclusion are enforced.
- Component installation builds a clean pinned checkout, requires successful
  patch application and atomically replaces a validated binary.
- Installed bundles can find their packaged observer patch outside an HZR checkout.
- Custom-config enrollment reloads use authenticated typed settings; disabling
  and re-enabling can resume monitoring without restarting the daemon.
- Project-scoped task filters and session conflicts cannot cross worktrees.
- Imported and explicitly linked sessions map to existing HZR accounting IDs.
- Monetary and token aggregate overflow fails explicitly instead of saturating.
- Stale source evidence no longer accrues working, blocked or idle time.
- The helper reads nested task worktree hooks, rejects directory symlinks,
  returns the pinned producer commit and reads a transaction-consistent snapshot.

The previous source version 0.8.7 sorts below the already published 0.8.601.
0.8.603 restores monotonic SemVer ordering.

The helper remains an optional local build. No prebuilt adapted agtx binary is
published while its pinned LICENSE/manifest provenance discrepancy is unresolved.
Polling does not capture every agent message. Historical price snapshots and
the PRD's full-scale performance acceptance remain unfinished; current estimates
are labeled as current-catalog values. See the review report for remaining
acceptance gaps and verification evidence.
