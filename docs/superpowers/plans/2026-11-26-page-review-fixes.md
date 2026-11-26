# Page review fixes (after the 2026-11-26 UI critique)

**Source:** the UI critique of `public/index.html` + `public/app.js` on 2026-11-26 (score 25/40; snapshot in `.impeccable/critique/`). The user chose: fix all five priority issues, phone first, and **a run whose session is frozen cannot be resolved until the session is unlocked**.

**Applies on top of:** phase 1 plan (`2026-11-25-polyclave-phase1.md`), `DESIGN.md`, the approved mockup `docs/superpowers/design/page-mockup.html`. Keep the look (palette, radii, glyphs, motion rules, fluorescent only for motion). Plain HTML + one script, no framework, no new dependencies, no external requests. Responsive breakpoint stays 860 px.

**Order:** items 1 → 6, one commit each, message given per item. `pnpm check` must pass after every commit. Test names are the contract: do not rename, weaken, skip or delete tests, except the three test changes item 2 names.

---

## 1. Phone: nothing is cut off (P0) — `fix: page fits a 375 px phone`

At 375 px the stream log grows to 512 px: `white-space: nowrap` `<code>` in "Worked for" steps (long `git push /path…` lines) sizes the grid track. The run detail also overflows by 6 px.

- `.log` and `.detail` (and any other `display: grid` container holding text): `grid-template-columns: minmax(0, 1fr)`; their children `min-width: 0`.
- Step `<code>`, chips and long paths (Target in key-values, the cwd under the composer): one line, `overflow: hidden; text-overflow: ellipsis`, full text in `title`.
- Touch targets at < 860 px: phone tabs, send button, decision pills, "Remove lock…", task chips, "Worked for" summary at least 44 px tall (padding, not font size).
- Hand check: at 375 × 812, `document.querySelectorAll('*')` has no element with `scrollWidth > clientWidth` inside `#stream-log` and the detail panel, except elements whose overflow is intentionally hidden.

## 2. Unlock before review (P1, user decision) — `feat: a frozen session's runs are resolved only after unlock`

Host (`src/runs.ts` `resolve`): if the run's session is frozen, throw `FrozenError` before any other check (HTTP maps it to `409`). `retry` already refuses frozen sessions; keep that.

Tests:
- Add to `tests/runs.spec.ts`: `it('refuses to resolve a run while its session is frozen', …)` — needs_review run, session frozen: each of accept / fail / allow_retry rejects with `FrozenError`, the run row is unchanged; after `unfreezeSession` accept works.
- Add to `tests/http.spec.ts`: `it('returns 409 when resolving a run of a frozen session', …)`.
- Two existing tests resolve before unfreezing and must swap those two steps (unfreeze first, then resolve), with no other change: `tests/milestone.spec.ts` (the case with `resolve … allow_retry` then `unfreeze`, near line 601) and `tests/recovery.spec.ts` (near line 564, `runs.resolve(… 'allow_retry' …)` before `store.unfreezeSession`). A third test does the same and is also allowed one change (added 2026-11-26 after DeepSeek's PLAN-ISSUES report): in `tests/runs.spec.ts` `it('resolve accept and fail clear stale detail', …)`, insert `store.unfreezeSession(session.id)` directly before the `runs.resolve(runFail.id, { decision: 'fail', … })` call; nothing else in that test changes. If swapping changes what the test proves (for example a recovery assertion that depends on the order), stop and write the problem in `PLAN-ISSUES.md` instead of changing assertions.

Page: when the run's session is frozen, the decision pills are not shown; in their place one line: "Remove the lock on <session> first. The lock means an agent process may still be running; check that none is, then remove it." with the lock card directly above it.

## 3. Decisions cannot be made by one stray tap (P1) — `fix: confirm review decisions in place`

- Accept, Mark failed and Allow retry: first tap turns the row into "Confirm accept 0e99c9b" / "Confirm fail" / "Confirm retry" plus "Back"; only the confirm sends. Escape or Back restores the row.
- The note is required for Fail and Allow retry (inline message if empty); optional for Accept.
- Any reason a pill is disabled is shown as visible text under the row, not only in `title`.
- Move the decision block to directly under the run's status line and chips; key-values and the activity timeline follow it.
- After a decision, show a short confirmation line in the panel ("Accepted 0e99c9b", "Marked failed", "Retry allowed") until the next render of another run.
- Focus: opening "Remove lock…" moves focus into the note field; confirm rows move focus to the confirm button; closing returns focus to the control that opened it.
- The two different "Cancel" buttons: the composer's becomes "Stop turn"; form-closing buttons stay "Cancel".

## 4. Phone shows connection and which session (P1) — `fix: phone shows host connection and current session`

- Under 860 px, a one-line header at the top of the Stream view: session avatar + name + its state glyph (running / waiting / locked).
- When the host is unreachable, a banner directly under the phone tab strip on every tab: "Host unreachable · reconnecting…" (same text and orange as the desktop status bar), removed on reconnect.
- On phone load, open the "Needs you" tab when there is at least one item needing the user; otherwise "Stream".

## 5. "Needs you" grouped by session (P2) — `feat: group needs-you by session`

- One tag per session that needs the user, headed "<session name> locked" when frozen or "<session name>" otherwise. Inside, in the order they must be handled: the lock (reason, turn, time, Remove lock…), then that session's `needs_review` runs, then its `interrupted` runs. Sessions with nothing pending do not appear. Recent runs stay below as now.
- The count everywhere ("Needs you N", status bar "N need you", phone tab) counts sessions that need the user, not items.
- The sidebar meta for a frozen session shows "Locked" and the reason ellipsised at the end of the line, with the full reason in `title`; lock-card headings name the session.

## 6. Right panel follows the stream; New task does not hide the list (P2) — `fix: right panel keeps context`

- Selecting a session in the sidebar or tabs: if the right panel shows a run of another session, return it to the "Needs you" list.
- A run detail opened from the list shows "Viewing <session> · <run id>" in its breadcrumb (already partly there; make the session name the first crumb).
- "New task" opens as a section above the "Needs you" list (the list stays visible below it); the Session field is shown as text "Session: <name>", not as an input; focus goes to Instructions.

---

## Also fix while there (small)

- Status glyph SVGs get `role="img"` and an `aria-label` ("running", "waiting", "locked", "needs you", "done", "failed"); session tabs include the state in their accessible name.
- The resolve note textarea gets a visible label.
- `.avatar.sm` font size 9 px → 11 px (DESIGN.md minimum); `.att-card` radius 14 px → 12 px (`card-sm`).
- Timeline text shows "interrupted (host stopped)" instead of the raw `host_lost`; same mapping for other `endReason` values (`exit` → "finished", `cancelled` → "stopped", `never_started` → "never started").
- The frozen session's stream shows a lock line at the end ("Locked: <reason>. Messages wait until you remove the lock.").
- `main.ts`: on the first SIGINT/SIGTERM, if turns are running, print "Waiting for N running turn(s) to finish. Press Ctrl-C again to stop them now." to stderr.

## Hand check at the end

Start the host with the fake codex on scratch repos (like `makeRepo`), seed: one session running with a queued message, one frozen with a `needs_review` run (kill -9 the host during a pushed task and restart), one delivered run. Check at 1440 px and at 375 × 812: items 1–6 and the small list. Record what you saw in `HANDOFF.md`.

Update `DESIGN.md` where a component changed (decision confirm row, phone stream header and banner, per-session needs-you tag), in the same commits.
