# Next job: Real-environment end-to-end acceptance (single-story milestone)

All three batches of `docs/superpowers/plans/2026-11-27-polyclave-v5-audit-hardening.md` (closing all 14 Codex audit findings) are reviewed and merged into `main`.

No local-implementer coding job is assigned right now. Next step:

1. Confirm green `macos-latest` and `windows-latest` CI on `main`.
2. Run the single end-to-end user story on a real Mac with real vendor CLIs (`codex` / `claude`), real GitHub, and a real phone over Tailscale Serve (see `PLAN-ISSUES.md`, "Narrow the next milestone to one end-to-end user story"), and record the evidence in `HANDOFF.md`.

New feature work on mesh dispatch, desktop control/CUA, and self-evolution/community distillation remains paused until this acceptance passes.
