# Next job: Audit Hardening — Batch 1 (Runtime & Sandbox)

Work on a new branch **`fix/hardening-batch1-runtime`** branched from `main` (after `docs: add 3-batch audit hardening plan and assign Batch 1 in NEXT.md`).

Execute **Batch 1 only** from `docs/superpowers/plans/2026-11-27-polyclave-v5-audit-hardening.md` (Items **1.1, 1.2, 1.3, 1.4**, covering Codex audit findings #12, #11, #7, #1).

## Rules for this job

1. Read `AGENTS.md`, `docs/superpowers/plans/2026-11-27-polyclave-v5-audit-hardening.md`, `PLAN-ISSUES.md`, and the Codex audit section at the end of `HANDOFF.md` before editing.
2. Make **one commit per item** with the exact commit messages in the plan. Do not start Batch 2 or Batch 3 in this branch; Batches 2 and 3 will follow from updated `main` after Batch 1 is reviewed and merged.
3. Run `cargo fmt --all -- --check`, `cargo clippy --locked --workspace --all-targets -- -D warnings`, and `cargo test --locked --workspace` after each item, and `bash scripts/privacy-scan.sh` (`clean`) at the end.
4. Do not weaken, skip, or delete any existing test. If an existing test conflicts, record it in `PLAN-ISSUES.md` and stop.
5. Append the Batch 1 report to `HANDOFF.md`, commit it (`chore: hardening-batch1-runtime handoff`), and **do not push `main` or merge** — leave the branch for the merge reviewer.
