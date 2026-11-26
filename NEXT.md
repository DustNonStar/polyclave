# Next job: page review 3

**Branch:** `feat/phase1`. **Job title for the report:** `Page review 3`.

Read `AGENTS.md` first: since 2026-11-26 the reviewer works only from GitHub, hand-check screenshots are committed to `handcheck/<job>/`, and you push `feat/phase1` at the end after `scripts/privacy-scan.sh` passes.

Do `docs/superpowers/plans/2026-11-26-page-review-3.md`, items 1 to 7 in order, one commit each with the message it gives.

Item 1 is the only host change: `src/git-effect.ts`, `src/http.ts`, and the two new tests it names in `tests/http.spec.ts`. No other file in `src/` or `tests/` changes, and no existing test changes. Items 2 to 7 change only `public/index.html`, `public/app.js` and `DESIGN.md`.

Then do the hand checks the work list names (fixtures under `/tmp/polyclave-hc/page-review-3/`, screenshots and `measurements.md` in `handcheck/page-review-3/`), append the report to `HANDOFF.md` in the format `AGENTS.md` gives, commit the report and `handcheck/page-review-3/` as `chore: page review 3`, run `scripts/privacy-scan.sh`, push `feat/phase1`, and stop.

If anything conflicts with a test or rule, stop: write it in `PLAN-ISSUES.md` (mark it `detail` or `direction`), commit, push, and stop.
