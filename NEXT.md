# Next job: Audit Hardening — Batch 3 (Phone Pairing, Exact-SHA Approval & Restart Reconnect)

Work on a new branch **`fix/hardening-batch3-phone-approval`** branched from the latest `main` (after Batch 2 merge).

Execute **Batch 3 only** from `docs/superpowers/plans/2026-11-27-polyclave-v5-audit-hardening.md` (Items **3.1, 3.2, 3.3, 3.4**, covering Codex audit findings #8, #3, #4, #10).

## Rules for this job

1. Read `AGENTS.md`, `docs/superpowers/plans/2026-11-27-polyclave-v5-audit-hardening.md`, `PLAN-ISSUES.md`, `docs/LESSONS.md`, `DESIGN.md`, and the latest sections of `HANDOFF.md` before editing.
2. Make **one commit per item** with the exact commit messages in the plan.
3. Run `cargo fmt --all -- --check`, `cargo clippy --locked --workspace --all-targets -- -D warnings`, and `cargo test --locked --workspace` after each item. Regenerate `proto.ts` via `POLYCLAVE_UPDATE_PROTO=1 cargo test -p polyclave-proto` when DTOs change, verify `git diff --exit-code -- apps/polyclave-app/ui/src/generated/proto.ts`, and run `npx tsc --noEmit`, `npm run build`, and `POLYCLAVE_BROWSER_CHANNEL=chrome npm run test:browser` under `apps/polyclave-app/ui`. Run `bash scripts/privacy-scan.sh` (`clean`) at the end.
4. Capture 1440×900 and 375×812 screenshots and overflow/target measurements under `handcheck/hardening-batch3/` (JPEG ≤ 70 quality, < 300 KB, fixtures under `/tmp/polyclave-hc/hardening-batch3/`), and update `DESIGN.md` in the same commit as UI changes.
5. Do not weaken, skip, or delete any existing test. If an existing test conflicts, record it in `PLAN-ISSUES.md` using the four-part format in `AGENTS.md` and stop.
6. Run a self-review pass, append the Batch 3 report to `HANDOFF.md` (explicitly separating offline-fake verification from unverified real-environment items), commit it (`chore: hardening-batch3-phone-approval handoff`), and **do not push `main` or merge** — leave the branch for independent blind review.
