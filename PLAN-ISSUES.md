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

## [direction, resolved] v5 Phase 1A recovery cannot infer whether go was delivered

The v5 spec (lines 17, 245–263, 343–345) and Phase 1A plan (lines 16, 84–86) require a restart to mark every pre-go crash as `never_started`. The database records `authorized` before the host sends `go`, but the listed schema and recovery inputs do not retain evidence of delivery or wrapper-side start. Once the wrapper has exited, these two cases can have the same evidence: host loss after authorization but before go, and host loss after go and the wrapper/target have exited. Neither a missing process nor a missing result proves that no write was attempted.

Evidence: a scratch `python3` pipe-and-SQLite check waited for the wrapper's ready line, committed authorized, and either closed the pipe or sent go first. After the wrapper exited, the observations were:

```
authorized then EOF: target never started; recovery evidence=authorized / absent
authorized then go: target started; recovery evidence=authorized / absent
```

The existing `src/recovery.ts:83–115` only uses `never_started` for the persisted prepared phase. The scratch check demonstrates the missing evidence, not platform-specific process-tree behavior. Classifying all authorized rows as never started would be unsafe.

Recommended: keep prepared/ready/authorized/go sequencing, and add a durable wrapper-side permit as a strict precondition to starting the Worker. Define the durable permit as the recovery boundary: no permit after the old wrapper has been excluded means the Worker never started; permit present or writer not excluded means conservatively handle possible execution. Add barrier tests for authorized-before-go and permit-before-spawn crashes. Alternative: keep the current protocol/schema and explicitly classify the authorized crash window conservatively, even when the host had not sent go. Neither option relaxes R1.

**Resolved (project owner, 2026-11-27):** retain the four steps. After reading `go\n` and before starting the Worker, the wrapper exclusively creates and syncs `tmp/turn-<id>.released` and syncs the parent directory on Unix; any failure prevents Worker launch. The wrapper does not delete the marker. Only the host clears it after committing the terminal turn to SQLite. At startup, recovery excludes the old wrapper before using the absence of a marker as proof; it evaluates release evidence before removing stale markers or temporary MCP configuration. No release marker means `never_started` / `excluded`; a marker plus unexcluded same-boot macOS writers means `possible` and `needs_review`.

## [detail, resolved] v5 Phase 1A Task 4 names a nonexistent rusqlite feature

The v5 spec (line 86) and Phase 1A plan (line 54) name both `bundled` and `fts5` as rusqlite features. The [published feature list](https://docs.rs/crate/rusqlite/0.43.0/features) includes `bundled` but not `fts5`; the [bundled SQLite build](https://github.com/rusqlite/rusqlite/blob/master/libsqlite3-sys/build.rs) compiles with `SQLITE_ENABLE_FTS5`. Requiring a Cargo feature named `fts5` is not the same as requiring FTS5 support.

Proposed correction: build against bundled SQLite and require a real test that creates the FTS5 table and executes MATCH on macOS and Windows. Keep WAL, single-writer design, search, and the test gate unchanged.

**Resolved (project owner, 2026-11-27):** use `rusqlite` with `features = ["bundled"]`. Exercise the actual FTS5 table, trigger synchronization, and MATCH queries in store tests on the macOS and Windows CI matrix.

Initial preflight stopped at base `d8388a3`. The project owner cleared both entries for work on `dot/v5-phase1a`; no live vendor probe is authorized in this job.

## [direction, open] Phase 1A migration must explicitly replace superseded v4 expectations

Phase 1A Task 9 (lines 99–107) requires migration of the legacy core invariant tests and explicitly records the change to `allow_retry`, but does not name the other legacy behavior assertions that must change. `AGENTS.md:63,69–72,84–87` says the old Freeze and Writers rules always apply, prohibits weakening or silently changing existing tests, and requires stopping on a conflict. The v5 spec (lines 5 and 356–375) explicitly replaces whole-session freeze with platform-specific Attempt isolation. Both behaviors cannot be implemented as written.

Evidence: `grep -nE 'it\\(.*(froz|freez|cancel|retry)' legacy-node/tests/{http,runs,runner,recovery}.spec.ts` and the assertions at these paths:
- `legacy-node/tests/runs.spec.ts:287–303` and `legacy-node/tests/runner.spec.ts:185–209` demand same-boot cancellation freeze the session and hold queued writes. The corresponding v5 spec at lines 357–362 permits a new macOS Attempt without requiring a global unlock, or safe continuation when Windows excludes the writer.
- `legacy-node/tests/recovery.spec.ts:97–136` demands a freeze after authorization without independent go evidence. The previously approved release-marker rule intentionally distinguishes this history from a released same-boot writer.
- `legacy-node/tests/runs.spec.ts:324–350` demands reuse of the interrupted worktree and its unpushed commit. The v5 spec at lines 362 and 371 calls for a clean next Attempt when a new generation is required, and forbids importing possibly tainted writes. That invariant cannot be represented by reusing the legacy test verbatim for both platforms.
- `docs/superpowers/plans/2026-11-25-polyclave-phase1.md:29–35` also requires whole-session freeze and rejecting any upstream other than the designated local bare origin. V5 spec lines 351–355 replaces the latter setup with the host-managed mirror even for a GitHub upstream; the one-target safety guarantee must remain in the new form.

Recommended resolution: explicitly authorize v5 to govern the Rust port where it supersedes Freeze, Writers, or the old single-worktree/remote setup. Keep every archived test unchanged. In Task 9, map each affected named test and old assertion to the replacement v5 assertion in a migration table: no reuse of a possibly written Attempt, no implicit success or repeat execution, a fresh worktree from the immutable base when required, and a single allowed and verified host-owned push target. Preserve equivalent assertions unchanged wherever v5 did not alter them. Alternative: require a separately approved list of every changed assertion before resuming. Do not add global session freeze or reuse an unverified Attempt as a compatibility shim.

Stopped after Tasks 1–3. Task 4 has no committed code. The original Node test files are archived without content changes. Local checks for Tasks 1–3 passed; OS code compiled and passed Clippy for Windows and macOS targets, and the process tests ran on Linux only. Rust dual-platform CI begins with Task 9 and is not yet configured.
