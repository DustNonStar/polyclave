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

## [direction, resolved] Worker write access to the mirror conflicts with immutable refs

Phase 1A Task 5 (line 64) and v5 §4.4 (line 430) grant the Codex shell write access to the entire bare mirror and shared Git metadata. V5 §4.3.1 (lines 351–355) simultaneously requires host-controlled immutable `refs/bases/<runId>` and permits Workers to publish only to the current Attempt's ref. The threat model (§1.1, line 20) includes instructions in repository content inducing out-of-scope edits. This issue is independent of upstream `origin` and of the already resolved Claude launch policy.

Evidence from two disposable local repositories: set both `receive.denyDeletes` and `receive.denyNonFastForwards` to true, then used two successive commits as test data. A regular fast-forward push successfully moved an existing `refs/bases/r00000001`. Direct Git ref updates from the filesystem writer then moved that baseline and rewound a different Attempt's branch. The two receive settings allow the former; a receive hook could stop it, but neither receive settings nor receive hooks guard ordinary filesystem writes. No live Worker, external upstream, or production mirror was involved.

Recommendation: make the final mirror and its metadata host-only, exclude it and the primary checkout's shared Git metadata from Worker write roots, and use a per-Attempt writable Git staging repo. Let the host validate and transfer only that Attempt's branch into the one authoritative mirror, then check the mirror tip and all original R1 conditions before recording a receipt. An incoming workspace is not a second R1 target; it is untrusted input. Preserve the immutable base, no direct upstream access, append-only publication, retry isolation and final result checks. Add offline regressions for out-of-scope refs, already-published bases, and two concurrent Attempts. Alternative: keep the direct Git push workflow via a host-controlled receive endpoint that enforces the same rules without giving the Worker mirror filesystem access; that requires a new sandbox-compatible transport and verified native support.

Task 5 has a local draft only. It adds offline Codex and Claude argument/parsing tests, fail-closed Claude command preparation, loopback MCP authorization, atomic receipts, and candidate isolation. Linux format, Clippy, and workspace tests passed on the draft. Its planned Codex write roots must not be connected to live execution as written. The canonical Task 5 commit and Tasks 6–9 are not complete; the draft has not been pushed.

**Resolved (project owner, 2026-11-27):** each Attempt receives its own writable staging bare repository; the final mirror and the primary checkout's shared Git metadata are outside Worker write roots. Under the per-repository lock the host imports only the current Attempt ref without a forced refspec and with tags disabled. The host checks the imported tip and full trailer range in the final mirror against the persisted immutable baseline. Submit, normal exit, and startup Recovery must consult staging before classification: a staged push without a receipt is present and requires review. Task 9 documents the two-stage path; staging is never an R1/R3 verification target.

## [direction, resolved] The Phase 1A migration gate depends on the Phase 1B Hub implementation

V5 design §4.6 (lines 464–474) asks for a fake LLM covering actual `ApiHubProvider` behavior and a pre-1B real `CliHubProvider` probe. Phase 1B plan lines 3, 14–28 and 38–43 make completion of Phase 1A and its real CLI gate a prerequisite, then first implement precisely those two providers and put the fake LLM integration in Phase 1B Task 4. Phase 1A Task 9 (lines 99–107) names the fake Worker and complete legacy core migration, not a Hub provider crate.

This affects existing assertions, not just an extra fixture: `legacy-node/tests/runner.spec.ts:91–127` requires ordinary session conversations to persist both user and agent messages and resume the same native thread; lines 130–156 require durable in-order queue delivery. `legacy-node/tests/e2e.spec.ts:257–335` exercises continuation across daemon restart. V5 locates the canonical Hub ledger and provider thread synchronization in Phase 1B Tasks 1–2. Cataloging those cases as migrated by substituting the Worker-only UI/API or by testing a fake that never exercises a Hub provider would weaken them and does not fall under the four approved assertion replacements.

