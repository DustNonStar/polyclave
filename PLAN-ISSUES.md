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

## [direction, resolved] Phase 1A migration must explicitly replace superseded v4 expectations

Phase 1A Task 9 (lines 99–107) requires migration of the legacy core invariant tests and explicitly records the change to `allow_retry`, but does not name the other legacy behavior assertions that must change. `AGENTS.md:63,69–72,84–87` says the old Freeze and Writers rules always apply, prohibits weakening or silently changing existing tests, and requires stopping on a conflict. The v5 spec (lines 5 and 356–375) explicitly replaces whole-session freeze with platform-specific Attempt isolation. Both behaviors cannot be implemented as written.

Evidence: `grep -nE 'it\\(.*(froz|freez|cancel|retry)' legacy-node/tests/{http,runs,runner,recovery}.spec.ts` and the assertions at these paths:
- `legacy-node/tests/runs.spec.ts:287–303` and `legacy-node/tests/runner.spec.ts:185–209` demand same-boot cancellation freeze the session and hold queued writes. The corresponding v5 spec at lines 357–362 permits a new macOS Attempt without requiring a global unlock, or safe continuation when Windows excludes the writer.
- `legacy-node/tests/recovery.spec.ts:97–136` demands a freeze after authorization without independent go evidence. The previously approved release-marker rule intentionally distinguishes this history from a released same-boot writer.
- `legacy-node/tests/runs.spec.ts:324–350` demands reuse of the interrupted worktree and its unpushed commit. The v5 spec at lines 362 and 371 calls for a clean next Attempt when a new generation is required, and forbids importing possibly tainted writes. That invariant cannot be represented by reusing the legacy test verbatim for both platforms.
- `docs/superpowers/plans/2026-11-25-polyclave-phase1.md:29–35` also requires whole-session freeze and rejecting any upstream other than the designated local bare origin. V5 spec lines 351–355 replaces the latter setup with the host-managed mirror even for a GitHub upstream; the one-target safety guarantee must remain in the new form.

Recommended resolution: explicitly authorize v5 to govern the Rust port where it supersedes Freeze, Writers, or the old single-worktree/remote setup. Keep every archived test unchanged. In Task 9, map each affected named test and old assertion to the replacement v5 assertion in a migration table: no reuse of a possibly written Attempt, no implicit success or repeat execution, a fresh worktree from the immutable base when required, and a single allowed and verified host-owned push target. Preserve equivalent assertions unchanged wherever v5 did not alter them. Alternative: require a separately approved list of every changed assertion before resuming. Do not add global session freeze or reuse an unverified Attempt as a compatibility shim.

Stopped after Tasks 1–3. Task 4 has no committed code. The original Node test files are archived without content changes. Local checks for Tasks 1–3 passed; OS code compiled and passed Clippy for Windows and macOS targets, and the process tests ran on Linux only. Rust dual-platform CI begins with Task 9 and is not yet configured.

**Resolved (project owner, 2026-11-27):** leave every archived Node test byte-for-byte unchanged. In the Rust migration, replace only three classes of assertions: whole-session freeze after cancel or crash becomes Attempt isolation under v5 §4.3; reuse of a prior worktree and unpushed commits in a retry that opens a new generation becomes a clean worktree and branch from the immutable Run base; and an existing old-generation mirror branch no longer blocks `allow_retry`, which marks that generation superseded. Hub and manager sessions do not freeze. All other core invariants retain their original strength, including R1 verification, single permitted mirror target, handshake and durable release evidence, independent idempotency checks, secret isolation, post-receipt nonzero exit, unreadable mirror behavior, terminal worker chat, in-turn consult, and candidate memory isolation. Task 9 must list each replaced assertion with its legacy test name and line, old and new assertions, and the v5 clause. The earlier release-marker and bundled SQLite decisions remain resolved.

## [direction, resolved] Legacy R3 upstream assertions are outside the three approved replacements

The archive is unchanged. The exact test `legacy-node/tests/git-effect.spec.ts:111`, “preflight accepts the fixture and rejects URL, split push, several push URLs and missing receive settings”, asserts that the configured upstream URL is rejected at line 121, split fetch and push URLs at line 127, and several push URLs at line 131. This is deliberate under v4 Phase 1 plan line 35. V5 §4.3.1 (lines 351–355) instead creates one host-owned mirror even when upstream is a GitHub URL; dispatch, verification, and result fetch use the stored mirror, not upstream. The project owner's authorization covers three replacement categories, not upstream acceptance. Changing the GitHub URL rejection into success is therefore a fourth category even though the one-target invariant survives.

