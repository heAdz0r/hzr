# CI Lessons

- Keep docs validation deterministic: run `bash scripts/sync-architecture-modules.sh` after any top-level `mod` change in `src/main.rs`.
- Run `bash scripts/validate-docs.sh` before push.
- If `ARCHITECTURE.md` is changed by sync, commit that update in the same PR.
- Do not remove README/CLAUDE/hook command coverage checks (`ruff`, `pytest`, `pip`, `go`, `golangci`).