Recommendation: align these phase gates with the existing implementation split. Phase 1A Task 9 still implements the Worker fixture, covers all Worker, shared storage/Git, security, receipt, and restart invariants that are independent of Hub providers, and lists each provider-dependent case with its exact owner task and unchanged assertion. Run the fake-LLM and real `CliHubProvider` tests in Phase 1B when those implementations exist; keep the archive available unchanged until the remaining assertions pass and deletion is explicitly approved. The Phase 1A real Worker probes remain separate. Alternative: explicitly bring the minimum Hub provider and durable queue into Phase 1A before Task 9, then leave the full Hub UI and triage in Phase 1B.

Checkpoint: Tasks 5–8 are individually committed and Linux format, Clippy and workspace tests passed (29 passed, 0 failed). Actual restart-state tests demonstrate that an unsigned staged push is imported and requires review; a new Attempt starts at the immutable base without the old file. The archive is unchanged. Task 9 and final native Rust CI have not run. The inherited Node workflow is not a test of this Rust branch. Retain the `dot/**` trigger and validate the complete privacy scan range when installing the Task 9 matrix.

**Resolved (project owner, 2026-11-27):** Phase 1A Task 9 covers Worker and shared-layer invariants and the native Rust checks. Transfer each Hub provider, Hub conversation continuation, and persistent chat queue assertion to its specific Phase 1B task without rewriting the assertion or claiming a Worker test replaces it. Run the model-provider fake and real integration checks in Phase 1B; keep the full Node archive unchanged until the transferred assertions pass and deletion is approved. Phase 1A Task 10 remains the separate local Worker CLI probe.

## [direction, resolved] Phase 1B startup versus the deferred real Worker probes

At `main` `5c101f8`, the Phase 1B plan (`docs/superpowers/plans/2026-11-26-polyclave-v5-phase1b.md:3`) requires Phase 1A and its real CLI gate to pass before Phase 1B starts. The v5 design also lists real Worker probes in §4.6 (line 477). But the merged review (`HANDOFF.md:745–751`) reports only the offline/native CI and local Rust checks; its final bullet explicitly leaves the opt-in real Codex and Claude Worker Task 10 probes separate. `PLAN-ISSUES.md:124` moved the real Hub integration into Phase 1B, not the real Worker probes. No real Worker pass is recorded at this base. The latest work order starts Phase 1B and retains the archive until local acceptance, so applying the literal start gate and starting Task 1 now are inconsistent unless the owner clarifies the gate.

Evidence: `git rev-parse --short origin/main` returned `5c101f8`; compare the Phase 1B prerequisite with the merged review and the preceding handoff at `HANDOFF.md:735–743`. Green native Rust CI is not evidence of real subscription CLI execution or verified Claude isolation.

- **A (recommended):** allow Phase 1B implementation and offline CI on `dot/v5-phase1b` now, without declaring the real Worker gate met. Keep real Codex/Claude Worker probes plus the 1B real Hub probe mandatory for final local acceptance. Keep Claude fail-closed and leave `legacy-node/` untouched.
- **B:** hold Phase 1B before Task 1 until the owner records both real Worker probe verdicts from an approved local setup; then start the branch work.

Stopped before Task 1. This branch adds only this checkpoint; no runtime, UI, tests, or archive files changed.

**Resolved (project owner, 2026-11-27):** implement all Phase 1B tasks, transferred Hub tests, and native offline CI now. Real Codex and Claude Worker probes, the real CLI Hub probe, and local 1440px/375px review remain final local acceptance, not passed. Preserve the complete legacy archive and Claude's fail-closed behavior. Do not merge or remove the archive from this branch.

## [direction, resolved] The built-in DeepSeek model ID does not match the official Chat Completions contract

