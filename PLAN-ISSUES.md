# Plan issues

## [detail, resolved] 2026-11-26 page review fixes, item 2 — a third existing test resolves a run of a frozen session

Item 2 of `docs/superpowers/plans/2026-11-26-page-review-fixes.md` makes `Runs.resolve` throw `FrozenError` when the run's session is frozen ("before any other check"), and HTTP map it to `409`. It names exactly two existing tests that "resolve before unfreezing and must swap those two steps", with "no other change": `tests/milestone.spec.ts` (the `resolve … allow_retry` then `unfreeze` case, near line 601) and `tests/recovery.spec.ts` (near line 564, `runs.resolve(… 'allow_retry' …)` before `store.unfreezeSession`). The session rules for this work also say the only allowed change to an existing test is those two swaps.

A third existing test does the same thing and is not named. `tests/runs.spec.ts`, `it('resolve accept and fail clear stale detail', …)` at line 519: its second half cancels a running turn at line 558 — `runner.cancel(session.id)` freezes the session as the Freeze rule requires — and then resolves the run of that frozen session at lines 563–566:

```ts
    const failed = await runs.resolve(runFail.id, {
      decision: 'fail',
      note: 'confirmed broken',
    })
```

With item 2's guard in place this call throws, and the test fails:

```
 FAIL  tests/runs.spec.ts > Runs > resolve accept and fail clear stale detail
Error: cancelled while an agent was running (turn 2)
 ❯ Runs.resolve src/runs.ts:586:15        (the new frozen check)
 ❯ tests/runs.spec.ts:598:31              (the resolve call above, in the patched file)
```

Measured with item 2 fully applied: `pnpm check` gives 129 passed, 1 failed, 1 skipped. Nothing in items 3–6 depends on this test, but `pnpm check` cannot pass after item 2's commit while the test stays as written, so item 2 cannot be completed under the current rules.

What the test needs is the same reorder item 2 describes for the other two: `store.unfreezeSession(session.id)` before that `resolve(… 'fail' …)` call. The test already unfreezes once between its accept case and its fail case (line 549), so one more unfreeze before the fail resolve fits its shape and changes neither the assertions nor what the case proves (after the unfreeze, `fail` still has to clear `detail`). That is a change to an existing test which the work list does not authorize, so it has not been made.

State left behind:

- Last commit is `cd0b85d fix: page fits a 375 px phone` (item 1). The tree is clean and `pnpm check` passes: 128 passed, 1 skipped.
- Item 2's complete, tested work is kept out of the tree as `/private/tmp/polyclave-item2.patch` (274 lines; `git apply` from the repo root re-applies it cleanly). It contains: the `Runs.resolve` frozen guard, `it('refuses to resolve a run while its session is frozen', …)` in `tests/runs.spec.ts`, `it('returns 409 when resolving a run of a frozen session', …)` in `tests/http.spec.ts`, the two named step swaps, the page's "Remove the lock on <session> first" line with the lock card directly above and no pills, and the `DESIGN.md` entries for both.
- Items 3–6 and the "Also fix while there" list were not started.

To proceed, one of these is needed: add this third swap to the allowed changes (recommended — it is the same one-line reorder the work list already permits twice), or rule that `fail` may still resolve a run of a frozen session, which contradicts item 2's "each of accept / fail / allow_retry rejects".

**Resolved (Claude, 2026-11-26):** the third change is allowed. Item 2 of the work list now names it: insert `store.unfreezeSession(session.id)` directly before the `fail` resolve in `it('resolve accept and fail clear stale detail', …)`, nothing else. The kept patch was reviewed and is fine to apply as it is.
