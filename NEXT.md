# Next job: Audit Hardening — Batch 2 (PR Gate, Invariants & SCM Observer)

Work on a new branch **`fix/hardening-batch2-pr-gate`** branched from the latest `main` (after Batch 1 merge).

Execute **Batch 2 only** from `docs/superpowers/plans/2026-11-27-polyclave-v5-audit-hardening.md` (Items **2.1, 2.2, 2.3, 2.4, 2.5, 2.6**, covering Codex audit findings #5, #6, #2, #9, #13, #14).

## Rules for this job

1. Read `AGENTS.md`, `docs/superpowers/plans/2026-11-27-polyclave-v5-audit-hardening.md`, `PLAN-ISSUES.md`, `docs/LESSONS.md`, and the latest sections of `HANDOFF.md` before editing.
2. Make **one commit per item** with the exact commit messages in the plan. Do not start Batch 3 in this branch; Batch 3 will follow from updated `main` after Batch 2 is reviewed and merged.
3. Run `cargo fmt --all -- --check`, `cargo clippy --locked --workspace --all-targets -- -D warnings`, and `cargo test --locked --workspace` after each item. If `proto.ts` or UI files change (Items 2.1 / 2.5), also check `git diff --exit-code -- apps/polyclave-app/ui/src/generated/proto.ts` and run `npx tsc --noEmit`, `npm run build`, and `POLYCLAVE_BROWSER_CHANNEL=chrome npm run test:browser` under `apps/polyclave-app/ui`. Run `bash scripts/privacy-scan.sh` (`clean`) at the end.
4. Do not weaken, skip, or delete any existing test. If an existing test conflicts, record it in `PLAN-ISSUES.md` and stop.
5. Append the Batch 2 report to `HANDOFF.md`, commit it (`chore: hardening-batch2-pr-gate handoff`), and **do not push `main` or merge** — leave the branch for the merge reviewer.