Phase 1B Task 1 (`docs/superpowers/plans/2026-11-26-polyclave-v5-phase1b.md:18`) and v5 §4.4 (`docs/superpowers/specs/2026-11-26-polyclave-v5-rust-design.md:388`) name `deepseek-v4.1` at the built-in official endpoint. The branch's registered default reflects that value at `crates/polyclave-store/src/hub.rs:534`. DeepSeek's published [Chat Completions contract](https://api-docs.deepseek.com/api/create-chat-completion/) lists only `deepseek-flash` and `deepseek-v4-pro` for its `model` parameter. The app allowlists the exact configured model and does not let the renderer override the base URL. A green fake-LLM fixture cannot certify this production default.

Evidence: compare the provider's public `model` schema with `git grep -n 'deepseek-v4[.]1' -- docs/superpowers/plans/2026-11-26-polyclave-v5-phase1b.md docs/superpowers/specs/2026-11-26-polyclave-v5-rust-design.md crates/polyclave-store/src/hub.rs`. No vendor account, credential or live request was used.

- **A (recommended):** register the official `deepseek-v4-pro` and `deepseek-flash` IDs on the built-in profile, with Pro first, and update the plan/spec. Migrate only the unchanged built-in inventory; preserve custom profiles, exact endpoint credential binding, stream/tool tests, and the real local acceptance gate.
- **B:** keep `deepseek-v4.1` only for an independently verified custom compatible provider configured by the owner. Do not advertise it as an official default; leave the stock DeepSeek profile inactive pending registry configuration.

Checkpoint: Tasks 1–7 are committed individually. The native Tauri build has not yet been verified. The Task 8 draft remains local and unpushed. Its Chromium fixture passed at 1440×900 and 375×812 with full-width canvas, no horizontal overflow, registered ink highlights, reduced-motion text, and no external requests. These are offline fixture checks, not final UI/CLI acceptance or proof that the transferred Hub suite fully passed. The published branch must not be merged as a complete Phase 1B. The Node archive is unchanged.

**Resolved (project owner, 2026-11-27):** use the official `deepseek-flash` first and `deepseek-v4-pro` second. Update the built-in registration, UI, tests, plan, and spec. Preserve custom endpoint bindings and existing explicit selections other than the retired stock option. Finish Task 8 and verify the final native CI before handoff.

## [direction, resolved] Where a mesh `worker_run` Run is charged against a Ticket budget

A Run started on another node never meets the requesting Ticket's `max_runs`. On the primary, the Hub tool `dispatch_remote` (`bins/polyclaved/src/hub_api/tools.rs:171–236`) takes no `ticket_id`, reads no Ticket and charges nothing; the primary ledger admits no Run (asserted at `bins/polyclaved/src/hub_api/tests.rs`, "the primary ledger admitted nothing"). By contrast, local `dispatch_task` is "Dispatch only under an approved Ticket" with `ticket_id` required (`tools.rs:524–525`). On the receiver, `mesh_worker_run` (`bins/polyclaved/src/hub_api/http.rs:1184`) builds `RunRequest { ticket_id: None, .. }`, so `RunManager::accept` (`crates/polyclave-core/src/runs.rs:146–155`) creates a fresh `explicit_user` "Direct request" Ticket with `max_runs = 1` for every new `request_id` and charges that. Each distinct request therefore brings its own budget: the number of remote Runs is bounded only by the token's 120-second expiry and the node's action allowlist.

What the documents say: v5 decision 5 (`docs/superpowers/specs/2026-11-26-polyclave-v5-rust-design.md:50`) has the Hub decide every next dispatch within the Ticket's `max_runs`; §4.4 (line 430) requires the atomic `status == 'active'`, same-repo and `runs_used < max_runs` check before `dispatch_task`. Lines 33, 59 and 492 keep the SQLite ledger and R1 exclusively on the primary. The Stage 3 plan (`docs/superpowers/plans/2026-11-27-polyclave-v5-stage3.md:18`, Item 3B.1) scopes mesh tokens by node, repository, action and expiry only. Neither document says whether a remote Run counts against the primary Ticket, a mirror Ticket on the receiver, or a per-node allowance, so this needs a decision rather than a guess.

- **A (recommended):** charge the primary Ticket. `dispatch_remote` with `action = worker_run` requires the caller's `ticket_id` (active, owned by the calling Hub session, same repository) and, in one `BEGIN IMMEDIATE` transaction on the primary, calls `reserve_ticket` and records a local remote-dispatch row keyed by (Hub session, `request_id`) with node id, Ticket and, once known, the remote Run id. A resend finds that row and does not charge again. Exhaustion returns `BudgetExceededError` and moves the Ticket to `blocked_on_user` exactly as `dispatch_task` does. The receiver keeps its current single-Run bookkeeping Ticket. This keeps the budget in the one authoritative ledger the spec names. Cost: one new store table and migration on the primary.
- **B:** the sender passes its `ticket_id` and `max_runs`; the receiver keeps a mirror Ticket keyed by (node id, remote Ticket id) and charges it. This splits budget state across ledgers and only trusts numbers the sender chose, so it adds no protection against a faulty primary.
- **C:** a per-node allowance on the receiver (for example a maximum of active or daily Runs per `mesh_nodes` row, set by the owner). It protects the receiver from a faulty or compromised primary but is not tied to the user's Ticket; it needs new configuration and UI. It can be added later on top of A as defense in depth.

Nothing for this item is implemented on branch `claude/mesh-run-budget-b7a297`; the separate `request_id` idempotency fix is unaffected by the choice.

**Resolved (project owner, 2026-11-27): A + C.** (A) Each remote Run is charged to the primary Ticket that dispatched it: `dispatch_remote` with `action = worker_run` requires an active, same-repository Ticket owned by the calling Hub session. In one primary transaction it reserves `runs_used` and records a remote-dispatch row keyed by (Hub session, `request_id`). A resend finds that row and is not charged again, and exhaustion behaves exactly as in `dispatch_task`. (C) Each receiver also enforces a per-node **concurrency** limit: at most N mesh-admitted Runs from one `mesh_nodes` row may be non-terminal at once. **The default is N = 3**, chosen to keep vendor quota from draining too fast. The user can change N per node in Settings. A dispatch over the limit is refused with a clear, retryable status and is not queued silently. C is a concurrency cap, not a total; the total budget stays with A.

## [detail, resolved] Decision A (charge the primary Ticket) breaks an existing mesh test that sends `worker_run` without a Ticket

Decision A above makes `ticket_id` required for `dispatch_remote` with `action = worker_run`. One existing test encodes the old behaviour: `mesh_worker_run_and_run_status_work_end_to_end_through_dispatch_remote` (`bins/polyclaved/src/hub_api/tests.rs`, the `worker_run` JSON at about line 1972 has no `ticket_id`) unwraps the result at line 1985.

Evidence, with A applied (commit `b20f908` on the local, unpushed branch `wip/mesh-budget-a`):

```
cargo test --locked --workspace
test hub_api::tests::mesh_worker_run_and_run_status_work_end_to_end_through_dispatch_remote ... FAILED
panicked at bins/polyclaved/src/hub_api/tests.rs:1985:14:
called `Result::unwrap()` on an `Err` value: Policy
test result: FAILED. 32 passed; 1 failed
```

Every other test passes, including the four new A tests. The rules say not to change an existing test unless the work list names the exact change, so A is kept off the review branch `claude/mesh-budget-ac-238293` (which carries C and C-UI only) and waits on `wip/mesh-budget-a`.

**Proposed fix (exact test change to authorize):** in that test, before the first `call_tool`, insert an active Ticket owned by session `hub` for `repo` with a valid mandate and `max_runs` ≥ 1 (the new helper `insert_ticket(&primary.store, "t-mesh", "hub", &repo, 1, "active")` does this), and add `"ticket_id": "t-mesh"` to the `worker_run` JSON. Nothing else changes: the existing assertions stay as they are (the remote Run, its single-Run bookkeeping Ticket on the receiver, `primary.runs.list()` still empty, idempotent resend, `run_status`). Optionally add `assert_eq!` that the primary Ticket's `runs_used` is 1 after both sends. Once authorized, `wip/mesh-budget-a` gets that one test edit and is rebased onto the review branch.

**Resolved:** Approved by merge reviewer 2026-11-27: consequence of owner decision A + C; only adds a Ticket and ticket_id, no assertion changed.

## [detail, resolved] The mesh schema steps check the live schema instead of bumping `user_version`

The task asked for a store migration that keeps existing databases upgradable. The existing test `run_origin_snapshot_migrates_from_v9_keeps_the_first_snapshot_and_survives_reopen` (`crates/polyclave-store/tests/store.rs:872`) asserts `user_version == 10` after upgrading, so a v11 bump would break it. Instead, `mesh_limits::upgrade` and `remote_dispatch::upgrade` run inside the migration transaction on every open: they add `mesh_nodes.max_concurrent_runs` only when `pragma_table_info` lacks it and use `CREATE TABLE IF NOT EXISTS`. Existing ledgers upgrade once; new ledgers get the same schema (tested by `mesh_concurrency_column_upgrades_an_existing_ledger_with_default_three`). If the reviewer prefers a version bump, the fix is a v11 step plus changing that assertion to 11, which needs a work-list line for the test change.

Also a detail choice to confirm: a mesh Run holds one of the node's slots while it is `pending` or `running`. `needs_review`, `interrupted`, `done` and `failed` free it, because no Worker process is running and on the receiver nothing would otherwise release a Run waiting for review. Counting every non-terminal state instead is a one-line change in `crates/polyclave-store/src/mesh_limits.rs` (`ACTIVE_SQL`).

**Resolved:** Accepted by merge reviewer 2026-11-27: no `user_version` bump (live-schema steps), `needs_review` and `interrupted` free a slot, and the limit lives in the separate `MeshNodeLimitDto`.

## [direction, resolved] Narrow the next milestone to one end-to-end user story

Raised by the 2026-11-27 audit (an external review plus the merge reviewer's check). The code has grown wide (mesh dispatch, desktop control/CUA, self-evolution and community distillation), but the core chain has never run end to end with real tools.

**Resolved (project owner, 2026-11-27): narrow it.** The next milestone is one user story, run with real vendor CLIs and real GitHub:

> On a Mac, the developer dispatches a real task to a Worker. Mid-run the host restarts, and afterwards the page shows the same task's accurate state. A Reviewer from another vendor asks for changes, the Worker submits a new version, and on the phone the developer sees the new diff, its tests and what is not yet accepted, then approves exactly that last SHA.

Until this story passes and its evidence (commits, platform, screenshots or logs) is recorded in `HANDOFF.md`, new feature work on mesh dispatch, desktop control/CUA, and self-evolution/community distillation is paused. Fixes to existing code and anything the story needs (for example the approval card's evidence) continue. Success measures for a later comparison with T3 Code and Orca: how many times a human has to relay messages, how long after recovery until the next decision can be made, and whether the approved SHA and the evidence for it can be named.

## [detail] How a `dsh` Worker chooses a model is unknown, so a set model is refused

`docs/LESSONS.md` item 9: the `dsh` adapter passed `--model <m>`, which the real `dsh headless` does not accept, and the tests passed only because nothing ran the arguments against the real flag list. Real help on this Mac (2026-11-27, read-only):

```
$ dsh --profile headless --help
Usage: dsh --profile headless [options] [task...]
Options:
  --json                      write newline-delimited run events to stdout ...
  --session-id <id>           adopt the persisted Session with this id; ...
  --permission-mode <preset>  select a configured permission preset for this task, ...
  --approval-policy <policy>  ... (choices: "ask", "never")
  --add-dir <path>            allow an additional physical absolute directory ... (repeatable)
  -h, --help                  show this help
```

`dsh --help` adds only launcher flags (`--profile`, `--from-default-profile`, `--patch <path>`, `--dump-config*`). There is no model flag and no documented model environment variable.

`dsh --profile headless --dump-config-schema` (run with `DSH_HOME` pointed at an empty scratch directory) shows several plugin-level `model` keys (one defaults to `deepseek-v4-flash`; others are `agentOptions.model`, `agents[].model`, `modelPolicies[].model`, `modelSelection`, `agent-default-model`). Which of them the headless agent actually uses, and whether a `--patch` overlay or a profile file in the isolated `DSH_HOME` would set it per run, cannot be established without a real agent run, which this job may not start.

**What the fix does now:** `dsh_args` no longer takes a model; `dsh_worker` refuses a set model with `Error::DshModelUnsupported` ("`dsh headless` has no way to choose a model; leave the model unset for dsh"), and the runner shows that message. The Hub CLI transport (`crates/polyclave-hub/src/cli_provider.rs`) no longer passes `--model` for `dsh` and refuses a non-`default` model with `Error::Profile`. A dsh Worker therefore runs on whatever model its isolated profile defaults to.

**Proposed follow-up:** in a job that allows one real `dsh` run, find the config key the headless agent obeys, then set it through a `--patch <path>` overlay written under the isolated `DSH_HOME` (never the user's `~/.dsh`), and lift the refusal with a test that checks the overlay. Separately, item 9 also notes that `mcp_url` is not injected into `dsh`; this fix does not cover that.

## [detail] The host records no test result per SHA, so the approval card cannot show one

Found while building the approval evidence card (2026-11-27). Nothing stores "tests for SHA X":

- `submit_result` (`crates/polyclave-adapters/src/mcp.rs`, `Submit`) carries only `run_id`, `branch` and `sha`; the stored receipt in `results.payload_json` has no test summary and no list of unfinished or unverified items.
- The SCM observer (`crates/polyclave-hub/src/scm_observer.rs`) reads GitHub check runs for a PR's head SHA but only emits failures as Hub queue text and `pr_gate` change-log entries, and keeps `scm_observer_cursors.last_check_sha` (which SHA it looked at, not whether checks passed). Nothing ties that to a Run's SHA.

So the card's "Tests for this exact SHA" row always says "No test result recorded for <sha>", and its "Request satisfied" row says the Worker cannot list unfinished items.

**What recording them would need (not built):** a `test_results` table keyed by `(run_id, sha, source)` with `source` in {`worker_claim`, `host_run`, `ci`}, status (`passed`/`failed`/`pending`), a summary and the time; for CI, the SCM observer writes one row per check-run conclusion for `head_sha` when it equals a Run's `result_sha`/`observed_sha`; optionally an extended `submit_result` with `tests` and `not_done` fields stored as the Worker's claim and shown labelled as such. `RunEvidenceDto` would then gain a list filtered by the exact SHA, so a result from an older SHA can never show. A host-run test (the host itself running the project's tests on the mirror SHA) is the only source that is not a claim and would need a direction decision.


## [detail, resolved] 2026-11-27 audit fix round changed two existing assertions

Two existing tests encode the protocol behaviour that `beed1ad` deliberately changes, so they cannot stay as written. Neither change weakens what the test proves:

1. `bins/polyclaved/src/hub_api/tests.rs:2000`, `mesh_worker_run_and_run_status_work_end_to_end_through_dispatch_remote`: `assert_eq!(run.request_id, "req-mesh-1")` becomes `assert_eq!(run.request_id, "hub/req-mesh-1")`. The wire `request_id` now carries the calling Hub session as a namespace; the receiver stores the wire value verbatim.
2. The same file at `:2741`, `dispatch_remote_resend_after_a_crash_is_not_charged_twice_and_exhaustion_blocks`: the hand-built crash fixture sets `direct.request_id = Some("req-lost")`. It must use `"hub/req-lost"`, because that is the exact value `dispatch_remote` puts on the wire for `req-lost`; otherwise the fixture simulates a request the primary never sends and the resend correctly admits a second Run. Every assertion in that test is unchanged.

**Resolved (lead, 2026-11-27):** both edits applied in `beed1ad`. They are fixture/protocol expectations, not guarantees: index `idx_runs_idempotent_req` on `(hub_session_id, request_id)`, the ledger key `(hub_session_id, request_id)` and every resend invariant stay exactly as before.

## [direction] A dispatch the receiver refuses with 429/400/403 still spends a Ticket run

Raised by the 2026-11-27 review (merge reviewer #5) and confirmed in `bins/polyclaved/src/hub_api/tools.rs`: `dispatch_remote` charges the primary Ticket with `charge_remote_dispatch` **before** the HTTP call, and `crates/polyclave-store/src/remote_dispatch.rs` never refunds. A 429 from the receiver's per-node concurrency limit therefore burns one `runs_used` although nothing was started. A resend with the same `request_id` is free, but a Hub that gives up or renames the call loses that run.

What a refund needs, measured against the code:

- **A new column, not a new `outcome` value.** `remote_dispatches.outcome` has a `CHECK`, and `ALTER TABLE` cannot change it; `user_version` cannot be bumped either (`crates/polyclave-store/tests/store.rs:883-888` asserts 10). The live-schema step in `remote_dispatch::upgrade` can add a nullable `refunded_at` the same way `mesh_limits` adds its column.
- **`runs_used` can only go up today.** `reserve_ticket` (`crates/polyclave-store/src/lib.rs:94-113`) is the only writer and only increments; there is no decrement helper anywhere.
- **`blocked_on_user` has no way back.** `ticket_close` only allows `active → done|cancelled|blocked_on_user`, so a Ticket that a 429 pushed into `blocked_on_user` can never be reused even after a refund. Restoring it needs a rule; the only available signature is "`status='blocked_on_user' AND runs_used = max_runs`", which would wrongly revive a Ticket the user closed by hand after using it up.
- **400 cannot be treated as "not admitted" without a receiver change.** `mesh_worker_run` can still answer 400 *after* the Run row exists: `hub.runner.schedule(...)` errors are mapped through `MeshAdmission::Refused(HttpError::from(..))` (`bins/polyclaved/src/hub_api/http.rs:1265-1268`), and `Error::Invalid` maps to 400. Refunding on 400 as things stand would return a run that the receiver really did admit.
- **The resend must pay again** (owner's stated rule), which means `charge_remote_dispatch` has to branch on `refunded_at` and re-reserve, and `dispatch_remote`'s `first` flag (`hub_api/tools.rs:244-248`) has to treat a refunded row as a first dispatch.
- **One existing assertion changes:** `bins/polyclaved/src/hub_api/tests.rs:2943-2946` (`dispatch_remote_at_the_receiver_limit_is_retryable_and_charged_once`) asserts `(2, "active")` while the comment says "The reservation is kept"; under the refund rule the dispatch is `(1, "active")` plus `refunded_at` set. The later assertion after the resend stays `(2, "active")`, but its comment ("without a new charge") must be rewritten.

Options for the owner:

- **A (recommended):** refund only the states that provably admitted nothing — 429 and 403 (both are returned before `accept_guarded` for the paths that matter), keep the row with `outcome` intact plus `refunded_at`, re-charge a resend, and **also** fix `mesh_worker_run` so a post-admission `schedule`/`get` failure returns the admitted Run (or an `admitted: true` body) instead of 400. Leave transport errors unrefunded: "never arrived" and "arrived, reply lost" stay indistinguishable.
- **B:** refund on 429 only, and leave 400/403 alone. Smallest change, still fixes the case the audit found, and needs no receiver change; the ticket-state restore problem remains.
- **C:** keep "never refund" and change the receiver to queue a dispatch over the limit instead of refusing it. Contradicts the recorded owner decision that an over-limit dispatch is refused with a clear retryable status and never silently queued.

Nothing is implemented on any branch. This is `direction` because A needs the receiver behaviour changed and because the `blocked_on_user` restore rule can override an explicit user decision.

## [direction] What recording a test result per SHA still needs

The 2026-11-27 fix round measured the existing `detail` item above against the code. The conclusion is that the approval card's tests row cannot be fixed by a query: **no final summary text exists in the ledger at all**. `results.payload_json` holds only `{run_id, branch, sha}`; `turns` has no text column; Worker turns never write `messages`; the vendor CLI's stdout is parsed into `AgentEvent` values that only reach the SSE stream and are never persisted. The card's "Worker's claim" already reads the last `progress` event of the Attempt — there is nothing later to read.

The cheapest visible slice (one commit, no new table) is to put the claim into the receipt: optional `tests` and `not_done` fields on `submit_result` (`crates/polyclave-adapters/src/mcp.rs:417-423`, `inputSchema` at `:467-473`), stored automatically because the whole `Submit` is serialised into `results.payload_json`, then surfaced by extending `RunEvidenceDto` and `apps/polyclave-app/ui/src/views/evidence.ts` (`tests(sha)` at `:53-61` needs the evidence argument). It touches six existing assertions, which is why it is not in this round:

- `crates/polyclave-proto/src/lib.rs:128-139`: the test name says `..._has_no_test_field` and asserts exact JSON equality.
- `crates/polyclave-core/tests/recovery.rs:405-412` and `:421-428`: two `RunEvidenceDto` struct literals.
- `apps/polyclave-app/ui/tests/approval-card.mjs:30-33`, `:113-117`, `:148-150`: the fixtures, the tests-row state and the assertion that the tests-row pill has **no** `data-tone`.

Decisions the owner must make before that work starts:

1. May the tests row ever go green, and only for `source in {host_run, ci}` that equals the exact SHA?
2. Is a host-run test (the host runs the project's tests on the mirror SHA) in scope? That is a new execution engine with its own timeouts and resource limits.
3. Which check runs count: only those whose `head_sha` equals the Run's `result_sha`?
4. What shows when a `worker_claim` says passed and a `ci` row says failed for the same SHA?
5. Do `tests`/`not_done` take part in the `Submit` equality check, so the same SHA with different text is refused as a different result?
6. Is renaming the proto test allowed?
7. Is the additive live-schema upgrade (no `user_version` bump) acceptable for a new `test_results` table?

## [detail] `dsh` still gets no `mcp_url`, and choosing its model still needs a real run

Sections 9–10 of `docs/LESSONS.md` and the existing `dsh` item above stand. Two facts from the 2026-11-27 round: `crates/polyclave-adapters/src/dsh.rs` still sets only `DSH_HOME` and `POLYCLAVE_MCP_TOKEN`, so a real `dsh` Worker cannot call `submit_result`; and the model key cannot be established from `--help` or `--dump-config-schema` alone. Both need one real `dsh` run under an isolated `DSH_HOME`, which no job has authorised yet. Not attempted this round.

## [direction] External SCM events still reach the model as a user turn

The 2026-11-27 round made `ScmObserver` write Hub input with `source='host'` (`store.hub_enqueue_external`), which closes two things: an external PR comment or review can no longer be read as the user's own message in the ledger, and it can never satisfy a Ticket's basis (`hub_ticket` requires `source='user'`, so no Run can be authorised by an injected comment).

What is unchanged: both Hub providers still map every non-`agent` message to `Role::User` when they build the prompt (`crates/polyclave-hub/src/provider.rs:157`, `crates/polyclave-hub/src/cli_provider.rs:214`). A PR comment still therefore arrives at the model as a user-role turn. The text is labelled by construction ("PR #42 (org/repo) comment by alice: …"), and the authorisation chain above stops it from becoming a Ticket on its own, but the model itself cannot tell "the user said this" from "GitHub said this".

Options for the owner:

- **A (recommended if this matters):** carry the reason in `messages.metadata_json` (already a column) and have both providers render a `source='host'` message with an explicit prefix such as `[host observation, not from the user]`. It touches prompt construction and every provider test that asserts message text, so it needs its own work list.
- **B:** render the two existing internal host messages (`The prior conversation was interrupted.`, the lifecycle notices) differently from SCM observations, which requires the metadata split above anyway.
- **C:** leave it. The ledger identity, the basis rule and the admission policy already stop an external comment from authorising anything.

Nothing is implemented. This does not affect R1, and no test asserts the current role mapping for host messages.