Recommendation: authorize R3's upstream assertions as a separately named fourth replacement. Test that upstream GitHub URLs (and split/multiple upstream URLs if they are ignored) do not affect the host-owned destination, that no network or alternate push is attempted, that the host mirror enforces its receive settings, and that changing upstream after admission cannot redirect R1. Keep the negative checks for an invalid or unsafe **host-owned mirror**. Alternative: retain the old admission filters, including upstream URL rejection, and explicitly revise v5 to narrow the supported repository setup. Do not edit the archived fixture under either option.

**Resolved (project owner, 2026-11-27):** add exactly one fourth replacement category to the Task 9 migration table. Leave the archive unchanged. Host admission, push, and R1 must ignore the original upstream whether HTTPS, SSH, split, multiple, or missing; the only target is the host-owned append-only bare mirror. Preserve the mirror validation and the rule against redirection.

## [direction, resolved] Native Windows Claude Worker lacks the specified write and push boundary

Phase 1A Task 5 (lines 65–67) retains Claude file and shell tools in non-interactive mode. V5 §4.3.1 (line 353) restricts Worker pushes to the host mirror and the current Attempt branch; §1.1 (line 20) includes indirect prompt injection and out-of-scope edits in the threat model. §4.2 (line 343) supplies a Windows Job Object to contain process lifetime, not a filesystem or network policy. A worktree and an isolated CLI configuration are not write or network boundaries.

Evidence: Anthropic's [sandboxing documentation](https://code.claude.com/docs/en/sandboxing#what-the-sandbox-restricts) states that native Windows shell commands are unsandboxed, and that even on supported platforms, the built-in Bash sandbox does not enclose the CLI's file tools. No live CLI or credentials were accessed. The specified launch flags do not implement an equivalent external boundary.

Recommendation: keep the Claude offline adapter, protocol fixtures, and Task 10 probes, but gate real Claude Worker launches on a verified external boundary, failing closed wherever it is absent. Keep native Windows daemon/store/CI support. Alternative: make an external confinement backend part of the release now and revise Task 5 and Task 10 to include cross-platform confinement verification. Do not equate Job Object success or mirror-side R1 verification with preventing unintended writes or other pushes.

Stopped after Task 4. The Linux workspace passed format, Clippy, and tests, including actual bundled SQLite FTS5 insert/update/delete trigger synchronization and MATCH, candidate isolation, concurrent lock and budget enforcement, consult, and change-log resync. The archive is unchanged. Tasks 5–9 and native Rust CI are not yet complete.

**Resolved (project owner, 2026-11-27):** implement Claude arguments, stream handling, and offline fixtures. Real launches require verified external sandbox isolation and shall return an explicit failure if it is absent, including on native Windows. No container or WSL2 isolation backend in Phase 1A.

## [direction, open] Worker write access to the mirror conflicts with immutable refs

Phase 1A Task 5 (line 64) and v5 §4.4 (line 430) grant the Codex shell write access to the entire bare mirror and shared Git metadata. V5 §4.3.1 (lines 351–355) simultaneously requires host-controlled immutable `refs/bases/<runId>` and permits Workers to publish only to the current Attempt's ref. The threat model (§1.1, line 20) includes instructions in repository content inducing out-of-scope edits. This issue is independent of upstream `origin` and of the already resolved Claude launch policy.

Evidence from two disposable local repositories: set both `receive.denyDeletes` and `receive.denyNonFastForwards` to true, then used two successive commits as test data. A regular fast-forward push successfully moved an existing `refs/bases/r00000001`. Direct Git ref updates from the filesystem writer then moved that baseline and rewound a different Attempt's branch. The two receive settings allow the former; a receive hook could stop it, but neither receive settings nor receive hooks guard ordinary filesystem writes. No live Worker, external upstream, or production mirror was involved.

Recommendation: make the final mirror and its metadata host-only, exclude it and the primary checkout's shared Git metadata from Worker write roots, and use a per-Attempt writable Git staging repo. Let the host validate and transfer only that Attempt's branch into the one authoritative mirror, then check the mirror tip and all original R1 conditions before recording a receipt. An incoming workspace is not a second R1 target; it is untrusted input. Preserve the immutable base, no direct upstream access, append-only publication, retry isolation and final result checks. Add offline regressions for out-of-scope refs, already-published bases, and two concurrent Attempts. Alternative: keep the direct Git push workflow via a host-controlled receive endpoint that enforces the same rules without giving the Worker mirror filesystem access; that requires a new sandbox-compatible transport and verified native support.

Task 5 has a local draft only. It adds offline Codex and Claude argument/parsing tests, fail-closed Claude command preparation, loopback MCP authorization, atomic receipts, and candidate isolation. Linux format, Clippy, and workspace tests passed on the draft. Its planned Codex write roots must not be connected to live execution as written. The canonical Task 5 commit and Tasks 6–9 are not complete; the draft has not been pushed.
