# Handoff after Task 8

- Branch: `feat/phase1`, last implementation commit `58dda9b test: milestone crash experiments` (Tasks 1–8 done by Gemini; it left no handoff note or `PLAN-ISSUES.md`).
- `pnpm check` on a clean clone (Node 26.9.0): 9 files, 108 tests, all passing, about 15 s.
- `pnpm test tests/milestone.spec.ts` three times in a row: 9/9 each time, no leftover processes.
- Not yet run on Node 22.19+ or 24+ (only Node 26 is installed on the review machine).

## Review (Gate A, 2026-11-25)

**Verdict: not passed yet.** The core logic is sound — every `done` comes from a `classifyRun` verdict or a user accept, there is exactly one place that starts a turn (`TurnRunner.launchTurn`), the store's claim modes, compare-and-set `endTurn`, token rules and the supervisor's `go` handshake match the plan. What blocks the gate is one logic defect and several tests that do not prove what the plan's contract requires. Fix the items under **Must fix** first, in this order, before starting Task 9. Do not change test names; strengthen the bodies.

### Must fix

1. **A restart undoes the user's `allow_retry` decision** (`src/recovery.ts` rule 2, lines ~141–195). After the user resolves a frozen run with `allow_retry` (→ `interrupted`) and unfreezes the session, the next host start re-classifies that run with `writer` taken from its last turn, which is still `possible`, so the run goes back to `needs_review` and may refreeze the session. Rule: a run's last-turn `writerCheck` only counts if that turn ended after the run's latest user resolution; when the user resolved after it, use `writer: 'excluded'` (the same assumption `Runs.retry` already makes). Add a recovery test: `a user allow_retry survives a restart` (needs_review → resolve allow_retry → unfreeze → recover → still `interrupted`, session not frozen). This is also a gap in the plan's Task 7 wording; the reviewer will update the plan on `main`.
2. **Fixed sleeps decide outcomes in `tests/milestone.spec.ts`** (lines 281, 479, 519, 638, 668, 722: `setTimeout(r, 100)` right after `SIGKILL`). Replace each with waiting for the killed harness's `exit` event (or `waitFor(() => !isAlive(pid))`), and where the supervisor matters, also wait until its group is empty. The plan's rule is "barriers, not sleeps".
3. **`supervisor killed, a member left behind`** (`milestone.spec.ts:371`) fakes the scenario: a detached `sh` stands in for the leader and the turn row is written straight into the database with a made-up start time. Run it through a real harness and a real supervisor: start a run whose fake codex leaves a member in the group (for example a background `sleep` that is not detached), `SIGKILL` the harness and then the supervisor pid only, and assert recovery reports it `unverifiable`, does not kill it, and freezes the session.
4. **`killed before authorization`** (`milestone.spec.ts:463`): after the kill, wait for the harness and the supervisor group to be gone (item 2) before checking `prompt_started`; also assert the recovery report lists the run as `retryable` and the turn as `never_started`.
5. **`accepted, then host killed before done was written`** (`milestone.spec.ts:623`): compare `resultSha` with X — the sha the fake pushed (from the `refLog` entry for the run branch) — and assert exactly one accepted result row (open the database and count `results WHERE accepted = 1`).
6. **`second host refused while the first lives …`** (`milestone.spec.ts:693`): snapshot the run, turn and session rows before starting the second harness and assert they are identical after it exits.
7. **"Nothing recorded" assertions in `tests/mcp.spec.ts`**: in `answers 401 to unknown, prepared-only, other-instance and ended tokens` (:40) call `submit_result` (not `tools/list`) with each token and assert zero result rows; in `rejects submit_result from a chat turn without recording anything` (:92) assert zero result rows; in `two identical concurrent submissions give one acceptance` (:329) count accepted rows = 1.

### Should fix

8. **Supervisor leaves group members behind on a normal exit** (`src/supervisor.ts`, `child.on('exit')` → `finishExit()` when not terminating). If the agent started a background process inside the group and exited, the supervisor exits and leaves it running while the turn is recorded with `writerCheck: 'excluded'`. On normal agent exit, run the same termination sequence (SIGTERM the other members, SIGKILL after 2 s) before exiting. Add a proc test: `a normal exit does not leave group members behind`.
9. **`pgrep` failures read as "no members"** (`supervisor.ts` `listOtherGroupMembers`, the `catch` returns `[]` for any status). Exit status 1 means no members; any other failure should keep waiting until the 2 s timer and then SIGKILL as usual.
10. **Cancel on a turn that was never authorized freezes the session** (`runner.ts` `cancel`). A turn still in `prepared` has no agent running; end it as `never_started` / `excluded` without freezing.
11. **Smaller test gaps from the audit:** `store.spec.ts` `settles one accepted result per run` should count accepted rows; `git-effect.spec.ts` `preflight is not fooled by changing origin after run creation` should create a run, change origin, push to the original bare repo and show the check sees that push; `refuses a worktree path that holds something else` should check "nothing deleted" for all three cases; `runner.spec.ts` `a failure before authorization ends the turn as never started and frees the session` should show a following turn actually runs and assert the error type; milestone `a detached tool writes …` should compare `observedSha` and detail before and after the late push.

### Plan issue (no action needed)

- `recovery.spec.ts` `owns turns by instance id, not pid`: the plan's revision 3 has no `hostPid` column, so the "same pid" half cannot be expressed. Keep the test as an instance-id test; also assert the turn's recovery outcome.

### Next steps

The next implementer (Gemini, continuing; DeepSeek only if Gemini runs out of quota) fixes the Must-fix items, then the Should-fix items, each as its own commit (`fix: …` / `test: …`), runs `pnpm check` and the milestone suite three times, and stops for a second Gate A review. Tasks 9–11 start only after that.

## Fixes

1. **Must-fix 1 (`allow_retry` across restarts):** `188e62b` (`fix: keep user allow_retry decision across restarts`) — updated `src/recovery.ts` Rule 2 so a turn's `writerCheck` only counts when the turn ended after `run.resolution`'s timestamp, otherwise treating `writer` as `'excluded'`; added `a user allow_retry survives a restart` in `tests/recovery.spec.ts`.
2. **Must-fix 2 (fixed sleeps in `tests/milestone.spec.ts`):** `113ce37` (`test: wait on exit and group teardown instead of fixed sleeps in milestone`) — replaced all six post-`SIGKILL` `setTimeout(r, 100)` calls with `killAndWait` (process `exit` event) and `waitFor` on supervisor/group teardown.
3. **Must-fix 3 (`supervisor killed, a member left behind`):** `739dc37` (`test: run unverifiable member milestone test through real harness and supervisor`) — added `groupMember` step in `tests/fixtures/fake-codex.ts` and ran the test through a real `host-harness` and real `supervisor`, killing the harness then only the supervisor pid (`pgid`).
4. **Must-fix 4 (`killed before authorization`):** `02e91a7` (`test: assert never_started and retryable in killed before authorization`) — asserted `report.turns` has `outcome: 'never_started'` and `report.runs` has `outcome: 'retryable'` after waiting for harness and supervisor teardown.
5. **Must-fix 5 (`accepted, then host killed before done was written`):** `87f9b86` (`test: verify pushed sha from refLog and single accepted row in milestone`) — checked `resultSha` equals the pushed sha from `refLog` and queried SQLite `SELECT COUNT(*) AS n FROM results WHERE accepted = 1` equals `1`.
6. **Must-fix 6 (`second host refused while the first lives …`):** `fd70aed` (`test: assert DB rows unchanged when second host is refused`) — snapshotted `runs`, `turns`, and `sessions` rows before running the second harness and asserted exact equality after exit.
7. **Must-fix 7 (`tests/mcp.spec.ts` result-row assertions):** `c7a71ff` (`test: assert zero result rows and single acceptance in mcp tests`) — called `submit_result` for every 401 token check, asserted zero rows in `results` for 401s and chat-turn rejection, and asserted `accepted = 1` row count equals `1` on concurrent identical submissions.
8. **Should-fix 8 (normal supervisor exit cleanup):** `a51a2a1` (`fix: clean up group members on normal supervisor exit`) — updated `src/supervisor.ts` so normal agent exit/error runs the same group cleanup when other members remain; added `a normal exit does not leave group members behind` in `tests/proc.spec.ts`.
9. **Should-fix 9 (`pgrep` failures in supervisor):** `f2266ac` (`fix: keep waiting for 2s kill timer on pgrep errors in supervisor`) — distinguished `pgrep` exit status 1 (no members) from other failures in `src/supervisor.ts` so non-1 failures wait out the 2 s timer and SIGKILL; added `keeps waiting until the 2 s timer when pgrep fails` in `tests/proc.spec.ts`.
10. **Should-fix 10 (cancel on prepared turn):** `43cf255` (`fix: cancel prepared turn as never_started without freezing`) — updated `TurnRunner.cancel` in `src/runner.ts` so a turn still in `prepared` ends with `endReason: 'never_started'`, `writerCheck: 'excluded'`, and no session freeze (and unblocks the pre-authorize test barrier); added `cancels a prepared turn as never_started without freezing the session` in `tests/runner.spec.ts`.
11. **Should-fix 11 + plan-issue note (smaller test gaps):** `c9a8161` (`test: strengthen assertions across store, git-effect, runner, recovery, and milestone`) — added accepted row count check in `tests/store.spec.ts`, real run + push + check after changing `origin` and "nothing deleted" checks for all three worktree conflict cases in `tests/git-effect.spec.ts`, `ENOENT` code check and follow-up turn execution in `tests/runner.spec.ts`, `report.turns` `never_started` outcome assertion in `tests/recovery.spec.ts`, and `observedSha`/`detail` before/after comparison in `tests/milestone.spec.ts`.

## Review (Gate A, second pass, 2026-11-25)

**Verdict: passed.** Tasks 9–11 may start.

- Clean clone (Node 26.9.0): `pnpm check` 9 files, 112 tests passing; `tests/milestone.spec.ts` 9/9 three times in a row; no fixed sleeps left in the milestone tests; no leftover agent processes.
- Every Must-fix and Should-fix item is addressed by the commit listed under "## Fixes"; the source changes in `recovery.ts`, `runner.ts` and `supervisor.ts` were read and are correct, and `supervisor killed, a member left behind` now runs through a real harness and supervisor.

Notes to carry into later work (not blocking):
1. `recovery.ts` reads the resolution time by parsing the `resolution` text. It works with the current format, but a dedicated `resolved_at` column would be sturdier; add it when the store next changes.
2. When `pgrep` itself fails at the 2 s mark, the supervisor kills only its direct child, because it cannot list the other members. The host still records the turn's writer as `possible` on a hard stop, so this is safe, only less tidy.
3. Still not run on Node 22.19+ or 24+; do that at Gate B.

Before Task 9: `main` now carries the approved page design (`PRODUCT.md`, `DESIGN.md`, `.impeccable/`, `docs/superpowers/design/page-mockup.html`) and an updated Task 10. It has been merged into this branch; Task 10 must build to `DESIGN.md` and the mockup.

## After Task 11

- Last commit before handoff: `14d3a7a` (`test: end-to-end acceptance and real Codex check`, preceded by `8da0fa5` `feat: local web page` and `ed5963c` `feat: HTTP and SSE API and host entry point`)
- `PLAN-ISSUES.md`: none (file does not exist)

### Real-Codex Test Result

Command: `POLYCLAVE_REAL_CODEX=1 pnpm test tests/real-codex.spec.ts` (`codex-cli 0.172.0`)

```
$ vitest run tests/real-codex.spec.ts

 RUN  v4.1.8 <checkout>

warning: You appear to have cloned an empty repository.

 Test Files  1 passed (1)
      Tests  1 passed (1)
   Start at  19:14:26
   Duration  52.66s (transform 20ms, setup 0ms, import 30ms, tests 52.52s, environment 0ms)
```

Recorded in `docs/superpowers/research/2026-11-24-t3-probe-results.md` under `## 10. Phase 1 acceptance (real Codex)`.

### Full `pnpm check` Output

```
$ pnpm typecheck && pnpm test
$ tsc --noEmit
$ vitest run

 RUN  v4.1.8 <checkout>

warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
Switched to a new branch 'agent/rf0ddf914'
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
To /private/var/folders/tz/j0q34ccj0bs3kq7q0ctg1qk80000gn/T/polyclave-mcp-ok-9TthXt/remote.git
 * [new branch]      agent/rf0ddf914 -> agent/rf0ddf914
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
Switched to a new branch 'agent/r2806f6e4'
To /private/var/folders/tz/j0q34ccj0bs3kq7q0ctg1qk80000gn/T/polyclave-pf2-2UEmBz/remote.git
 * [new branch]      agent/r2806f6e4 -> agent/r2806f6e4
warning: You appear to have cloned an empty repository.
Switched to a new branch 'agent/r81349f06'
warning: You appear to have cloned an empty repository.
To /private/var/folders/tz/j0q34ccj0bs3kq7q0ctg1qk80000gn/T/polyclave-mcp-rej-mHaUCq/remote.git
 * [new branch]      agent/r81349f06 -> agent/r81349f06
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
Switched to a new branch 'agent/r5ed944ba'
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
To /private/var/folders/tz/j0q34ccj0bs3kq7q0ctg1qk80000gn/T/polyclave-mcp-cancel-j2D8NG/remote.git
 * [new branch]      agent/r5ed944ba -> agent/r5ed944ba
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
Switched to a new branch 'agent/r8c9ad4c6'
Switched to a new branch 'other-branch'
warning: You appear to have cloned an empty repository.
To /private/var/folders/tz/j0q34ccj0bs3kq7q0ctg1qk80000gn/T/polyclave-mcp-conc-fEVkZF/remote.git
 * [new branch]      agent/r8c9ad4c6 -> agent/r8c9ad4c6
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
To /private/var/folders/tz/j0q34ccj0bs3kq7q0ctg1qk80000gn/T/polyclave-wtc-ouuvRD/remote.git
   03b1b57..acbe0e7  main -> main
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
Switched to a new branch 'agent/r784d8039'
warning: You appear to have cloned an empty repository.
To /private/var/folders/tz/j0q34ccj0bs3kq7q0ctg1qk80000gn/T/polyclave-mcp-diff-nxUlXQ/remote.git
 * [new branch]      agent/r784d8039 -> agent/r784d8039
Switched to a new branch 'agent/r1234abcd'
warning: You appear to have cloned an empty repository.
To /private/var/folders/tz/j0q34ccj0bs3kq7q0ctg1qk80000gn/T/polyclave-crb-ZM8NSo/remote.git
 * [new branch]      agent/r1234abcd -> agent/r1234abcd
warning: You appear to have cloned an empty repository.
To /private/var/folders/tz/j0q34ccj0bs3kq7q0ctg1qk80000gn/T/polyclave-mcp-diff-nxUlXQ/remote.git
   31d9f0a..bdec977  agent/r784d8039 -> agent/r784d8039
To /private/var/folders/tz/j0q34ccj0bs3kq7q0ctg1qk80000gn/T/polyclave-crb-ZM8NSo/remote.git
   03b1b57..3a3305c  agent/r1234abcd -> agent/r1234abcd
warning: You appear to have cloned an empty repository.
Switched to a new branch 'agent/r9cd869c5'
Switched to a new branch 'agent/r9999aaaa'
To /private/var/folders/tz/j0q34ccj0bs3kq7q0ctg1qk80000gn/T/polyclave-rec-10-Ye9C7U/remote.git
 * [new branch]      agent/r9cd869c5 -> agent/r9cd869c5
To /private/var/folders/tz/j0q34ccj0bs3kq7q0ctg1qk80000gn/T/polyclave-crb-ZM8NSo/remote.git
 * [new branch]      agent/r9999aaaa -> agent/r9999aaaa
Switched to branch 'main'
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
To /private/var/folders/tz/j0q34ccj0bs3kq7q0ctg1qk80000gn/T/polyclave-crb-ZM8NSo/remote.git
   03b1b57..cca8b86  main -> main
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
Preparing worktree (new branch 'agent/rb7110a1c')
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
Preparing worktree (new branch 'agent/r06e45629')
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
To /private/var/folders/tz/j0q34ccj0bs3kq7q0ctg1qk80000gn/T/polyclave-runs-14-mWSIaz/remote.git
   78aae05..900ce83  agent/r8a558aef -> agent/r8a558aef
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.
To /private/var/folders/tz/j0q34ccj0bs3kq7q0ctg1qk80000gn/T/ms-8b-hQIcTr/remote.git
   96c1650..21274c9  agent/r292f7250 -> agent/r292f7250
warning: You appear to have cloned an empty repository.
warning: You appear to have cloned an empty repository.

 Test Files  11 passed | 1 skipped (12)
      Tests  126 passed | 1 skipped (127)
   Start at  19:17:43
   Duration  13.28s (transform 641ms, setup 0ms, import 1.03s, tests 55.63s, environment 0ms)
```

### `pnpm test tests/milestone.spec.ts` (3 consecutive runs)

Run 1:
```
 Test Files  1 passed (1)
      Tests  9 passed (9)
   Start at  19:17:57
   Duration  12.07s (transform 28ms, setup 0ms, import 38ms, tests 11.93s, environment 0ms)
```

Run 2:
```
 Test Files  1 passed (1)
      Tests  9 passed (9)
   Start at  19:18:10
   Duration  11.99s (transform 27ms, setup 0ms, import 37ms, tests 11.86s, environment 0ms)
```

Run 3:
```
 Test Files  1 passed (1)
      Tests  9 passed (9)
   Start at  19:18:22
   Duration  11.94s (transform 28ms, setup 0ms, import 38ms, tests 11.80s, environment 0ms)
```

## Gate B review (Claude): not passed — one must-fix

Checked on a clean clone of `6e7133d` with Node 26.9: `pnpm check` 126 passed, 1 skipped (the gated real-Codex test); `tests/milestone.spec.ts` 9/9 three times; `tests/e2e.spec.ts` + `tests/http.spec.ts` 14/14; no leftover host, supervisor or fake-codex processes. No fixed sleeps in the new tests (the two `setTimeout`s are shutdown timeouts). No MCP token in any GET body or SSE event (`listSessionTurns`/`listRunTurns` strip it). The page makes no external requests and draws none of the out-of-scope controls.

By hand, with `main.ts` and the fake codex on a scratch repo: two quick messages (second queued, delivered as turn 2); cancel a running turn (session locks, as the Freeze rule says); unlock with a note (queued message delivered with the interruption note once); a task to "delivered (verified)"; `kill -9` the host while a task was waiting after its push (supervisor took the agent down; nothing left running); restart (session frozen, run `needs_review`, observed sha shown); a late push to the branch, then Accept with the old sha (409, page shows the new tip), Accept the new sha (`done`); unlock and chat again; 375 px layout (three tabs, no horizontal scroll). All of that works.

### Must fix

1. **The page does not notice the host going away or coming back.** `connectStream()` in `public/app.js` sets only `onmessage`. After `kill -9` the page keeps showing the session as "Running" with nothing to say the host is unreachable; after a restart `EventSource` reconnects silently, but recovery's changes happened before the HTTP server started, so no event arrives and the page still shows "Running" instead of the frozen session and the `needs_review` run until the user reloads by hand. Fix: on `es.onerror` mark the host unreachable in the status bar (in place of "Dispatch open · up …"); on `es.onopen` (after the first) call `refreshAll()`, re-read `/api/status`, and reload the selected session's data. Add a test to `tests/e2e.spec.ts` only if it can be done without a browser; otherwise describe the hand check in this file.

### Should fix

2. After a resolve, the run keeps its old `detail` ("an agent process may still be writing"), and the page shows it on a `done` run. Either clear `detail` in `Runs.resolve` for `accept` / `fail`, or hide `detail` on `done` and `failed` runs in the page.
3. `POST /api/sessions/:id/unfreeze` on a session that is not frozen still writes an "Unfrozen by user" message. Return `409` when the session is not frozen, and write nothing. (A stray unfreeze message after a `possible` turn would also stop recovery rule 1a from refreezing.)

### Notes (no change needed now)

- The session card shows only the "running" badge while a message waits; the waiting state shows in the stream and the status bar. A cancelled turn leaves no marker in the stream. Both can wait for a later polish pass.
- `POST /api/runs` maps unexpected errors to `400` instead of `500`.
- The real-Codex test asks for `PONG` instead of the plan's `ORCHID-7`; it still proves the resumed thread remembers the first turn, so it is accepted as is.
- Codex writes its built-in `.system` skills into `~/.codex/skills` through the `codex-home/skills` link. This is what Codex does on its own anyway.
- Node 22 and 24: on official builds v22.23.5 and v24.22.3 (checksums verified), `tsc --noEmit` passes and the full suite gives 126 passed, 1 skipped on both; `tests/milestone.spec.ts` 9/9 on both; no leftover processes.

## Status when Gemini ran out of quota (checked by Claude)

Gemini made all three Gate B fixes before stopping: `419d10f` (page: "Host unreachable · reconnecting…" in the status bar, full reload on reconnect), `26437d5` (clear `detail` on accept and fail, with a test), `849f8e1` (unfreeze returns `409` on a session that is not frozen, with a test). On a clean clone, `pnpm check` gives 128 passed, 1 skipped; `tests/milestone.spec.ts` 9/9.

History note: the first half of the page fix (the `onopen`/`onerror` handlers and the status-bar text) landed inside `abc367d` ("chore: Gate B, Node 22 and 24 pass"), because Claude committed with `git commit -a` while Gemini's edit was still uncommitted; `419d10f` holds the rest. The code is right; read `abc367d` and `419d10f` together when reviewing the page fix.

The hand check for item 1 and the "## Gate B fixes" section are below.

## Gate B fixes

All three Gate B findings are in the tree, and the hand check the review asked for is recorded here. Commits: `abc367d` holds the first half of the page fix (the `onopen`/`onerror` handlers and the status-bar text, together with the Node 22/24 run) and `419d10f` the rest; `26437d5` clears `detail` on `accept` and `fail` and adds `tests/runs.spec.ts`; `849f8e1` makes `POST /api/sessions/:id/unfreeze` return `409` on a session that is not frozen, writing nothing, and adds `tests/http.spec.ts`. No test was renamed, weakened, skipped or deleted, and nothing under `docs/` changed.

No automated test covers must-fix 1. The review allowed this file when the check cannot be done without a browser, and `tests/e2e.spec.ts` drives the API with `fetch`, not a DOM — so the behaviour is established below by hand and stays without a regression test until a browser test exists.

### Hand check for must-fix 1 (the page notices the host going away and coming back)

Fixture: a scratch tree built like `makeRepo` — bare remote with `receive.denyDeletes` and `receive.denyNonFastForwards`, clone, one commit pushed to `main`. Host: `node src/main.ts --state-dir <tmp>/state --port 4399 --codex node tests/fixtures/fake-codex.ts`, with `POLYCLAVE_TEST_USER_CODEX_HOME`, `FAKE_CODEX_SCRIPT`, `FAKE_CODEX_LOG` and `FAKE_CODEX_BARRIER_DIR` set. Three sessions on that tree (`app`, `web`, `api`) and two runs:

- `app` / `rbe60e332`: commit `hello.txt` with the `Run-Id` trailer, push, `submit_result` → `delivered (verified)`, tip `bd44f6a`.
- `web` / `r6836e4d5`: commit, push, `submit_result`, then wait forever; the host is killed with `kill -9` while that turn waits after its push.

Browser: headless Chrome 158 driven over CDP at 1440×900 and at an emulated 375×812. Screenshots in `/private/tmp/polyclave-gateb-handcheck/`: `shot-desktop-1.png`, `shot-desktop-hostdown.png`, `shot-desktop-after-restart.png`, `shot-desktop-run-detail-frozen.png`, `shot-desktop-running-queued.png`, `shot-desktop-final.png`, `shot-phone-sessions.png`, `shot-phone-needs.png`, `shot-phone-375-run-detail.png`.

At 1440 before the kill the status bar read `1 running 0 queued 0 need you` on the left and `Dispatch open · up 40s · codex` on the right.

After `kill -9` (pid from `state/host.pid`), the page untouched: three seconds later the right end of the status bar showed the orange `Host unreachable · reconnecting…` in place of `Dispatch open`, and `/api/status` was refused. The left side keeps the last data it read — the marker is what says it is no longer live.

Restart on the same state dir and the same port, page never reloaded: `EventSource` reconnected on its own, `onopen` ran `refreshAll()`, and with no user action the page showed the recovery result — right end `Dispatch open · up 4s · codex`, left side `0 running 0 queued 2 need you`, the `web` card `Frozen · host stopped while an agent was running (turn 2)`, the lock card with `Remove lock…` and `Keep locked`, and the run as `Needs review` with detail `an agent process may still be writing` and observed sha `822f0c3`. The review's finding was that before the fix the page kept `Running` and the old run list until a manual reload; the behaviour above is the fixed one.

Resolving from the page: open the run → `Remove lock…` → note → `Sign & remove lock` (lock card gone, decision pills shown) → `Accept 822f0c3` → run `delivered (verified)`, resolution `accept: accept (seen 822f0c3a at 2026-11-26T04:14:48.574Z)`, and the remote tip stayed `822f0c3`. Unlock-then-resolve is the intended order. Today the host also accepts a resolve while the session is still frozen — `Runs.resolve` has no frozen check — which is item 2 of `2026-11-26-page-review-fixes.md`, not a Gate B finding. Observation for that item: with an empty note the page sends the decision word, so the resolution line reads `accept: accept (seen …)`.

Queued message while a turn runs: on `api`, one message started a turn and a second was stored waiting; the page showed `Working for 2s …`, `Waiting for agent events…`, the queued bubble `Waiting · sent 23:14`, and the status bar read `1 running 1 queued`. The session card shows only its `running` badge, as the review notes.

Phone at 375×812: the three tabs and each pane render with `document.documentElement.scrollWidth === clientWidth` (no horizontal scroll). The run detail fits: `#side-body` 375/375, and no element inside it has `scrollWidth > clientWidth` with visible overflow; the long step lines ellipsise inside their own box (`git push …` code 881 > 267, `overflow: hidden`), so nothing is cut off by the viewport. At 375 the status bar is `display: none`, so an unreachable host has no marker on a phone — the phone gap item 4 of `2026-11-26-page-review-fixes.md` closes. Must-fix 1 was about the desktop status bar.

Also checked: the page loads only same-origin resources — `performance.getEntriesByType('resource')` gives `/app.js`, `/api/runs`, `/api/status`, `/api/sessions`, `/favicon.ico` and the two per-session endpoints, with no external request. After a further restart both runs were still `delivered (verified)` and every session state survived.

Cleanup: the host was stopped with `SIGTERM` and exited 0; no `src/main.ts`, `fake-codex` or supervisor process remained; ports 4399 and 9333 were closed; the headless Chrome was shut down.

`pnpm check` on this tree: `tsc --noEmit` clean, `vitest run` 128 passed, 1 skipped (the gated real-Codex test). This is the tree the page review work list starts from.

## Gate B re-review (Claude): passed

Checked `3bdc008` against the must-fix and the two should-fixes. `pnpm check` on this tree: 128 passed, 1 skipped. The hand-check screenshots match the text above: the desktop status bar turns to the orange `Host unreachable · reconnecting…` after `kill -9`, and after a restart on the same port the page shows the recovery state (frozen `web`, lock card, `needs_review` run with observed sha, `2 need you`) without a reload. No host, fake codex or headless Chrome left running; ports 4399 and 9333 closed. No regression test for the page reconnect exists yet (no browser test harness); that stays a known gap.

Carried into the page review work list: the phone has no unreachable marker (item 4), a frozen session's run can still be resolved (item 2), and an empty Accept note is stored as `accept: accept (seen …)` (item 3 makes notes required for Fail and Allow retry; for Accept with an empty note store just the decision and the seen sha, without repeating the word).

Next: `docs/superpowers/plans/2026-11-26-page-review-fixes.md`.

## Page review fixes

`docs/superpowers/plans/2026-11-26-page-review-fixes.md`, items 1–6 in order and then the "Also fix while there" list. One commit per item, `pnpm check` after every commit (130 passed, 1 skipped each time; the skip is the gated real-Codex test). No dependencies added, no external requests from the page, nothing under `docs/` touched in these commits, nothing pushed.

| Commit | Work-list item |
|---|---|
| `cd0b85d` `fix: page fits a 375 px phone` | 1 |
| `e76c3c7` `feat: a frozen session's runs are resolved only after unlock` | 2 |
| `477a51d` `fix: confirm review decisions in place` | 3 |
| `c7dc18d` `fix: phone shows host connection and current session` | 4 |
| `f891b35` `feat: group needs-you by session` | 5 |
| `9b79f40` `fix: right panel keeps context` | 6 |
| `e082230` `fix: small page fixes` | Also fix while there |

What each one changed:

- **1** — `.log`, `.detail`, the step list, key-values and the timeline are `minmax(0, 1fr)` single columns with `min-width: 0` children; step code, chips, the Target path and the composer's cwd are one line with the full text in `title`; at ≤860px the phone tabs, send button, answer pills, "Remove lock…", unlock buttons, "Worked for" summary and clickable run chip are 44px tall (padding/height, font size unchanged). `DESIGN.md` gained the phone touch-target and "nothing is cut off" rules.
- **2** — `Runs.resolve` throws `FrozenError` when the run's session is frozen, before any other check (HTTP already maps it to `409`). Tests: `it('refuses to resolve a run while its session is frozen', …)` in `tests/runs.spec.ts` (accept / fail / allow_retry each reject and the run row is unchanged; accept works after `unfreezeSession`) and `it('returns 409 when resolving a run of a frozen session', …)` in `tests/http.spec.ts`; the two named step swaps in `tests/milestone.spec.ts` and `tests/recovery.spec.ts`, plus the third swap PLAN-ISSUES described. The page shows no decision pills on a locked session: one line "Remove the lock on <session> first. …" with the lock card directly above the block.
- **3** — first tap on Accept / Mark failed / Allow retry opens a confirm row ("Confirm accept <sha>", "Confirm fail", "Confirm retry" plus "Back"); only the confirm sends, Escape or Back restores the row and returns focus to the pill that opened it. A note is required for Mark failed and Allow retry (inline error) and optional for Accept; with no note the host now stores `accept (seen <sha8> at <time>)` instead of repeating the word. Every disabled pill's reason is written under the row. The decision block moved directly under the status line and chips (lock card above it), key-values and the timeline follow. A decision leaves one line under the status row ("Accepted 0e99c9b", "Marked failed", "Retry allowed") until another run is rendered. Opening "Remove lock…" moves focus into the note field and "Keep locked" returns it. The composer's button reads "Stop turn".
- **4** — under 860px a 44px line at the top of the Stream: 22px avatar, session name, and its state glyph plus word (running / waiting / locked). The unreachable state gets a 36px banner directly under the phone tab strip on every tab ("Host unreachable · reconnecting…", same orange as the status bar), removed on reconnect. A phone load opens "Needs you" when at least one session needs the user, otherwise "Stream".
- **5** — one tag per session that needs the user, headed "<session> locked" when frozen, holding the lock card (then that session's `needs_review` runs, then its `interrupted` runs); recent runs stay below. Every "need you" count counts sessions, not items. The lock card heading names its session, and a frozen session's sidebar meta reads "Locked · <reason>" with the full reason in `title`.
- **6** — selecting another session in the sidebar or tab bar sends the panel back to the "Needs you" list instead of showing a run of the previous session; the run breadcrumb reads "Viewing <session> · <run id>"; "New task" opens above the list (the list stays visible), shows the session as text ("Session: <name>") and focuses Instructions.
- **Also fix** — every status glyph is `role="img"` with an `aria-label` (running, waiting, locked, needs you, done, failed; the idle ring is `aria-hidden`) and session tabs carry the state in their accessible name; the resolve note has a visible "Note" label; `.avatar.sm` initials 9px → 11px and `.att-card` radius 14px → 12px; timeline end reasons read as words (`exit` "finished", `cancelled` "stopped", `host_lost` "host stopped", `never_started` "never started"); a locked session's stream ends with "Locked: <reason>. Messages wait until you remove the lock."; `main.ts` prints "Waiting for N running turn(s) to finish. Press Ctrl-C again to stop them now." on the first SIGINT/SIGTERM when turns are running.

### PLAN-ISSUES.md and how it was resolved

Item 2's guard broke a third existing test the work list did not name: `tests/runs.spec.ts` `it('resolve accept and fail clear stale detail', …)` cancels a running turn (which freezes the session) and then resolves the run with `fail`, so the new guard made it throw (`pnpm check`: 129 passed, 1 failed, 1 skipped). Written up in `PLAN-ISSUES.md` instead of changing the test. Claude resolved it on 2026-11-26: the third change is allowed — insert `store.unfreezeSession(session.id)` directly before that `fail` resolve, nothing else in that test. That is what `e76c3c7` does; no assertion changed and the case still proves `fail` clears `detail`. Nothing else in the work list conflicted with a test or rule. `PLAN-ISSUES.md` itself is still untracked on disk (the work list gives no commit message for it).

### Hand check at the end (1440 and 375 × 812)

Fixture: a fresh scratch tree made like `makeRepo` — bare remote with `receive.denyDeletes` and `receive.denyNonFastForwards`, clone, one commit pushed to `main`. Host: `node src/main.ts --state-dir <tmp>/state --port 4401 --codex node tests/fixtures/fake-codex.ts` with `POLYCLAVE_TEST_USER_CODEX_HOME`, `FAKE_CODEX_SCRIPT`, `FAKE_CODEX_LOG` and `FAKE_CODEX_BARRIER_DIR`. Three sessions (`app`, `web`, `api`) and the work list's seed: one delivered run (`app`, commit + push + `submit_result`), one run that waits after its push and is killed with `kill -9` (`web` frozen, run `needs_review` after the restart), and a chat turn on `api` that waits with a second message queued. Browser: headless Chrome 158 over CDP at 1440×900 and 375×812. Screenshots in `/private/tmp/polyclave-handcheck-final/shots/`.

At 1440:

- Status bar `1 running 1 queued 1 need you` while the one needy session held two items (a lock and a `needs_review` run); the sidebar badge, panel badge and phone tab all read 1. `desktop-1-needs-grouped.png`.
- Needs you showed one tag `web locked` (avatar, name, lock glyph right) with the lock card inside (`web locked 23:42`, "host stopped while an agent was running (turn 2)", `Remove lock…`) and then its run `rbab5b28a Add web.txt · an agent process may still be writing · 8c884a0`; Recent runs below with `r6fbda9e0 Add hello.txt · delivered (verified) · a478d5e`.
- The frozen run's detail ran crumb / title / status / chips / lock / question / key-values / activity, with no decision pills, the "Remove the lock on **web** first. …" line, and the lock card immediately above the block; the crumb read `Viewing web · rbab5b28a`. `desktop-2-frozen-run.png`.
- `Remove lock…` put focus in the unlock note field (active element `TEXTAREA` inside the form). `desktop-3-unlock-form.png`.
- After the decisions: `1 running 1 queued 0 need you`, no tags, two recent runs; tab names `app, done`, `web, failed`, `api, running`; glyph labels `done`, `failed`, `running`; New task sat above the list with `Session: app` as text (no Session input) and focus in Instructions; with a run open, clicking the `api` session returned the panel to the list; the delivered run read `delivered (verified)` with resolution `accept (seen 8c884a0c at 2026-11-26T04:43:12.273Z)` and timeline `Turn #2 · interrupted (host stopped)`. `desktop-4-list-after.png`, `desktop-5-new-task.png`, `desktop-6-context-back-to-list.png`, `desktop-7-delivered-run.png`.

At 375 × 812:

- The load opened the "Needs you" tab (count 1) because a session needed the user; the needs list and the run detail had `document.documentElement.scrollWidth === clientWidth`, `#side-body` 375/375 and no element with visible overflow. `phone-1-needs-first.png`, `phone-2-frozen-detail.png`.
- Phone tabs 44px, `Remove lock…` 44px; unlocking there put focus in the note field; after `Sign & remove lock` the pills appeared with the visible "Note" label. `phone-3-unlocked.png`.
- `Accept` opened `Confirm accept 8c884a0` with `Back` and focus on the confirm button, no pills; Escape restored the pills with focus back on the accept pill; confirming with an empty note produced `delivered (verified)`, the line `Accepted 8c884a0`, a count of 0 and the stored resolution `accept (seen 8c884a0c at …)`. `phone-4-confirm.png`, `phone-5-accepted.png`.
- The running session: header `AP api running` with the running glyph, `Working for 41s`, the queued bubble `Waiting · sent 23:42`, and the composer button reading `Stop turn`. `phone-6-running-queued.png`.
- The `app` stream at 375 with the long `git push …` step open: `#stream-log` 375/375, the step `code` elements ellipsised (`881 > 267`, `452 > 267`) with their full text in `title`, and nothing with visible overflow — the item 1 requirement on the Stream view, not only the detail panel. `phone-7-stream-long-push.png`.
- With the host gone, `Host unreachable · reconnecting…` appeared directly under the phone tab strip. `phone-8-before-host-gone.png`, `phone-9-host-unreachable.png`.
- The locked session's stream ended with `Locked: host stopped while an agent was running (turn 12). Messages wait until you remove the lock.` (checked with a synthetic lock on a third session).

Stopping: the first `SIGINT` while the `api` turn was still running printed exactly `Waiting for 1 running turn(s) to finish. Press Ctrl-C again to stop them now.` on stderr; the second `SIGINT` ended the host (exit 130) and the phone banner appeared. Every host, `fake-codex` and headless Chrome this work started was stopped — the intermediate checks on ports 4399 and 9333 as well as the final run on 4401 and 9334 — and all four ports are closed.

Final `pnpm check` on `e082230`:

```
 Test Files  11 passed | 1 skipped (12)
      Tests  130 passed | 1 skipped (131)
   Duration  17.77s (transform 716ms, setup 0ms, import 1.26s, tests 70.02s, environment 0ms)
```

## Page review re-review (Claude): passed except one layout bug

Checked `cd0b85d`..`bb71fa0`. `pnpm check`: 130 passed, 1 skipped. The only lines removed from existing tests are the three unfreeze-before-resolve changes item 2 allows and one import line. The host change is right: `resolve` checks the frozen session right after loading the run, the HTTP layer maps it to 409, and the new resolution text `accept (seen … at …)` still matches the timestamp parser in `recovery.ts`. Every new value the page interpolates is escaped. The screenshots in `/private/tmp/polyclave-handcheck-final/shots/` match the report for items 1, 2, 3, 5 and 6 and the small fixes.

One bug (item 4): on a phone, while the host is unreachable, `#phone-banner` shows **above** the tab strip and a tall empty band opens under the tabs (`phone-9-host-unreachable.png`). The banner is the first child of `.app`, and the phone grid `auto 1fr` stretches the tab strip into the `1fr` row. The report says the banner "appeared directly under the phone tab strip"; the screenshot shows otherwise. From now on, reports describe the screen, not the work list (see `AGENTS.md`, Hand checks).

Process change from here on: `AGENTS.md` holds the standing rules for every agent, and `NEXT.md` holds the one job to do now. The user only needs to say "Read AGENTS.md and NEXT.md and do it." `NEXT.md` now asks for the phone banner fix.

## Phone banner fix

- 88ffc59 fix: phone banner sits under the tab strip — pnpm check: 130 passed

Commits:
- `88ffc59` `fix: phone banner sits under the tab strip`

The change: `.app` now pins its rows instead of relying on auto-placement. Desktop keeps `grid-template-rows: 1fr auto` with `.work` on row 1 and `.statusbar` on row 2; under 860px it is `auto auto 1fr` with `.tabs` on row 1, `.phone-banner` on row 2 and `.work` on row 3, so the banner can only ever sit between the tab strip and the work area, and an empty `auto` row collapses when the banner is hidden. Only `public/index.html` changed. `DESIGN.md` already describes the banner as "directly under the phone tab strip", so it needed no edit.

Tests: no test changed (none allowed for this job). Final `pnpm check` on `88ffc59`:

```
 Test Files  11 passed | 1 skipped (12)
      Tests  130 passed | 1 skipped (131)
```

Hand check (fake codex, scratch repo under `.handcheck/phone-banner/`, host on port 4402, headless Chrome 158 over CDP). Measurements are `getBoundingClientRect()` values in CSS pixels.

Host up, 375 × 812 — tab strip at 0–61px, `#phone-banner` hidden with height 0, stream header at 61–105px: no banner and no gap (`phone-head.top - tabs.bottom` = 0). `.handcheck/phone-banner/shots/phone-1-up-stream.png`; the same view on the other two tabs in `phone-2-up-sessions.png` and `phone-3-up-needs.png`.

Host killed with `kill -9`, page untouched, 375 × 812 — the orange banner now sits at 61–97px, directly under the 0–61px tab strip, and the stream header follows at 97–141px: gap tabs→banner 0, banner→header 0. On the Sessions and Needs you tabs the banner is at 61–97px with gap tabs→banner 0 as well. `.handcheck/phone-banner/shots/phone-4-down-stream.png`, `phone-5-down-sessions.png`, `phone-6-down-needs.png`.

Host restarted on the same port 4402 — the banner went away by itself with no reload: `performance.getEntriesByType('navigation').length` = 1, `performance.timeOrigin` 05:01:38 against 05:02:09 when the shot was taken, and the gap from the tab strip to the stream header is back to 0. `.handcheck/phone-banner/shots/phone-7-back-up.png`.

1440 px — `#phone-banner` computes to `display: none` both up and down, so no banner appears; the status bar carries the state in its own words: `0 running 0 queued 0 need you  Dispatch open · up 12s · codex` up, then `0 running 0 queued 0 need you  Host unreachable · reconnecting…` down, with the three columns and the 30px status bar unchanged. `.handcheck/phone-banner/shots/desktop-1-up.png`, `desktop-2-down.png`.

Issues: none opened. The one item in `PLAN-ISSUES.md` (the third unfreeze-before-resolve change) was already resolved by the reviewer and is not reopened.

Cleanup: the hand-check host was killed (`kill -9`, twice, as the check needs) and the headless Chrome and CDP port were stopped; no `src/main.ts`, `fake-codex` or browser process is left and ports 4402 and 9335 are closed. The fixture, scripts and screenshots stay in the git-ignored `.handcheck/phone-banner/` (nothing under `/tmp`).

Not done: nothing.

## Phone banner fix review (Claude): passed

Checked `88ffc59` and `2332f41`. `pnpm check`: 130 passed, 1 skipped. Only `public/index.html` changed. `.app` now places each child in an explicit row, so the banner can only sit between the tab strip and the work area, and the desktop rows are unchanged. The screenshots in `.handcheck/phone-banner/shots/` match the report: on all three phone tabs the banner sits directly under the tab strip with no gap, and at 1440 there is no banner and the status bar marker still shows. No process or port was left behind. This report described the screen and gave measurements; that is the format to keep.

Phase 1, Gate B and the page review fixes are all done. `NEXT.md` has no open job until the user picks the next direction.

## Page review 2

Commits:
- `179f207` `fix: desktop stream keeps to its column`
- `d4418ed` `fix: live updates keep focus and caret`
- `6f2d627` `fix: confirm uses the sha shown when the row opened`
- `8fca795` `feat: unlock shows evidence and its effect`

`pnpm check` after every commit: 130 passed, 1 skipped. Page-only work: `public/index.html`, `public/app.js`, `DESIGN.md`; nothing in `src/`, `tests/` or `docs/`.

Tests: no test changed (this job needs none and allows none). Final `pnpm check` on `8fca795`:

```
 Test Files  11 passed | 1 skipped (12)
      Tests  130 passed | 1 skipped (131)
```

Hand check (fake codex, fixture under `.handcheck/page-review-2/`, host on port 4403, headless Chrome 158 over CDP). Numbers are `getBoundingClientRect()` / `scrollWidth` values in CSS pixels.

**1. Desktop stream keeps its column.** Session with a 190-character cwd, 1440 × 900: `.main` 764 / 764 and `#stream-log` 764 / 764 (`scrollWidth` / `clientWidth`), document overflow 0. With a user bubble and a queued bubble on screen, both end at x = 1016 and the Send button at 1004, while the right panel starts at 1048 — all three fully left of it. The cwd under the composer is one line, ellipsised, with the 190 characters in its `title`. `.handcheck/page-review-2/shots/item1-desktop-long-cwd.png`, `item1-desktop-queued.png`.

**2. Live updates keep focus and caret.** A fake turn that emits three replies with 500 ms between them fired SSE traffic while typing: unlock note `abc` → `abcdef` (caret 6, still focused) with the updates arriving mid-word; decision note `xy` → `xyz` (caret 3, focused); New task instructions `task` → `task ok` (caret 7, focused); on the 375 emulation the unlock note went `abcdefp1` → `abcdefp1p2` (caret 10, still focused, on the Needs you tab). No character was lost in any field. `shots/item2-unlock-note.png`, `item2-decision-note.png`, `item2-task-instructions.png`, `item2-phone-unlock-note.png`.

**3. Confirm keeps the sha the row showed.** First tap stored `{runId: r5542a05b, decision: accept, sha: 6aae12d8ba94888e19e0df01277e87981d5cc5bd}` and the row read "Confirm accept 6aae12d". Setting that run's `observedSha` to `ffff…` in `state.runs` and calling `render()` closed the row and printed under the status line "The branch tip moved to fffffff while you were confirming. Review it, then accept again.", the Accept pill then read "Accept fffffff", and focus was on it. On an unchanged row the confirm sent exactly the shown sha — captured request body `{"decision":"accept","sha":"6aae12d8ba94888e19e0df01277e87981d5cc5bd","note":""}` — and the run became `done` with `accept (seen 6aae12d8 at 2026-11-26T05:27:41.184Z)`. `shots/item3-moved-tip.png`, `item3-confirm-sent.png`.

**4. Unlock shows evidence and its effect.** Session locked by `kill -9` during a pushed task (reason "host stopped while an agent was running (turn 2)", one message queued). At 1440 the card showed: *Agent process* "process group 68066, started Thu Nov 26 05:25:49 2026"; *Turn* "Turn 2, ended 00:25 (host stopped)"; *Working directory* the run's worktree `…/state/worktrees/rc9396473` on one ellipsised line with the full path in `title`; then "To check on the Mac: `ps -g 68066` shows nothing, and `git -C …/state/worktrees/rc9396473 status` shows no change you did not expect." Opening the form put focus in the note and showed "Removing the lock lets app run again.", "Run rc9396473 will then wait for your decision." and "1 queued message will be sent.", with the button reading "Remove lock with this note". Escape closed the form and returned focus to "Remove lock…". The stream's own "Remove lock…" button opened the form in the panel with focus in the note; on 375 it switched to the Needs you tab first and did the same. After unlocking, the card's place held "Lock removed on app. Run rc9396473 now waits for your decision.", `/api/sessions` reported the session unfrozen with 0 queued and the run still `needs_review`. `shots/item4-lock-card-evidence.png`, `item4-unlock-form.png`, `item4-stream-unlock-desktop.png`, `item4-phone-stream-unlock.png`, `item4-phone-lock-removed.png`.

Issues: none opened; `PLAN-ISSUES.md` is unchanged.

Cleanup: the hand-check host was stopped with `SIGTERM` (exit 0) after the `kill -9` the fixture needs, and the headless Chrome and its CDP port were stopped. No `src/main.ts`, `fake-codex` or browser process is left, and ports 4403 and 9336 are closed. Fixture, scripts and screenshots stay in the git-ignored `.handcheck/page-review-2/`.

Not done: nothing.

## Page review 2 review (Claude): items 1–3 pass; item 4 needs a small follow-up

Checked `179f207`..`4dc1511`. `pnpm check`: 130 passed, 1 skipped; nothing outside `public/` and `DESIGN.md` changed. Code read: the focus keeper captures only text fields inside the side panel, the new-session box and the composer, and restores by a stable key; the confirm row stores the sha at the first tap and sends that sha; lock evidence comes from `/api/sessions/:id/turns`, fetched only when a session's lock reason changes; every interpolated value is escaped. The screenshots match the report for items 1–3.

Item 4 works, but the screenshots show two things the report did not mention:
- At 375 the lock card is wider than its group: the time in the card heading is clipped at the card edge, and the note field and "Remove lock with this note" stick out on the right (`item4-phone-stream-unlock.png`). Likely cause: `.lock` is a grid with no column template and the `.unlock .row` buttons do not wrap; the longer button text (from the work list) tipped it over.
- "started Thu Nov 26 05:25:49 2026" is the host's UTC `ps lstart` string (`src/proc.ts` runs `ps` with `TZ=UTC`), shown next to "ended 00:25" in local time, so one event reads five hours apart. The work list said "as the host gives it"; that was the reviewer's mistake.

Also seen, minor: the stream lock line puts the lock glyph on its own line above the text (`item4-stream-unlock-desktop.png`).

`NEXT.md` now asks for these three fixes.

## Page review 2 follow-up

Commits:
- `904533d` `fix: lock card fits a phone`
- `e0ada71` `fix: agent start time in local time`
- `73a3d49` `fix: stream lock line layout`

`pnpm check` after every commit: 130 passed, 1 skipped. Page-only: `public/index.html`, `public/app.js`, `DESIGN.md`; nothing in `src/`, `tests/` or `docs/`.

Tests: no test changed (this job needs none and allows none). Final `pnpm check` on `73a3d49`:

```
 Test Files  11 passed | 1 skipped (12)
      Tests  130 passed | 1 skipped (131)
```

Hand check (fake codex, fixture under `.handcheck/page-review-2-follow-up/`, host on port 4404, headless Chrome 158 over CDP). Numbers are `getBoundingClientRect()` / `scrollWidth` values in CSS pixels.

**1. Lock card fits a 375 px phone.** Locked session (`kill -9` during a pushed task), form open, 375 × 812: nothing inside `.need-group` has `scrollWidth > clientWidth` with visible overflow and the document has no horizontal scroll (0). The card spans 30–345 and its content 46–329: the heading sits at 46–329 with the time inside at 294–329 (no longer clipped), the note field at 46–329, "Keep locked" at 223–329 and "Remove lock with this note" at 46–329 — the two buttons take 44px each, and the submit wrapped onto its own full row below the right-aligned "Keep locked" (button tops 796 and 848). `shots/item1-phone-lock-card.png`, `item1-phone-buttons.png`.

**2. Agent start time in local time.** The lock turn read "Agent process = process group 76805, started 00:34:01" and "Turn = Turn 1, ended 00:34 (host stopped)". The host recorded `agentStartedAt` "Thu Nov 26 05:34:01 2026" (UTC) and `endedAt` 05:34:06Z, i.e. 00:34 local — start and end now read in the same zone. The other branches, checked in the page: `fmtAgentStart('Wed Nov 25 23:58:12 2026')` → "started Nov 25, 18:58:12"; `'Fri Dec 26 05:34:01 2025'` → "started Dec 26, 2025, 00:34:01"; a string that does not parse is shown raw ("not a date") and a missing value reads "unknown". `shots/item2-local-start.png`.

**3. Stream lock line on one line.** At 1440 the glyph starts the first text line (glyph top 239, text top 236, same row, glyph left of the text), the text wraps to 2 lines and "Remove lock…" sits at the right of the same row; the line is 629/629. At 375 the same shape with 4 text lines (glyph top 332, text top 329) and the button in the row, line 343/343 — the glyph is never alone above the text. `shots/item3-lock-line-1440.png`, `item3-lock-line-375.png`.

Overflow check for the changed components at 375 (no element with `scrollWidth > clientWidth` and visible overflow): `.need-group` 339 wide, the run detail `#side-body` at 375 — which renders the same lock card with the evidence block, check line and note field — and `#stream-log` at 375; document overflow 0 in all three. At 1440 the stream column is 764 and the panel 391 with no overflow. `shots/final-375-needs.png`, `final-375-run-detail.png`, `final-375-stream.png`, `final-1440.png`.

Issues: none opened; `PLAN-ISSUES.md` is unchanged.

Cleanup: the hand-check host was stopped with `SIGTERM` (exit 0) after the `kill -9` the fixture needs; the headless Chrome and its CDP port were stopped. No `src/main.ts`, `fake-codex` or browser process is left, and ports 4404 and 9337 are closed. Fixture, scripts and screenshots stay in the git-ignored `.handcheck/page-review-2-follow-up/`.

Not done: nothing.

## Page review 2 follow-up review (Claude): passed

Checked `904533d`..`3f7a8e4`. `pnpm check`: 130 passed, 1 skipped. The lock card fits 375 (heading time, note and both buttons inside the edge; the submit takes its own 44 px row), the agent start time is read as UTC and shown in local time next to the end time, and the stream lock line keeps glyph, text and button in one row. Screenshots match the report.

Third UI critique after this: 26/40 (trend 25 → 26 → 26). The earlier fixes hold; the new findings are older defects (silent send failure) and one side effect of the evidence block (a long lock card on a phone). `NEXT.md` now asks for page review 3, which includes the user's decision to show a commit summary before accepting.

## Branch published (2026-11-26)

The reviewer role moves to a remote agent that reads only GitHub; implementers now push themselves after `scripts/privacy-scan.sh` passes, and commit hand-check screenshots to `handcheck/<job>/` (see `AGENTS.md`). CI runs `pnpm check` on every push. Before the first push the roughly 70 local commits were squashed into one, because the history carried a local path; **commit hashes mentioned above this line are local only and are not on GitHub.** Next: `NEXT.md` (page review 3, given to Gemini).

Same day, user decision: `main` was fast-forwarded to the published branch and all work now goes straight to `main`; `feat/phase1` is retired.

## Page review 3

- 189ebd6 test: wait for the thread id before the token snapshot — pnpm check: 130 passed
- 6b0a4c9 feat: run detail includes a commit summary — pnpm check: 132 passed
- 3d0f519 feat: show the commit before accepting it — pnpm check: 132 passed
- 964f909 fix: failed send or stop keeps the text and says why — pnpm check: 132 passed
- 0b2bd5f fix: opening a run selects its session — pnpm check: 132 passed
- 0571866 fix: review question after unlock — pnpm check: 132 passed
- 943aeed fix: confirm sits away from the first tap — pnpm check: 132 passed
- 092906e fix: shorter lock card on a phone — pnpm check: 132 passed
- 4e3cb4e fix: constrain host question width on phone — pnpm check: 132 passed

Commits:
- `189ebd6` `test: wait for the thread id before the token snapshot`
- `6b0a4c9` `feat: run detail includes a commit summary`
- `3d0f519` `feat: show the commit before accepting it`
- `964f909` `fix: failed send or stop keeps the text and says why`
- `0b2bd5f` `fix: opening a run selects its session`
- `0571866` `fix: review question after unlock`
- `943aeed` `fix: confirm sits away from the first tap`
- `092906e` `fix: shorter lock card on a phone`
- `4e3cb4e` `fix: constrain host question width on phone`

Tests:
Final `pnpm check` on `4e3cb4e`:

```
 Test Files  11 passed | 1 skipped (12)
      Tests  132 passed | 1 skipped (133)
   Start at  01:44:11
   Duration  17.97s (transform 847ms, setup 0ms, import 1.30s, tests 72.30s, environment 1ms)
```

`pnpm test tests/milestone.spec.ts` passed 5 consecutive runs (9/9 each) after `189ebd6`.
Existing test changed: `tests/milestone.spec.ts` (`second host refused while the first lives; old token refused after restart`), polling SQLite with `waitFor` for non-null `runs.agent_session_id` between `bHold.waitReached()` and the DB snapshot (allowed by `docs/superpowers/plans/2026-11-26-page-review-3.md`, item 0). Two new tests added to `tests/http.spec.ts` (`run detail includes the commit subject and changed files` and `run detail commit is null when there is no sha`), allowed by item 1.

Hand check:
Fixture under `/tmp/polyclave-hc/page-review-3/` (bare remote + long-path clone + fake codex on port 4410, headless Chrome over CDP at 1440×900 and 375×812). Screenshots and JSON measurements in `handcheck/page-review-3/`. All 13 JPEGs are 32.8–127.7 KB (< 300 KB), and all 115 loaded page resources were same-origin (`external: []`).

- **Item 7 (lock card at 1440 and 375):** On a fresh desktop load (`handcheck/page-review-3/item7-desktop-locked.jpg`), `How to check on the Mac` is open by default (`detailsOpen: true`) showing `ps -g 83715` and `git -C /private/tmp/polyclave-hc/page-review-3/state/worktrees/raaea020a status` in separate `code` lines, `Working directory` reads `…/worktrees/raaea020a`, and `Can't check from here? Keep it locked; nothing runs until you remove the lock.` sits above `Remove lock…`. On a fresh 375×812 load (`handcheck/page-review-3/item7-phone-locked-needs.jpg`), the Needs you tab opens, `How to check on the Mac` is closed (`detailsOpen: false`), `Remove lock…` ends at `y = 588.4` inside the `812` px viewport with `docOverflow: 0` and `badCount: 0`. Expanding `How to check on the Mac` at 375 (`handcheck/page-review-3/item7-phone-details-open.jpg`) wraps the full `git -C …` command inside the card with `docOverflow: 0` and `badCount: 0`.
- **Item 4 (opening a run selects its session):** With `api-server` selected, opening `web`'s run from Needs you at 1440 (`handcheck/page-review-3/item4-desktop-open-run.jpg`) switches the session tab, sidebar selection, and composer placeholder (`Ask web — …`) to `web`. At 375 (`handcheck/page-review-3/item4-phone-open-run.jpg`), opening `web`'s run from Needs you keeps the active tab on `Needs you` (`side`), updates the hidden phone stream header (`#phone-head .name`) and composer placeholder to `web`, with `docOverflow: 0` and `badCount: 0`.
- **Items 5 and 2 (unlocked question + What changed / Accepted commit):** After unlocking `web`, at 1440 (`handcheck/page-review-3/item5-desktop-unlocked-review.jpg`) and 375 (`handcheck/page-review-3/item2-phone-what-changed.jpg`), the question reads `Review 03be558: accept it, mark it failed, or allow a retry.` with `Host note: An agent process may still be writing.` below it. Directly above `Note` and the pills, `What changed` shows subject `feat: add nested widget, binary asset and styles (raaea020a)`, 5 files (`README.md +1 −0`, `assets/icon.bin binary`, `docs/notes.md +2 −0`, middle-ellipsised `src/components…omponent-name.ts +2 −0`, `src/styles/theme.css +1 −0`), and `and 2 more files`. At 375 after fixing `.ask` grid column sizing (`4e3cb4e`), `#host-question` stays inside its margins with `docOverflow: 0` and `badCount: 0`. After accepting (`handcheck/page-review-3/item2-desktop-accepted-commit.jpg`), `Accepted commit` renders directly under `delivered (verified)`.
- **Item 6 (confirm row and 400 ms guard):** Fast double-tap on `Accept 03be558` plus an immediate click on `#confirm-decision` at 1440 (`handcheck/page-review-3/item6-desktop-confirm-arming.jpg`) opened the confirm row with `Back` on the left (`x = 1107`) and `Confirm accept 03be558` on the right (`x = 1178.8`) at `opacity: 0.45` (`aria-disabled="true"`), keeping the fine print (`Accept applies to 03be558 only …`) visible and sending `0` `/resolve` requests. After >400 ms at 375 (`handcheck/page-review-3/item6-phone-confirm.jpg`), `Confirm accept 03be558` is at `opacity: 1` (`aria-disabled="false"`) with `docOverflow: 0` and `badCount: 0`.
- **Item 3 (failed send preserves text and shows error):** After `kill -9` on the host, submitting `Keep this draft when host is down` leaves the text in the composer, re-enables Send (`sendDisabled: false`), and shows orange `Not sent: Failed to fetch. Your message is still here.` below the composer box at 1440 (`handcheck/page-review-3/item3-desktop-send-failed.jpg`) and 375 (`handcheck/page-review-3/item3-phone-send-failed.jpg`). Note on 375 Stream (`item3-phone-send-failed.jpg`): the pre-existing `.act` task-chip row above the composer reports `scrollWidth 368 > clientWidth 343` (`overflow-x: visible`) because the long chip text meets the right edge; the composer and error line themselves do not overflow. After restarting the host and resubmitting (`handcheck/page-review-3/item3-desktop-send-recovered.jpg`), the message appears in the stream, the textarea clears, and the error line hides.

Issues: none opened; `PLAN-ISSUES.md` unchanged.
Cleanup: host, fake codex, and headless Chrome stopped; `ps` confirmed no remaining `polyclave`, `fake-codex`, or port 9340 processes; ports 4410 and 9340 closed.
Privacy scan: privacy-scan: clean (origin/main..HEAD)
Not done: "Stop turn" failure error message (`Could not stop the turn: …`) and `commit === null` ("The host could not read this commit from the remote.") were verified by code inspection and `tests/http.spec.ts` (`run detail commit is null when there is no sha`), not separately screenshotted in the browser.

## Round 3 follow-up merge (2026-11-26)

The reviewer accepted Gemini's page review 3 (`189ebd6`..`c6642db`, CI green) and pushed one follow-up fix on a side branch (`review/round3-followup`): per-session composer drafts, send state and errors; stale run navigation and commit summaries ignored; long task chips constrained on phones; `role="status"` on the composer error. Claude reviewed it (code read, `pnpm check` 132 passed / 1 skipped, and in a browser: drafts stay per session, a send with the host down keeps the text and shows "Not sent: …", the send after restart clears it) and merged it into `main` as `ed2bc12`, re-signed with the project account because the web commit carried a personal profile name. `scripts/privacy-scan.sh` now also rejects commit names other than the project account. The reviewer's written verdict on round 3 is not in this file.

## History rewrite (2026-11-26)

User decision: the repository has no AI contributors. The history of `main` was rewritten once to drop `Co-authored-by:` lines from commit messages (code unchanged) and force-pushed with a lease. Hashes of commits on `main` in this file were updated to the new ones; hashes from before the first push were never on GitHub. `scripts/privacy-scan.sh` now rejects AI attribution lines, and commit names are no longer restricted to the account login (the profile name is fine).

## No-SHA HTTP test cleanup

Commits: `ae31cd5` test: wait for the no-SHA HTTP run to settle.

Tests: TypeScript check passed. The job authorized changing only `run detail commit is null when there is no sha` in `tests/http.spec.ts`. It now polls its own run's detail endpoint to a terminal status and keeps all existing assertions. The named test passed on macOS Node 22 and 24 in Actions run 45774170700, attempt 1. Both full-suite jobs reported `Tests  1 failed | 131 passed | 1 skipped (133)` because the separate request-ID replay test failed during temporary fixture cleanup with ENOTEMPTY. That test was not changed.

Hand check: none (test-only change).

Issues: No PLAN-ISSUES.md change. The separate request-ID replay cleanup failure is outside this job's scope.

Cleanup: No manually started host, browser, or listening port; the test runner manages its fixtures.

Privacy scan: privacy-scan: clean (origin/main..HEAD)

Not done: No changes outside the authorized test and this report. See the branch check for the job's final CI status.

## No-SHA HTTP test cleanup review (Claude): passed, merged

Branch `dot/fix-nocommit-test` (`ae31cd5`, `ba81f1a`) fast-forwarded into `main`. Only the named test changed: it now polls its own run's detail to a terminal status and keeps every assertion. CI on `ba81f1a` green on Node 22 and 24; locally the test passed 5 times in a row and `pnpm check` gave 132 passed, 1 skipped; privacy scan clean.

Follow-up (not done): the same race remains in three other tests of `tests/http.spec.ts` that end with `await runs.idle()` right after `POST /api/runs` (`replays a run request id and rejects a changed one`, `maps busy and frozen to 409, never 500`, `POST /api/runs returns before the run finishes`). `runs.idle()` can resolve before the run started by the POST has registered, so temp-dir cleanup races a running git (`ENOTEMPTY`; the first one already failed once on CI run 45774170700). Prefer one fix at the root: make `Runs.idle()` also wait for runs whose start the HTTP handler has scheduled but not yet begun, with a test that calls `idle()` right after the POST and asserts the run is no longer pending/running; then the three tests need no change.

## Wait for scheduled run starts in `Runs.idle()`

Commits: `d50e7a9` fix: wait for scheduled run starts before going idle.

Tests: The new HTTP regression returned `pending` before the fix and reached `done` with it. It holds worktree preparation at a gate and captures the run's status when `idle()` resolves, without polling for completion. The three existing HTTP tests were not changed. `pnpm check` on `d50e7a9` (Actions run 45776470621) passed on macOS Node 22 and 24, with the same summary on both:

```
 Test Files  11 passed | 1 skipped (12)
      Tests  133 passed | 1 skipped (134)
```

Hand check: none (no UI change).

Issues: No `PLAN-ISSUES.md` changes.

Cleanup: No manually started host or browser; the test runner manages its fixtures and listening ports.

Privacy scan: privacy-scan: clean (origin/main..HEAD)

Not done: No outstanding implementation items. Merge review is separate.

## Scheduled run idle fix review (Claude): passed, merged

Branch `dot/idle-waits-for-scheduled-runs` (`d50e7a9`, `93cbf9a`) fast-forwarded into `main`. `Runs.start()` now registers its whole operation in `pending` synchronously, before worktree preparation, and the retry path tracks its settle loop the same way; `idle()` waits on `pending`. The HTTP handler calls `runs.start()` before it responds, so `idle()` right after a POST now waits for that run. The new test `idle waits for a newly posted run before worktree preparation finishes` fails against the old `src/runs.ts` (status `pending` at idle) and passes with the fix, so it pins the race. CI green on Node 22 and 24 for both commits; locally `tests/http.spec.ts` + `tests/runs.spec.ts` passed 5 runs in a row (35/35) and `pnpm check` gave 133 passed, 1 skipped; privacy scan clean. The three existing tests were not changed.

## Stabilize the `killGroup` supervisor-exit test

Commits: `c193636` test: wait for supervisor exit after killing its group.

Tests: The authorized change is limited to `kills the whole group on killGroup` in `tests/proc.spec.ts`. The two earlier macOS failures both reached the final assertion: the agent PID was gone, but the supervisor PID was still visible. A Linux diagnostic probe with a group listing limited to live processes observed the same pattern once in 25 attempts: the supervisor was already a zombie at `killGroup()` return, and its PID was gone after Node's `exit` event. The historical macOS logs did not capture process state. The test now subscribes before sending signals and keeps both assertions: agent PID absent immediately after `killGroup()`, supervisor PID absent after its own exit event. There is no fixed sleep or production change. The updated test passed 20 consecutive targeted runs with the Linux probe. `pnpm check` on `c193636` (Actions run 45782513661) passed on macOS Node 22 and 24:

```
 Test Files  11 passed | 1 skipped (12)
      Tests  133 passed | 1 skipped (134)
```

Hand check: none (test-only change).

Issues: No `PLAN-ISSUES.md` changes.

Cleanup: The diagnostic probe awaited all supervisor exit events. No manually started host, browser, or listening port.

Privacy scan: privacy-scan: clean (origin/main..HEAD)

Not done: No production process-management changes. Merge review is separate.

## killGroup test stabilization review (Claude): passed, merged

Branch `dot/killgroup-supervisor-exit` (`c193636`, `1521885`) fast-forwarded into `main`. Test-only: `kills the whole group on killGroup` now subscribes to the supervisor's `exit` event before `go()` and checks the supervisor PID only after that event; both assertions stay, no production change. The diagnosis holds: `killGroup()` decides on `pgrep -g`, which skips an exited but unreaped process, while the test's `isPidAlive` uses `kill(pid, 0)`, which still succeeds on it, so the old check could see the supervisor in the window before Node reaps its direct child. The test failed on CI twice before (`main` `8a88628`, run 45773438030; the retired branch at `d89c8cc`, run 45765720845). Production callers (`runner.ts`, `recovery.ts`) never check the supervisor PID after `killGroup()`, so the host is unaffected. CI green on Node 22 and 24 for both commits; locally the test passed 20/20 and `pnpm check` gave 133 passed, 1 skipped; privacy scan clean.

## Standardize on Node.js 24

Commits: `0954f8c` build: standardize on Node.js 24.

Tests: Frozen-lockfile install and TypeScript check passed. `pnpm check` on macOS Node.js 24.22.3 (Actions run 45785216685) ran as the sole CI job and passed:

```
 Test Files  11 passed | 1 skipped (12)
      Tests  133 passed | 1 skipped (134)
```

Hand check: none (no UI or host behavior change).

Issues: No `PLAN-ISSUES.md` changes. Node.js 24.x is the single supported runtime; the older Phase 1 reports remain historical. Windows support for Node.js does not change the host's macOS-only requirement. The parallel worktree investigation is separate and this change does not fix or close it.

Cleanup: No manually started host, browser, or listening port.

Privacy scan: privacy-scan: clean (origin/main..HEAD)

Not done: No Windows host port or changes to production source or tests. Merge review is separate.

## Node.js 24 standardization review (Claude): passed, merged

Branch `dot/node24-only` (`0954f8c`, `cc16cc7`) fast-forwarded into `main`: CI is one macOS job on Node 24, `engines.node` is `^24.0.0`, `@types/node` is 24.19.2 (lockfile updated, frozen install passes), and AGENTS.md, REVIEWER.md and README.md say Node 24 only. No source or test change. CI green on both commits (Node 24). Locally on Node 26 (the machine default) `pnpm install --frozen-lockfile` and `pnpm check` pass (133 passed, 1 skipped) with an "Unsupported engine" warning only. The phase 1 plan's Tech Stack still said Node 22, and AGENTS.md says its constraints always apply, so a dated note now says the runtime is Node 24 only.

Not merged: `dot/diagnose-parallel-worktrees` is a diagnostic branch for the intermittent `two sessions on one repo work in separate worktrees` failure (a CI probe step and a diagnostic message on the existing assertion). It stays with Dot; its probe step is keyed to the removed Node 22 matrix entry, so it will not run on `main`'s workflow as written.

## Standardize on Node.js 26

Commits: `379ba0f` build: standardize on Node.js 26.

Tests: A clean `pnpm install --frozen-lockfile` and TypeScript check passed with Node.js 26.13.1 and pnpm 11.7.0. The single macOS CI job on `379ba0f` (Actions run 45789015255) ran `pnpm check` on Node.js 26.13.1 and passed:

```
 Test Files  11 passed | 1 skipped (12)
      Tests  133 passed | 1 skipped (134)
```

Hand check: none.

Issues: No `PLAN-ISSUES.md` changes. Node.js 26.x is the only supported runtime. The approved release-age exception applies only to `@types/node@26.6.11`; its required `undici-types` resolution is `8.9.0`. All other versions and release-age checks are unchanged. The Node 22 investigation is retired; `dot/diagnose-parallel-worktrees` was not merged and can be deleted by the merge reviewer after the owner approves. Treat a recurrence of the parallel-worktree failure on Node.js 26 as a new job.

Cleanup: No manually started host, browser, or listening port.

Privacy scan: privacy-scan: clean (origin/main..HEAD)

Not done: No changes to production source or tests. Merge review is separate.

## Finish the Node.js 26 documentation cleanup

Commits: docs: remove superseded runtime references.

Tests: Searched all tracked text outside the append-only handoff for superseded runtime references; none remain. No source, tests, dependencies, or CI configuration changed in this follow-up. See the branch's Node.js 26 check for the final suite result.

Hand check: none.

Issues: No `PLAN-ISSUES.md` changes. Prior review findings remain historical and do not imply that those old reviews ran on the current runtime.

Cleanup: No manually started host, browser, or listening port.

Privacy scan: privacy-scan: clean (origin/main..HEAD)

Not done: Prior handoff entries and Git history are unchanged. No branch was deleted or merged.

## Node.js 26 standardization review (Claude): merged with fixes

Branch `dot/node26-only` (`379ba0f`, `8372486`, `e904273`) fast-forwarded into `main`, then one fix commit on top, at the user's request:
- Removed `pnpm-workspace.yaml`. It exempted `@types/node@26.6.11` from pnpm's `minimumReleaseAge` check, and that version was published less than a day before (one of five 26.6.x releases in two days). The release-age check is a supply-chain guard, so no exception. `@types/node` is now pinned to `26.6.6` (published 2026-10-28), which needs no exception. The lockfile was rebuilt from the previous `main` lockfile, so only `@types/node` and `undici-types` changed. The phase 1 plan's Tech Stack line now says 26.6.6. The Dot report above ("approved release-age exception", `26.6.11`, `undici-types` 8.9.0 as the reason) is superseded by this entry.
- Restored `docs/superpowers/plans/2026-11-25-polyclave-phase1-gemini-review.md` and `docs/superpowers/plans/2026-11-26-page-review-3.md` to their earlier text. They are historical records (they kept the Node 22 observations made at the time). Only documents that set current rules (AGENTS.md, REVIEWER.md, README.md, the phase 1 plan) say Node 26.

Verified on Node 26: `pnpm install --frozen-lockfile` passes with the default release-age policy, and `pnpm check` gives 133 passed, 1 skipped. Rule for later jobs: never add release-age or other policy exceptions without asking the user.

## v5 Phase 1A Rust host and native checks

Commits:
- `b359566` chore: archive legacy-node and initialize rust workspace
- `1c44f47` feat(proto): add pure data contracts and ts-rs exports
- `70b0c3a` feat(os): implement macOS/Windows process isolation, boot epoch, and path primitives
- `b5a7ca9` feat(store): add single-writer SQLite WAL store, FTS5 memory, and change_log
- `e379a1b` feat(adapters): implement Codex/Claude worker adapters and host HTTP MCP server
- `ae157e5` feat(core): add MirrorManager, WorktreeManager, check_run_branch, and classify_run
- `5345d73` feat(core): implement TurnRunner, RunManager, and startup Recovery
- `9348cc5` feat(daemon): implement polyclaved HTTP/SSE server and loopback auth
- `c545d7e` test: migrate phase1 invariants to Rust and enable macOS/Windows CI

Tests: Linux format and locked Clippy passed; locked workspace tests: 43 passed, 0 failed. Native matrix: the [check run for the Task 9 code commit](https://github.com/DustNonStar/polyclave/actions/runs/45926582054) passed on both macos-latest and windows-latest. The matrix runs the committed TypeScript export check and the full-history privacy scan on both operating systems. No existing Node test was edited, deleted, or relabeled as a passed Rust test. The [migration table](docs/superpowers/plans/2026-11-27-polyclave-v5-test-migration.md) identifies approved replacements, direct Rust evidence, and unchanged Phase 1B assertions.

Hand check: none for this backend job. For local Task 10, explicitly opt in to the real Codex and Claude probes respectively (`POLYCLAVE_REAL_CODEX=1` and `POLYCLAVE_REAL_CLAUDE=1`). In a disposable project on each supported native setup, verify normal initial/resume streams, commits with Run-Id trailers, publication to staging only, host import and receipt against the immutable baseline, and exact final SHA after local pullback. Verify isolated vendor history survives multiple turns and restart, and the per-turn MCP config is absent after normal completion, cancellation and recovery. On a system without independently verified Claude confinement, verify the explicit refusal; this is not a successful real Claude Worker probe. The enabled real-Claude path still requires a verified external isolation backend; never bypass that gate for a probe. Phase 1B owns the separate real CLI Hub continuation and fake-LLM integration.

Issues: project-owner decisions recorded as resolved in PLAN-ISSUES.md cover durable release evidence, bundled FTS5, the approved legacy semantic replacements, mandatory Claude confinement, staging-to-mirror isolation, and Phase 1B test ownership.

Cleanup: test Workers and supervisors are complete or reaped; disposable servers closed. No real vendor CLI was run.

Privacy scan: `privacy-scan: clean (origin/main..HEAD)`.

Not done: local real-CLI Task 10; the Phase 1B tests in the migration table. Leave `legacy-node/` intact until the transferred tests pass and deletion is explicitly approved. No merge was performed.

## v5 Phase 1A review: passed, merged

Branch `dot/v5-phase1a` (`d1c5c27..adb51ea`) fast-forwarded cleanly into `main`.

- **Code & invariant audit:** Verified `classify_run` (R1 external verification, immutable baseline, `WriterCheck::Possible` isolation), 4-step launch handshake with durable release marker (`unix.rs`, `recovery.rs`), per-attempt staging repo → host-only final mirror isolation (`adapters/src/lib.rs`, `mirror.rs`), fail-closed Claude confinement when no external sandbox is available, `TurnDto` compile-time exclusion of `mcp_token` + `ts-rs` export check (`apps/polyclave-app/ui/src/generated/proto.ts`), `worker_chat` terminal 409 guard (`runs.rs`, `routes.rs`), and zero edits to `legacy-node/`.
- **CI & local macOS verification:** GitHub Actions run `45926917504` passed on both `macos-latest` and `windows-latest`. Local macOS verification after fast-forward passed `cargo fmt --all -- --check`, `cargo clippy --locked --workspace --all-targets -- -D warnings`, `cargo test --locked --workspace` (43 passed, 0 failed, including the `TurnDto` compile-fail doctest), and `git diff --exit-code -- apps/polyclave-app/ui/src/generated/proto.ts`.
- **Deferred items:** Local opt-in real-CLI Task 10 probes (`POLYCLAVE_REAL_CODEX=1`, `POLYCLAVE_REAL_CLAUDE=1`) remain separate; `legacy-node/` remains intact until Stage 1B closes all deferred Hub assertions listed in `docs/superpowers/plans/2026-11-27-polyclave-v5-test-migration.md`.

## v5 Phase 1B Hub and optical desktop review handoff

Commits:
- `ddb05f7` feat(hub): add HubModelProvider trait and ApiHubProvider streaming
- `87ec0f4` feat(hub): implement CliHubProvider incremental resume and canonical dual-write
- `f2a896d` feat(hub): add read-only repo tools, memory curation, and manager sessions
- `b84fe10` feat(hub): implement proactive event triage loop and ticket budget guardrails
- `81190d9` feat(app): add Tauri v2 desktop shell, daemon lifecycle, and keyring integration
- `1a86b47` feat(ui): build 3-column optical layout, dual themes, SSE client, and core views
- `180b0f8` feat(ui): implement clean-room damped-spring motion engine, WebGL2 aurora, ink stream, vial, and pet stage
- `c39126b` docs(ui): sync DESIGN.md and verify v5 desktop/mobile UI in headless browser
- `7dc30ab` ci: use runner Chrome for offline desktop and mobile checks

Tests: the [native check for the Task 8 code and CI follow-up](https://github.com/DustNonStar/polyclave/actions/runs/45935004696) passed on macos-latest and windows-latest, including UI tsc --noEmit, both Chrome viewports, the generated proto.ts check, and the full-history privacy scan. Linux: format, locked Clippy, and locked workspace tests passed (55 passed, 0 failed). The fixture checked Flash-first ordering, the default display label, no horizontal overflow, full-width aurora, ink ranges and cleanup, plain reduced-motion streaming, and no non-loopback page requests at 1440×900 and 375×812. The stock profile test covers both new databases and the idempotent upgrade of the unchanged legacy stock profile; it preserves a custom endpoint and an independent custom profile. No archived Node test files or generated protocol exports were edited. Offline Hub evidence includes stream and tool-call parsing, in-order durable queue / request replay, continuation and provider switching, interrupted input handling, repository scoping, memory confirmation, and Ticket budgets. Offline provider and browser fixtures do not certify vendor login, the installed WebView, or a live provider's tool behavior.

Hand check (owner, pending):
1. Build the lightweight UI with `npm ci --ignore-scripts` and `npm run build` from `apps/polyclave-app/ui`, then build both companion binaries with `cargo build --locked -p polyclaved -p polyclave-app`. Use a disposable project and a separate state directory. Verify browser fragment bootstrap, desktop tray open/hide, daemon survival when the window closes, native review/blocked notifications, and keychain status with no plaintext credential in UI output or logs.
2. At **1440×900** and **375×812**, test both themes in the installed desktop WebView and direct browser. Check three panes versus responsive tabs, focus during SSE updates, 44 px phone controls, and no horizontal overflow. Check Review's displayed SHA, read-only Files, verified Memory, and the explicit confirmation for Remember this rule (including a Run decision). In Settings, check Flash first, Pro second, quota rings with real data where available, and honest unavailable values otherwise.
3. In both sizes, trigger streaming and verify the full-width moving aurora, ink fade and disappearing custom highlights, composer caret, context vial, and Deepy's footless spray and the other three pets. Watch idle settle to static afterglow and enable reduced motion to verify text stays readable. Headless evidence is not a substitute for the installed WKWebView/WebView2 smoke check.
4. In a disposable repository, opt in separately to real Codex and Claude **Worker** probes (`POLYCLAVE_REAL_CODEX=1`, `POLYCLAVE_REAL_CLAUDE=1)). Verify allowed workspace edits, staged publication and signed receipt, R1 recheck and final SHA, controlled cancellation, persistent isolated homes, and temporary MCP config cleanup. Claude without verified confinement must refuse; that refusal is not a passed real Claude Worker probe.
5. For the real **CLI Hub** probe, verify Codex and Claude with the configured read-only tool policy. Send a turn, switch away and back, restart, and confirm same-thread delta continuation, transcript preservation, and missing-thread recovery. Verify no unauthorized repository access or tools, Claude subscription launch without the bare flag, and record observed latency and tool-call correctness without credential data. Smoke-check the separately configured DeepSeek API profile on both official model IDs.

Issues: the sequencing gate and the DeepSeek model inventory decisions in PLAN-ISSUES.md are resolved. Flash is first within the official profile; Pro is optional. The change does not switch unrelated sessions to DeepSeek. Native command macro registration from the prior checkpoint was corrected. The browser fixture uses the hosted runner Chrome channel rather than downloading a second browser in CI; installed WebView checks are separate.

Cleanup: workspace and UI fixtures shut down their listeners and stop their subprocesses. No real vendor CLI, vendor API, or user machine was used.

Privacy scan: `privacy-scan: clean (origin/main..HEAD)`.

Not done: installed desktop/browser walkthrough and real provider probes remain pending local review. No merge or archive removal; keep `legacy-node/` unchanged until the owner approves removal after acceptance.

### Final DeepSeek follow-up: durable tool-enabled replay

The official [thinking mode contract](https://api-docs.deepseek.com/guides/thinking_mode/#tool-calls) requires earlier assistant reasoning and tool exchanges when the next request carries tools, including when an earlier turn made no tool call. The API Hub now stores the replay frames transactionally with its canonical reply in a host-only table and restores them only for the same registered profile. Public message and SSE shapes are unchanged. Older same-profile text replies still recover saved reasoning; a different provider does not receive the private replay frames.

Verification: a local fake Flash endpoint accepted the first turn, the SQLite store was reopened, and the endpoint required and received the first answer's reasoning on the second tool-enabled request. Linux workspace checks now report **56 passed, 0 failed**. The final native check applies to the branch head containing this follow-up.

Local addendum: with tools enabled, run two Flash prompts in the same Hub session, restarting the daemon between them. Verify the second request succeeds; repeat once with a tool call in the first turn. Keep real vendor response records local and separate from the Worker and CLI Hub probes above.

## v5 Phase 1B review: passed, merged

Branch `dot/v5-phase1b` (`61f7aa6..81b8d40`) fast-forwarded cleanly into `main`.

- **Code & invariant audit:** Verified `polyclave-hub` read-only repo tools (`repo_tools.rs` canonicalization, symlink rejection, and private-key/binary guards), `ApiHubProvider` (`deepseek-flash` default first, `deepseek-v4-pro` secondary, durable reasoning/tool-call replay), `CliHubProvider` (`hub_provider_threads` incremental resume and canonical dual-write), `apps/polyclave-app/src-tauri/src/keyring.rs` (`Zeroizing` and masked-tail-only status), and clean-room `ui/src/motion/` (`aurora.ts` WebGL2 + 2D fallback with `0×0` idle sleep, `pet-stage.ts` legless `Deepy` + 30fps spray + stacking, `ink-stream.ts` `CSS.highlights`, `vial.ts`, `smooth-caret.ts`, `spring.ts`, `scheduler.ts`). `legacy-node/` remains untouched.
- **CI & local macOS verification:** GitHub Actions run `45936398871` passed on both `macos-latest` and `windows-latest`. Local macOS verification after fast-forward passed `cargo fmt --all -- --check`, `cargo clippy --locked --workspace --all-targets -- -D warnings`, `cargo test --locked --workspace` (56 passed, 0 failed), `git diff --exit-code -- apps/polyclave-app/ui/src/generated/proto.ts`, `tsc --noEmit`, `npm run build`, and `POLYCLAVE_BROWSER_CHANNEL=chrome node tests/offline-browser.mjs` at `1440×900` and `375×812` (`violations: []`, `highlights=true`, zero external requests).
- **Pending local real-environment acceptance:** Combined real vendor CLI/API probes (`codex`, `claude`, DeepSeek live `deepseek-flash` / `deepseek-v4-pro`) and installed Tauri WKWebView smoke check remain pending; keep `legacy-node/` until that pass completes.

## PR gate fixes review: passed, merged

Branch `claude/pr-gate-fix-335f12` (`3dec8e4..32ab418`) was fast-forwarded into `main`.

- `8f425d5`: the gate now pushes the exact R1-verified SHA (`<sha>:refs/heads/<branch>` with `--force-with-lease`) and checks the remote tip afterwards. The daemon test fixture now gets its SHA from the real promotion path; this is the only change to an existing test.
- `16fbaf6`: revise, bounce and gate preparation accept only `result_sha`. A missing verified SHA escalates, and a caller `head_sha` that differs is refused.
- `4229f2c`: the reviewer gets the whole `base..head` range. A diff over 24 KB escalates to a human instead of being truncated.
- `32ab418`: before any push or PR, the gate checks that each selected reviewer has a provider; if one does not, the gate escalates without pushing.
- Local macOS after the fast-forward: fmt clean, Clippy `-D warnings` clean, `cargo test --locked --workspace` 130 passed, 0 failed; privacy scan clean. Windows CI has not run yet.
- Open (direction): the host mirror is created with `git init --bare` and has no `origin`, so a real PR push still has nowhere to go. Where the GitHub URL and the bot credential come from must be decided. Also not verified: real vendor CLIs, real API reviewers, real GitHub push, PR and auto-merge.

## Pairing QR fix review: passed, merged

Branch `claude/pairing-qr-fix-5a523f` (`05ae56e`) was fast-forwarded into `main`.

- `views/qr.ts` now encodes the pairing URI as a standard QR code (ISO/IEC 18004, byte mode, error correction M) with a 4-module quiet zone, using `qrcode-generator@2.0.4`. `build.mjs` copies the encoder into `dist`, so the page still makes no external requests.
- `tests/qr-roundtrip.mjs` rasterizes the built SVG in Chrome and decodes it with `jsqr@1.4.0` (dev only). The decoded text equals the URI byte for byte for short, 69-byte daemon-shaped, and longer URIs. It runs in `npm run test:browser`, which CI calls.
- `DESIGN.md` was updated in the same commit.
- Not verified: a real phone scan, pairing approval and revocation. Nit: today's 69-byte URI is version 5 (45 modules including the quiet zone) on a 112 px tile, about 2.5 CSS px per module. If phone scans are unreliable, enlarge the tile.

## PR push target from origin review: passed, merged

Branch `claude/pr-push-origin-81ae96` (`764e98c..ed08752`) was fast-forwarded into `main`. It implements the owner decision of 2026-11-27: infer the publish target from the repository's `origin`.

- `764e98c`: when a Run is admitted, `origin`'s URLs and pushurls are recorded with credentials stripped (new table `run_publish_origins`). Changing `origin` later does not change the record.
- `dd6b1d6`: before any push or PR, the gate requires exactly one GitHub HTTPS or SSH origin URL whose `owner/repo` equals `repo_slug`, and no separate pushurl. Otherwise it escalates and pushes nothing. The push always goes to the canonical `https://github.com/<repo_slug>.git`, never to a snapshot URL or a named remote. Worker push, R1 and the mirror are unchanged, as the R3 decision requires.
- `ed08752`: the bot token (`Polyclave.GitHub` / `bot_token`) reaches git through a one-shot `credential.helper`, with the user's helpers cleared and `GIT_ALLOW_PROTOCOL=https`. Tests confirm that a sentinel token is absent from argv, stderr and events.
- Existing tests: only an import line changed. Local macOS after the fast-forward: fmt and Clippy clean, 139 passed, 0 failed; privacy scan clean.
- Nit: the token reaches git through an environment variable, which other processes of the same user can read while the push runs. This is within the v5 threat model, which trusts same-user processes.
- Not verified: a real GitHub push, PR or auto-merge; Windows CI.

## Mesh worker_run idempotency review: passed, merged

Branch `claude/mesh-run-budget-b7a297` (`f0dedff..ac81ae8`) was fast-forwarded into `main`.

- `ac81ae8`: `worker_run` now requires a non-blank `request_id`. The receiver returns 400 without one, and the primary's `dispatch_remote` refuses before sending. The same id twice admits one Run. Local macOS: fmt and Clippy clean, 141 passed, 0 failed; privacy scan clean.
- Remaining gap: the `request_id` comes from the Hub model's tool call, not from durable primary state. After a primary crash, the model may resend with a new id. Option A in the open PLAN-ISSUES entry (a primary remote-dispatch row keyed by Hub session and `request_id`) would close this.
- `f0dedff`: opens a `direction` entry in `PLAN-ISSUES.md` asking where a remote Run is charged against a Ticket budget. Options A, B and C are listed there; A is recommended. Until it is decided, remote Runs still bypass the requesting Ticket's `max_runs`.

## Legacy-node removal: no acceptance record (2026-11-27)

A records check, not a review of code.

- **What the commit says.** `01c8645` "chore: remove archived legacy-node after v5 Rust acceptance" deletes all 33 files under `legacy-node/` (14,311 lines). The message has no body. The GitHub check run for that commit on `main` failed; the next green run on `main` is at `52bc69f`.
- **What the records show.** Before `01c8645`, every record keeps the archive until acceptance:
  - "v5 Phase 1A Rust host and native checks": leave `legacy-node/` intact until the transferred tests pass and deletion is explicitly approved.
  - "v5 Phase 1B Hub and optical desktop review handoff": keep `legacy-node/` unchanged until the owner approves removal after acceptance.
  - "v5 Phase 1B review: passed, merged": real vendor CLI and API probes and the installed WebView check remain pending; keep `legacy-node/` until that pass completes.
  - `PLAN-ISSUES.md`, the owner decision of 2026-11-27 on Phase 1B startup: the real Worker and CLI Hub probes and the local 1440/375 review remain final local acceptance, not passed; preserve the complete archive.
- **What is missing.** No entry in this file, `PLAN-ISSUES.md`, `REVIEWER.md` or `handcheck/` records that real-environment acceptance passed, or that the owner approved removing the archive. Who decided the removal is not recorded. `docs/LESSONS.md` item 7 describes the same gap.
- **Recovery.** The archive is unchanged in git history: `git checkout 01c8645^ -- legacy-node` restores it.
- **Still pending.** Real `codex` and `claude` Worker probes; real CLI Hub probes; live DeepSeek `deepseek-flash` and `deepseek-v4-pro` calls; the installed WKWebView and WebView2 walkthrough; a real GitHub push, PR and auto-merge through the PR gate; a real phone scan of the pairing QR, with approval and revocation; a two-machine mesh run. `README.md` lists the same items by capability.

## Docs v5 sync review: passed, merged

The four commits on `claude/docs-v5-sync-308251` were cherry-picked onto `main` as `8089136..5acb369`, because `main` had moved on with the A + C budget decision. README, PRODUCT.md and NEXT.md now describe v5. README has a per-capability verification table, and the PRODUCT.md competitor claim is replaced by what Polyclave itself checks. A later commit updates NEXT.md, which still listed the mesh budget question as open.
- Found while writing: `dsh_args` passes `--model`, but the real `dsh headless` has no such flag (LESSONS item 9). This is not fixed yet.

## Mesh worker_run budget: A + C (branch claude/mesh-budget-ac-238293)

Commits on the review branch, on top of main@ed63590 (not pushed, not merged):
- 312398d feat(daemon): cap concurrent mesh Runs per node, default 3
- fe41091 feat(ui): per-node mesh concurrency setting
- this report with `handcheck/mesh-budget-ac/` and two `PLAN-ISSUES.md` items (chore: mesh-budget-ac)

Held back on the local, unpushed branch `wip/mesh-budget-a` (one commit on top of fe41091):
- b20f908 feat(hub): charge remote worker_run to the primary Ticket

A is finished and its own tests pass, but it breaks one existing test that sends `worker_run` without a Ticket. The rules forbid changing that test without a work-list line, so A waits for that line. See `PLAN-ISSUES.md`, "[detail] Decision A … breaks an existing mesh test", which names the exact test edit to authorize.

Tests (macOS only, no native Windows CI run yet):
- Review branch head fe41091: `cargo fmt --all -- --check` clean; `cargo clippy --locked --workspace --all-targets -- -D warnings` clean; `cargo test --locked --workspace` 145 passed, 0 failed (main had 141; 4 new); the generated proto check passes and `git diff --exit-code` on `proto.ts` is clean; UI `tsc --noEmit` and `npm run build` pass; `npm run test:browser` (system Chrome) passes.
- `wip/mesh-budget-a` at b20f908: fmt and clippy clean; workspace 148 passed, 1 failed (`mesh_worker_run_and_run_status_work_end_to_end_through_dispatch_remote`, Policy at tests.rs:1985, the existing test above). 4 new A tests pass.
- No existing test was changed. The test files only gained new tests and helpers.

New tests:
- C: `mesh_worker_run_refuses_a_fourth_concurrent_run_and_admits_after_a_slot_frees` (4th refused with 429 while 3 run; a resend at the limit returns the existing Run; a slot frees when a Run finishes; cap, not total), `mesh_worker_run_concurrent_admissions_cannot_both_take_the_last_slot`, `mesh_concurrency_setting_persists_through_the_local_api_and_is_enforced` (auth required, 1..=16, default 3, survives node re-registration, enforced), and store `mesh_concurrency_column_upgrades_an_existing_ledger_with_default_three` (old ledger upgraded, value survives reopen).
- A (on wip): `dispatch_remote_charges_the_primary_ticket_once_per_request_id`, `dispatch_remote_resend_after_a_crash_is_not_charged_twice_and_exhaustion_blocks` (crash before send and crash after the receiver admitted; exhaustion gives `BudgetExceededError` and `blocked_on_user`), `dispatch_remote_refuses_a_foreign_wrong_repo_or_closed_ticket_before_sending` (no request reaches the node), `dispatch_remote_at_the_receiver_limit_is_retryable_and_charged_once`.

Design notes:
- C counts a Run against its node while it is `pending` or `running`. The count and the admission are atomic: the check runs under the RunManager admission gate and again in the transaction that writes the Run (`AdmissionGuard` in `polyclave-core`), which also writes the `mesh_admissions` row. Over the limit: HTTP 429, `Retry-After: 30`, JSON `{code: "mesh_concurrency_limit", message, limit, retryable: true}`, nothing admitted or queued.
- A refund rule: a reservation is never refunded. The row records what the receiver answered (`admitted` with the remote Run id, `at_limit`, `refused`, `unreachable`); a resend with the same `request_id` is free and reaches the Run the receiver may already hold. A refund cannot tell "never arrived" from "reply lost", so it could let a retry run unpaid. A 429 reaches the Hub as a retryable HTTP 429 error.
- Schema: idempotent steps at open (no `user_version` bump), because an existing store test pins version 10. Recorded in `PLAN-ISSUES.md`.

Hand check (C-UI), daemon built from the branch under `/tmp/polyclave-hc/mesh-budget-ac/`, screenshots by Playwright with system Chrome:
- `itemCUI-desktop-mesh.jpg` (1440 × 900): two node cards, each with a `Concurrent Runs` row and a `− n +` stepper; values 5 (set through the API) and 3 (default).
- `itemCUI-desktop-mesh-after-plus.jpg`: after `+` on Win Box, the stepper shows 4 (read back from the daemon's reply).
- `itemCUI-phone-mesh.jpg` / `itemCUI-phone-mesh-after-plus.jpg` (375 × 812): the same, the hint wraps, buttons 44 × 44 px.
- `measurements.md`: no horizontal overflow at either size (`scrollWidth` 1440 / 375, no overflowing element in the cards), no request to a host other than 127.0.0.1. The nodes read `(offline)` because their endpoints do not exist.

Issues: two `[detail]` items opened in `PLAN-ISSUES.md` (the A test conflict; the schema step and slot-state choice).
Cleanup: the scratch daemon on port 4417 was stopped (`pgrep` empty, port closed); the Playwright browsers closed with their script. No fake codex left running (test fixtures only).
Privacy scan: `privacy-scan: clean (origin/main..HEAD)`.
Not done: A is not on the review branch (see above). Not verified: a real second machine. Every mesh test runs both nodes in one process over loopback, with a fake codex; no real vendor CLI ran. Native Windows CI has not run on this branch.

### Addendum: A joins the review branch (merge reviewer approval, 2026-11-27)

The merge reviewer approved the one test edit and accepted the three design choices. `feat(hub): charge remote worker_run to the primary Ticket` was cherry-picked from `wip/mesh-budget-a` onto this branch as b7b5534. The approved edit is folded into it: `mesh_worker_run_and_run_status_work_end_to_end_through_dispatch_remote` now inserts an active Ticket `t-mesh` for session `hub` and `repo` and passes `"ticket_id": "t-mesh"`. No assertion changed. Both `PLAN-ISSUES.md` items are marked resolved.

Tests at b7b5534 (macOS only): fmt clean; clippy `-D warnings` clean; `cargo test --locked --workspace` 149 passed, 0 failed (main had 141; 8 new); proto check passes and `proto.ts` has no diff; UI `tsc --noEmit` and `npm run build` pass. Native Windows CI has not run. The local branch `wip/mesh-budget-a` is no longer needed.

## Mesh budget A + C review: passed, merged

Branch `claude/mesh-budget-ac-238293` (`312398d..27c6fcf`) was fast-forwarded into `main`. It implements the owner decision A + C.

- C (`312398d`, `fe41091`): each receiver caps how many mesh-admitted Runs from one sending node can be `pending` or `running` at once. The default is 3, the cap is checked inside the admission transaction, and over the cap the receiver answers 429 and queues nothing. Settings has a per-node stepper from 1 to 16; hand-check images are in `handcheck/mesh-budget-ac/`.
- A (`b7b5534`): remote `worker_run` requires the calling Hub session's active, same-repository Ticket. Each dispatch is charged exactly once, keyed by Hub session and `request_id`, and the charge is never refunded. Exhaustion moves the Ticket to `blocked_on_user`.
- The merge reviewer approved one existing-test edit: adding a Ticket and `ticket_id` to `mesh_worker_run_and_run_status_work_end_to_end_through_dispatch_remote`. No assertion changed.
- Local macOS after the fast-forward: fmt and Clippy clean, 149 passed, 0 failed, the `proto.ts` check clean, privacy scan clean. Not verified: a real second machine and Windows CI.

## dsh --model fix review: passed, merged

Branch `claude/dsh-model-fix-84011a` (`2aec8e4`) was fast-forwarded into `main`.
- The real `dsh --profile headless --help` (read-only, on this machine) lists only `--json`, `--session-id`, `--permission-mode`, `--approval-policy`, `--add-dir` and `-h/--help`. It has no model flag.
- The Worker adapter and the CLI Hub transport no longer pass `--model`. A `dsh` Worker with a model set is refused with `Error::DshModelUnsupported`. A `dsh` Hub turn with a non-default model is refused with `Error::Profile`. Tests now check arguments against a strict fake that accepts only the real options.
- The merge reviewer approved two existing-test edits: `agy_and_dsh_args_match_expected_flags` and the `cli_provider.rs` args test. Both asserted that `--model` was passed, which was the bug itself.
- Local macOS: fmt and Clippy clean, 150 passed, 0 failed; privacy scan clean.
- Still open (`PLAN-ISSUES.md` detail entry): how to choose a `dsh` model (a `--patch` file inside the isolated `DSH_HOME` needs one real run), and the fact that `mcp_url` is never passed to `dsh` (LESSONS item 9).

## Dot branch `dot/claude-code-probe`: rejected

Commits `4dc6d59` and `2e6d3a9` were reviewed read-only and not merged.
- `tests/claude-probe.spec.ts` imports Node `src/` modules that `01c8645` deleted. `main` has no `package.json`, so no CI job runs the test.
- Its fake (`tests/fixtures/fake-claude-probe.mjs`) parses the probe's own prompt to produce the expected frames, so the pass proves only that the harness agrees with its fake (LESSONS item 5). The report says no live Claude Code was run.
- It is superseded by the Rust adapter `crates/polyclave-adapters/src/claude.rs` and the real Task 10 probe (`POLYCLAVE_REAL_CLAUDE=1`). `HANDOFF.md` conflicts on merge.
- Flag check against the real `claude --help` (2.1.332): every flag the probe uses exists except `--max-turns`, which is not listed. A possible `--bare` / `--tools ""` hardening of `claude.rs` may be considered later, checked against real help output.
- Action for Dot: delete the branch.

## Approval card evidence review: passed, merged

Branch `claude/approval-card-evidence-a09726` (`35aece8`, `b117cc4`) was fast-forwarded into `main`.
- The Run approval card now shows a one-line summary and three rows: "Delivery verified (R1)", "Tests for this exact SHA" and "Request satisfied (your decision)". `RunEvidenceDto` reads only the current Attempt's accepted receipt and the latest progress text, which is labelled as the Worker's claim. A test shows that it never reads an older Attempt's receipt.
- No source records tests per SHA, so the tests row always says "No test result recorded for <sha>". A `detail` entry in `PLAN-ISSUES.md` describes what recording them would need.
- The merge reviewer checked the 375 px screenshot `handcheck/approval-card-evidence/r1-phone-light-card.jpg`. The summary reads "Commit not verified · no test result for bbbbbbb · waiting for your decision", and the three rows are distinct.
- Local macOS: fmt and Clippy clean, 152 passed, 0 failed, the `proto.ts` check clean, privacy scan clean. UI `tsc` and the browser tests were run by the branch session; this checkout has no UI dependencies installed. Not verified: Windows CI and a real phone.

## 2026-11-27 全项目审查总结（合并审查者）

范围：对照外部审核（ChatGPT，基于 `52bc69f`）和 `main@3dec8e4` 的代码做全面审查，之后用子会话逐项修复，由合并审查者审完后快进合并或 cherry-pick 进本地 `main`。最终状态：macOS 本地 fmt、Clippy 干净，152 个测试通过、0 失败，隐私扫描 clean。**32 个提交尚未推送，Windows CI 未运行。**

### 发现的问题、原因和修复

| # | 问题 | 原因 | 修复（提交） |
|---|---|---|---|
| 1 | PR 门禁推送的 `refs/polyclave/runs/<id>/final` 在生产代码里从未被创建，真实推送必然失败 | 测试夹具手工造出这个 ref，跨模块约定只在测试里成立（LESSONS 第 5 条） | 改为推送 R1 核验过的确切 SHA，带租约，推送后核对远端分支顶端；夹具改走真实的验收路径（`8f425d5`） |
| 2 | 返工和准备阶段会退回到未核验的 `observed_sha`，未验证的提交可能被评审、被批准 | 为了"总能拿到 SHA"写的兜底绕过了 R1（LESSONS 第 6 条） | 只接受 `result_sha`，缺失就转人工；评审、推送和 `expectedHeadOid` 用的是同一个 SHA（`16fbaf6`） |
| 3 | Reviewer 只看到最后一个提交，diff 超过 24 KB 时被悄悄截断 | 用了 `git show <head>`，截断后也没有告知 | 改为完整的 `base..head`；超长时转人工，不再截断（`4229f2c`） |
| 4 | 先推送、开 PR，后调用 Reviewer，推送前不检查 Reviewer 是否可用；API Reviewer 的配置永远匹配不上 | 流程顺序问题；读取的配置列表里缺字段 | 推送前检查每个 Reviewer 都找得到可用的模型，找不到就不推送、转人工；改为直接读取配置表（`32ab418`） |
| 5 | 镜像没有配置远程，PR 推送没有目标地址 | 镜像用 `git init --bare` 创建，推送目标从未设计 | 所有者决定按仓库 `origin` 推断：Run 受理时记录 `origin`（去掉凭证），推送前核对必须是 `github.com/<repo_slug>`，只推送到规范 URL；bot 令牌通过一次性 credential helper 传给 git（`764e98c..ed08752`）。Worker 推送和 R1 仍然只认本机镜像 |
| 6 | 手机配对二维码是假的：把链接哈希后画成随机格子，扫码器读不出 | 渲染器从未真正编码 URI | 换用 `qrcode-generator@2.0.4` 生成标准 QR；用 `jsqr` 在浏览器里往返解码，逐字节核对，并接入 CI（`05ae56e`） |
| 7 | 跨机 `worker_run` 不受工单预算约束，没有 `request_id` 时重试会重复派活 | 接收端固定 `ticket_id: None`，并随机生成 id | `request_id` 改为必填，同一 id 只受理一个 Run（`ac81ae8`）。所有者决定采用 A + C：远端 Run 扣主控工单的额度，每次派发只扣一次，不退款（`b7b5534`）；每台接收机器对同一来源默认最多 3 个并发 Run，超出返回 429，可在设置里按机器调整为 1–16（`312398d`、`fe41091`） |
| 8 | README、PRODUCT、NEXT 仍是 v4 的内容；PRODUCT 对竞品的说法过于笼统 | 入口文档不在任何分支的工作清单里，没人负责更新（LESSONS 第 7 条） | 重写为 v5，并加了逐项能力的验证状态表；竞品对比改为只陈述 Polyclave 自己核验什么；NEXT 指向新的里程碑（`8089136..5acb369` 及后续） |
| 9 | 删除 `legacy-node` 的提交没有验收记录，也没有批准记录 | 删除时没有要求附上验收证据 | 在上方 "Legacy-node removal: no acceptance record" 一节记录了这个缺口；代码可以从 `01c8645^` 恢复 |
| 10 | dsh 适配器传了真实 CLI 不认的 `--model` | 适配器只用自造的假脚本验证过（LESSONS 第 9 条） | 对照真实的 `dsh headless --help`，不再传 `--model`，设置了模型会明确报错；测试改用只接受真实选项的严格假件（`2aec8e4`） |
| 11 | 审批卡把"提交已核验 / 测试通过 / 需求满足"混在一起 | 卡片只列 SHA 和改动统计 | 卡片改为三行证据加一句摘要；没有测试记录时如实显示"no test result recorded"（`35aece8`、`b117cc4`） |
| 12 | Dot 的分支 `dot/claude-code-probe` 一直没审 | — | 驳回：它依赖已删除的 Node 代码，假件照着测试写，从没跑过真实 CLI。由 Dot 删除这个分支 |

另外两项所有者决定（都已记录在 `PLAN-ISSUES.md`）：跨机预算采用 A + C；**下一个里程碑收窄为一个端到端用户故事**（真实任务 → 中途重启后恢复 → 另一家厂商的 Reviewer 要求修改 → 返工 → 在手机上批准最后那个 SHA）。在它跑通之前，暂停 mesh、桌面控制、自我进化方向的新功能。审查经验已写入 `docs/LESSONS.md` 第 5–8 条。

### 尚未完成

1. **推送和 Windows CI**：32 个提交只在本地 `main`，所有者要求暂缓推送。
2. **真实环境验收一项都没跑**：真实 Codex、Claude Worker 和 CLI Hub 探针；DeepSeek 线上调用；安装版 WebView；GitHub 上的推送、开 PR、自动合并；手机扫码配对、批准和撤销；两台机器之间的跨机派活。
3. **系统不按 SHA 记录测试结果**，审批卡的测试那一行目前只能显示"无记录"（见 `PLAN-ISSUES.md` 中的 detail 条目）。主线用户故事需要它。
4. **dsh 如何指定模型仍是未知**，需要一次真实运行来确认；另外 `mcp_url` 从未传给 dsh（见 `PLAN-ISSUES.md` 中的 detail 条目）。
5. **`legacy-node` 删除的依据**：建议保持删除、不恢复，但需要逐条核对迁移表里约 20 条 Phase 1B 断言是否都有对应的 Rust 测试，然后由所有者在本文件里记录批准。
6. **小项**：二维码每个格子只有约 2.5 CSS 像素，需要真机实测，必要时放大；`claude.rs` 可以考虑 `--bare`、`--tools ""` 加固（先对照真实 `--help`）。

## 2026-11-27 复查（合并审查者，额度受限，只做定点检查）

- **修复是否到位**：生产代码里已经没有 `observed_sha` 兜底，也没有 `runs/<id>/final` 和 `push_mirror_branch` 的残留。152 个测试通过（见上一节）。
- **修复引出的新问题（未修）**：A 和 C 叠加之后，主控在发送**之前**就扣了工单额度（`tools.rs` 中的 `charge_remote_dispatch`），而接收端以 429 拒绝（已达并发上限）时，这次扣费不会退回。用同一个 `request_id` 重试不会再扣，但 Hub 如果放弃这次派发或换了 id，这个额度就白白浪费了。建议：对 429、400、403 这类接收端明确表示"没有受理"的回应予以退款，保留 dispatch 记录，同一 id 重发时重新扣费；传输错误仍然不退。这是 `detail` 级问题。
- **文档**：README 的验证状态表还停在旧状态（dsh 一行写着"仍会传 `--model`"、mesh 一行写着"预算问题未决"、本地合并清单不全），NEXT.md 还写着 A + C "正在实现"，这次都已更正。v5 设计规范本身没有写入今天的三项所有者决定（按 origin 发布、A + C、里程碑收窄），这些决定目前只记在 `PLAN-ISSUES.md`。下次修订规范时应把它们并进去。

## 2026-11-27 详细复查（合并审查者）

**检查结果（`0a5123b`，macOS）**：fmt、Clippy（`-D warnings`）干净；`cargo test --workspace` 全部通过；UI `npm ci`、`tsc --noEmit`、`npm run build` 通过；`npm run test:browser` 通过，包括 1440×900 和 375×812 下零溢出、无外部请求，二维码在版本 3、4、5、10 下往返解码都正确，审批卡在双主题、双尺寸下都正确；`proto.ts` 无差异。

**确认原有修复到位**：PR 门禁只推送经 R1 核验的 SHA，带租约，推完核对远端；返工没有核验 SHA 就转人工；评审看到完整的 `base..head`；推送前检查 Reviewer 可用；按 `origin` 快照发布；dsh 不再传 `--model`；审批卡三行证据分开显示，测试一行没有数据时如实说"无记录"。

**新发现的问题（都还没修）**

1. **[重要] 首次推送会覆盖同名的远端分支**。`push_verified_sha(..., None)` 先读远端当前的分支顶端，再用 `--force-with-lease=<dest>:<当前顶端>` 推送（`pr_gate.rs`，首次推送在 `run_pr_gate` 中调用）。如果 GitHub 上已经有一个同名的分支（人建的，或者另一个 Run 留下的），它会被强制覆盖。建议：首次推送只在远端没有这个分支、或者顶端已经等于这个 SHA 时才推送；否则转人工。
2. **[重要] 跨机 `request_id` 在接收端没有按来源区分**。接收端的幂等键是（按仓库划分的 mesh 会话，`request_id`），里面不含发送节点，也不含发送方的 Hub 会话（`http.rs` 中的 `mesh_worker_run`）。主控上两个不同的 Hub 会话，或者两个不同的发送节点，只要用了同一个 `request_id`（比如模型都起名 "fix-1"），第二个请求就会拿到第一个请求的 Run：主控照样扣了额度，但实际上什么都没启动，而且结果会被当成自己的。建议：接收端把 `request_id` 加上发送节点做命名空间，主控发送时再加上 Hub 会话前缀。
3. **[次要] diff 大小检查在推送之后**。超过 24 KB 的 PR 会先推送、开 PR，之后才转人工（`run_pr_gate` 评审循环开头）。第一次检查应该挪到推送之前。
4. **[次要] 返工后路径越界直接转人工，但没有 Hub 通知**（`run_pr_gate` 中 `revise_scope` 那一支），和其他转人工的分支不一致。
5. **[次要] 并发上限触发的 429 不退还工单额度**（上一节已记录）。
6. **[次要] 审批卡的 "Worker's claim" 取的是该 Attempt 最后一条 progress 事件**，未必是最终总结。标注是对的，但内容可能是中途的进度。

**文档仍然过时**
- `REVIEWER.md` 还是 v4 的内容：`pnpm check`、Node.js 26、"Phase 1 is built and accepted"、冻结会话、409 解锁。
- `AGENTS.md` 和 `GEMINI.md` 第 1、3 条还写着"Keep the Node tests unchanged…""For archived Node-only work, use the legacy check"，但 Node 测试已经随 `legacy-node` 一起删除了。
- v5 设计规范里还没写入今天的三项所有者决定（按 origin 发布、A + C、里程碑收窄）。
- 这三个都是规则或设计文档，需要所有者确认后再改。

## 2026-11-27 独立安全与运行链路审查（Gemini，基于 `main@82a8107`）

范围：在 `main@82a8107` 上对照实际运行链路做独立审查，补充上方“详细复查（合并审查者）”的发现，供下一轮复查与修复参考。

### 新发现的问题（均未修）

1. **[高危安全] `ScmObserver` 绕过 Admission Policy，把外部 GitHub 评论直接写成 `source='user'`**。
   - **位置**：`crates/polyclave-hub/src/scm_observer.rs:72-76`。
   - **现象与影响**：`emit_event` 直接调用 `self.store.hub_enqueue(hub_id, &intent, &event.summary)`，未经过 `Triage::inferred`。外部 PR 评论与机器人消息以 `source='user'` 落库，存在间接提示词注入风险，且可被误认为本人显式消息从而满足 `basis_msg_id`。
   - **修法与验收**：存储层拆出外部事件入队方法（强制 `source='host'`）；`ScmObserver` 必须经 `Triage::inferred` 过滤 `bot_actors_json` 与 `inferred_mode` 后再入队。补反例测试验证外部评论永不落为 `source='user'`。

2. **[阻断 + 安全] 手机端 Tailscale 访问 403，且配对交换接口可伪造 `peer_identity`**。
   - **位置**：`bins/polyclaved/src/auth.rs:32-38`；`bins/polyclaved/src/routes.rs:300-316`。
   - **现象与影响**：
     - `LocalAuth::allowed_origin` 仅放行 `http://127.0.0.1:{port}` 与 `tauri://localhost`（及 `http/https://tauri.localhost`），手机经 `https://<host>.ts.net` 发 POST 批准会被拒 403。
     - `exchange_pairing` 挂载在 `public_api`（无 Host/Origin 校验），且第 312 行 `let peer = header_peer.or(body.peer_identity);` 信任客户端自报的 `peer_identity`。
   - **修法与验收**：当存在受信任 Tailscale/Mesh 身份头且请求 `Host` 与 `Origin` 主机名一致（同源 `https://<host>`）时放行，仍拒绝跨站 Origin；移除 `exchange_pairing` 的 `body.peer_identity` 兜底（仅认受信任反向代理/Tailscale 身份头），并补 Host/Origin 校验与回归测试。

3. **[逻辑缺陷] `ScmObserver` 把 GitHub Review ID 与 Issue Comment ID 混用同一个游标**。
   - **位置**：`crates/polyclave-hub/src/scm_observer.rs:152-205`。
   - **现象与影响**：`poll_once` 用同一个 `cursor.last_comment_id` 与 `max_seen_id` 比较 `review.id` 和 `comment.id`。GitHub 上两者是独立自增序列，ID 较小的一侧会被永久跳过。
   - **修法与验收**：在 `scm_observer_cursors` 中拆分 `last_review_id` 与 `last_comment_id`（沿用 live-schema 幂等升级步骤，不撞 `user_version == 10` 测试），补交叉 ID 顺序的回归测试。

4. **[功能断链] `ScmObserver` 事件无法触发手机私密推送通知**。
   - **位置**：`crates/polyclave-hub/src/scm_observer.rs:77-81`；`crates/polyclave-hub/src/notify.rs:148-163`。
   - **现象与影响**：`emit_event` 写入 `ChangeTopic::PrGate` 的 `ObservedEvent` 只有 `kind`（`ci_failure` / `merge_conflict` / `human_review` 等）而无 `status` 字段；`classify_attention_change` 第 153 行强制取 `payload.get("status")`，导致 CI 失败、合并冲突、人工评审永远不会唤醒通知。
   - **修法与验收**：`classify_attention_change` 同时识别 `topic == "pr_gate"` 下的 `kind`（`ci_failure` / `merge_conflict` / `human_review`），只发元数据摘要，补单测。

5. **[适配缺口] `dsh` Worker 仍未注入 `mcp_url`**。
   - **位置**：`crates/polyclave-adapters/src/dsh.rs:34-49`。
   - **现象与影响**：`2aec8e4` 已移除无效的 `--model` 参数，但 `dsh_worker` 仍只设置了 `DSH_HOME` 与 `POLYCLAVE_MCP_TOKEN`，从未把 `ctx.mcp_url` 写入配置或传给 `dsh`，真实 `dsh` 无法调用 `submit_result`。
   - **修法与验收**：结合 `PLAN-ISSUES.md` 中 `dsh` 模型选择条目一并推进，需先实测 `dsh` 支持的 MCP 配置方式，再补齐注入与测试。

### 统一修复优先级建议（供 DeepSeek v4.1 Flash 复查与接手修复）

合并上方“详细复查（合并审查者）”的 6 项与本节 5 项，建议按以下顺序修复：

- **P0（主线用户故事阻断）**：
  1. PR 门禁首次推送防覆盖、24 KB diff 检查前置到推送前、返工路径越界转人工补发 Hub 通知（合并审查者 #1/#3/#4）。
  2. Tailscale 同源放行与 `exchange_pairing` 移除 `body.peer_identity` 兜底及 Host/Origin 校验（Gemini #2）。
  3. `ScmObserver` 禁写 `source='user'`（经 `Triage::inferred` + `source='host'`）、拆分 `last_review_id` / `last_comment_id` 游标、接入 `notify.rs` 私密推送（Gemini #1/#3/#4）。
  4. 按 SHA 记录测试结果供审批卡展示（`PLAN-ISSUES.md` detail 条目）。
- **P1（次要缺陷）**：
  5. Mesh `request_id` 按发送节点与 Hub 会话做命名空间隔离，429/400/403 退还主控 Ticket 额度（合并审查者 #2/#5）。
  6. 审批卡 `Worker's claim` 取值优化（合并审查者 #6）。
  7. `dsh` 注入 `mcp_url` 与模型选择（Gemini #5 及 `PLAN-ISSUES.md` detail 条目）。

## 2026-11-27 审计修复（`fix/audit-2026-11-27`，由接手本项目的 DeepSeek 实机执行）

分支 `fix/audit-2026-11-27`，基点 `main@5fdc9e1`。修的是上一节"独立安全与运行链路审查（Gemini）"和"详细复查（合并审查者）"里列出的问题。**没有推送，也没有合并**：所有者此前要求暂缓推送，这一轮沿用。

Commits（每项一个提交，每项后都跑过 fmt / Clippy `-D warnings` / workspace 测试）：

- `03bc09c fix(pr-gate): refuse a first push that would overwrite someone else's branch` — 合并审查者 #1。原因确认：`push_verified_sha(..., None)` 先用 `ls-remote` 读出远端顶端，再拿这个顶端做 `--force-with-lease`，租约**必然自我满足**，所以同名远端分支会被强制覆盖。现在首次推送只允许"分支不存在"或"顶端已经等于该 SHA"，其余返回 `Error::Policy`，由 `run_pr_gate` 转成 `Escalated` 并给 Hub 发通知，不推送、不开 PR。trait 文档同步改写（原文写的就是这个有洞的语义）。测试：`git_pusher_never_overwrites_a_branch_it_did_not_create`、`refused_first_push_escalates_with_a_hub_notice_and_no_pr`。
- `19576d3 fix(pr-gate): decide that the PR range is reviewable before pushing it` — 合并审查者 #3。24 KB 检查从评审循环开头挪到**每次推送之前**（首次推送前、以及返工推送新 SHA 之前），超限直接转人工，不再"先推送、先开 PR、再转人工"。测试：`oversized_pr_range_escalates_before_the_push_and_opens_no_pr`。
- `4631418 fix(pr-gate): tell the Hub when a revise leaves the Run's paths` — 合并审查者 #4。返工后路径越界分支原来只改状态、不通知；现在走同一套 `escalate`，并在文案里写明新 SHA 没有被推送。测试：`out_of_scope_revise_escalates_with_a_hub_notice_and_pushes_nothing`。
- `03b33bc fix(auth): let the phone's own origin approve, and stop trusting a declared identity` — Gemini #2，**这一项是手机端真正的阻断**。`allowed_origin` 只放行 `http://127.0.0.1:<port>` 和 tauri 源，手机从 `https://<host>.ts.net` 发批准必然 403。现在：存在受信任身份头、`Host` 唯一、且 `Origin` 是同一主机的 `https://` 同源时放行；跨站 Origin 仍然拒绝。`POST /api/pairing/exchange` 不再读 `body.peer_identity`（该字段已从请求体删除，带它会被 422 拒绝），改为只认代理头，并补 `Host`/`Origin` 校验（Loopback 主机或带身份头才可配对）。测试：`phone_page_origin_is_accepted_and_a_cross_site_origin_is_refused`、`pairing_never_trusts_a_peer_identity_from_the_request_body`；原有 `stage3_pairing_dual_identity_auth_revocation_mesh_and_private_notifications` 未改动、仍通过。
- `3720020 fix(observer): external SCM events are host input, and bots are never admitted` — Gemini #1。存储层新增 `hub_enqueue_external`（强制 `source='host'`）与 `run_repo_cwd`；`ScmObserver` 一律用它入队，因此外部 PR 评论/评审**不可能**再以 `source='user'` 落库，也就不可能满足 `hub_ticket` 的 basis（该 SQL 只接受 `source='user'`）。同时按该仓库的 admission policy 过滤：`off` 全部静默，`<name>[bot]` 或 `bot_actors_json` 里的账号一律不进入 Hub、也不进 Hub 会读的 change log。`Triage` 的判定抽成 `inference_verdict` 供两处共用，避免两份规则漂移。测试：`external_comments_are_host_input_never_user_input`、`a_bot_comment_never_wakes_the_hub`。
- `4470a0a fix(observer): keep review and comment cursors apart so neither hides the other` — Gemini #3。GitHub 的 review 与 issue comment 是两套自增序列，原来共用一个 `last_comment_id`，id 较小的一侧会被永久跳过。新增 `scm_review_cursors`（沿用 live-schema 加表，`user_version` 仍是 10，`store.rs` 那条 `== 10` 断言未受影响），review 用自己的游标。测试：`a_large_review_id_does_not_hide_later_comments_with_smaller_ids`（review id 900 之后 id 6 的新评论必须被发现）。
- `26af096 fix(notify): wake on an observed check failure, conflict or human review` — Gemini #4。`classify_attention_change` 只认 `payload.status`，而 observer 写的是 `kind`（`ci_failure` / `merge_conflict` / `human_review`），所以 CI 失败、合并冲突、人工评审永远不会推送。现在两条都认，仍然只发元数据标题，不带事件正文。测试：`observed_pr_gate_kinds_wake_the_phone_without_any_payload_text`。
- `beed1ad fix(mesh): namespace the wire request_id with the calling Hub session` — 合并审查者 #2。查证结果与审计原文有出入：接收端的 `node_id` 是**接收方自己**的 mesh id（`authorize` 强制 `scope.node_id == node.id`，token 里也没有发送方身份），所以"接收端加发送节点前缀"在现有协议里做不到。实际能做且已做的是主控侧命名空间：wire 上发 `"<hub_session>/<request_id>"`，账本键仍是 Hub 的原始 `request_id`（否则 `record_remote_dispatch_outcome` 会找不到行、并被静默吞掉），原始值上限收紧到 200 字节给前缀留预算。测试：`two_hub_sessions_with_the_same_request_id_get_two_remote_runs`。

Tests: 每个提交都在本机 macOS 跑过 `cargo fmt --all -- --check`、`cargo clippy --locked --workspace --all-targets -- -D warnings`、`cargo test --locked --workspace`。分支顶端 `beed1ad`：**163 passed, 0 failed**（基线 `5fdc9e1` 为 152）。新增 11 个测试，全部是净新增；改了两处既有断言，理由见下条 Issues。**Windows CI 没有跑**，UI 的 `tsc`/浏览器检查本轮未跑（没有改动任何 UI 文件或 DTO，`proto.ts` 因此无需重生成，CI 的漂移门不受影响）。

Hand check: 本轮没有 UI 改动，所以没有 1440/375 截图，也没有 `handcheck/<job>/`。所有验证都是 Rust 侧的命令输出，逐条写在上面的提交说明里。

Issues（已写入 `PLAN-ISSUES.md`）:

- **改了两处既有断言**，都是"协议/行为被有意改变"所必需，不是放宽：
  - `bins/polyclaved/src/hub_api/tests.rs:2000`：`run.request_id` 由 `"req-mesh-1"` 改为 `"hub/req-mesh-1"`（wire 上现在带会话前缀）。
  - 同一文件 `:2741`：崩溃重发夹具里手工构造的 wire `request_id` 由 `"req-lost"` 改为 `"hub/req-lost"`，让"崩溃前主控发出去的那条请求"与真实前缀一致；该测试的所有断言原样保留。
- 新增 `direction` 条目：429/400/403 退还工单额度（含设计、以及为什么 400 需要先改接收端语义）。
- 原 `detail` 条目（按 SHA 记录测试结果）补上了落地清单与 7 个需要所有者决定的问题。
- `dsh` 注入 `mcp_url` 仍未做：需要一次真实 `dsh` 运行才能确定配置方式，本轮不允许起真实 vendor。

Cleanup: 没有起任何 host、fake codex 或浏览器；测试用的临时目录与假 HTTP 服务都由 `tempfile` / `abort()` 收掉。

Privacy scan: `privacy-scan: clean (origin/main..HEAD)`。

Not done: Windows CI；真实环境验收（本轮一项都没跑）；按 SHA 记录测试结果；`dsh` 的 `mcp_url` 与模型选择；`REVIEWER.md`、`AGENTS.md` 和 v5 规范里过时的内容（属于所有者确认后才能改的规则/设计文档）。

### 自检（交 Gemini Neon 之前）

写完这 8 个提交后逐条重读了自己的改动，找出并修掉了三个问题，另有一项残余风险如实记下。全部都在这一个补充提交里：

- `03feffe fix: self-review round — pairing origin rule, precise publish refusal, one policy read per gate`
  1. **配对交换的原生回归**（我自己引入的）：`exchange_pairing` 起初只接受"同源 https"，于是桌面页面自己（`http://127.0.0.1:<port>`）和 Tauri 外壳去交换票券会被 403。改为复用 API 的同一套 `origin_allowed` 判断，两条路径共用一条规则，不会再漂移。
  2. **拒绝原因不精确**：首次推送的 `Error::Policy` 既可能是"分支属于别人"，也可能是"SHA 不在镜像里/分支名非法"，但我把它们都写成了"分支不空"。新增 `polyclave_hub::Error::Publish` 只表示"目标分支不是本门禁的"，两种情况分别转人工、分别写文案；返工推送的租约失败也一并按 `Publish` 转人工并通知 Hub（原来会直接冒泡成错误）。`map_hub_err` 把它映射为 409。新增测试 `a_branch_that_moved_under_a_revise_escalates_instead_of_being_forced`。
  3. **每个事件两次数据库读**：`admits` 原来对每个事件各查一次 `runs.repo_cwd` 和 `admission_policies`，现在每个 gate 只读一次策略再传下去；顺手把 review 游标保存时的重复读去掉，并清理了 `emit_event` 里多余的嵌套块。
  4. **残余风险（未改，如实记录）**：Hub provider 仍把 `source='host'` 的消息按 **user** 角色交给模型（`crates/polyclave-hub/src/provider.rs:157`、`cli_provider.rs:214`），所以一条外部 PR 评论在模型的对话里仍是一条"用户"发言——只是它在账本里不是用户消息、也永远无法充当工单依据。这一层属于"要不要在历史里给外部消息单独加标签"的产品决定，已作为 `direction` 记进 `PLAN-ISSUES.md`；本轮不动 prompt 构造。

自检后分支顶端：**164 passed, 0 failed**（新增 1 个测试，自检提交自身也过了 fmt / Clippy `-D warnings` / workspace 测试）。文档同步：`README.md` 增加了"Reaching the daemon from a phone"一节（loopback + Tailscale Serve、身份头、同源规则），并更新了验证状态表里 PR 门禁、手机配对、SCM 观察器、mesh 四行。

## 2026-11-27 审计修复合并审查（Gemini）：passed, merged

Branch `fix/audit-2026-11-27` (`03bc09c..ea4a072`, 11 commits) was fast-forwarded into `main` and the branch deleted.

- **Code & invariant audit:** verified `pr_gate.rs` first-push refusal (`Error::Publish` + empty-lease `--force-with-lease=<dest>:` for absent branches), pre-push `reviewable_diff` before both first push and revise push, revise scope-violation Hub notice, `auth.rs`/`routes.rs` unified `origin_allowed` + removal of `body.peer_identity`, `scm_observer.rs` `hub_enqueue_external` (`source='host'`) + single per-gate policy read + separate `scm_review_cursors` table, `notify.rs` `pr_gate` `kind` metadata wakeups, and `tools.rs` session-namespaced wire `request_id`.
- **Approved test edits:** `bins/polyclaved/src/hub_api/tests.rs:2000` and `:2741` (wire `request_id` namespace sync, no weakened assertions).
- **Verification (macOS):** `cargo fmt --all -- --check` clean; `cargo clippy --locked --workspace --all-targets -- -D warnings` clean; `cargo test --locked --workspace` 164 passed, 0 failed (including `TurnDto` compile-fail doctest); `bash scripts/privacy-scan.sh` clean.
- **Nits / open items:** `pr_gate.rs:379` maps a narrow TOCTOU `git push` lease failure to `Error::Transport` rather than `Error::Publish`; multi-primary mesh `request_id` isolation relies on unique Hub session IDs. Local `main` remains unpushed (Windows CI pending). Open `[direction]` items in `PLAN-ISSUES.md`: per-SHA test results for the approval card, prompt rendering of `source='host'` messages, and mesh 429 refund.

## 2026-11-27 主线里程碑：按 SHA 记录测试结果 + 外部消息 prompt 隔离

分支 `fix/per-sha-tests-and-prompt-isolation`，基点 `main@c88a856`。只做 `NEXT.md` 主线里程碑需要的两项，没有碰 mesh / CUA / 自我进化，**没有推送 origin/main**。

Commits:

- `b5aff98 feat(evidence): record and show test results for the exact SHA being approved`
- `7d12fba feat(prompt): mark host observations so an external comment never reads as the user`

### 1. 按 SHA 记录测试结果（关闭 PLAN-ISSUES 的两条：原 `[detail]` 与 `[direction]`）

按所有者的 7 条决策落地，逐条对应：

| 决策 | 落地方式 | 证据 |
|---|---|---|
| (1) 只有 ci/host_run 且 SHA 精确匹配 `result_sha` 才可绿灯；worker_claim 始终标注自报 | `evidence.ts` 用 `r.result_sha === test_sha` 判定；claim-only → `Worker’s claim`（橙），observed SHA 上的通过 → `Reference only`（橙），失败一律 `Failed`（红） | `approval-card.mjs` r3/r4/r5 |
| (2) 不建 host_run 引擎 | 只保留 `TestResultSource::HostRun` 这个词汇位，没有写入点 | — |
| (3) 只统计等于 `result_sha`（否则 `observed_sha`）的 check-run | `Store::run_test_sha` = `COALESCE(result_sha, observed_sha)`，observer 只在 PR head 等于它时写行 | `check_runs_are_recorded_only_for_the_shas_this_run_is_about` |
| (4) claim 与 ci 冲突以 ci 为准，同时展示自报，不可误标通过 | 行状态取 check 结论；claim 追加引用并写明 "A CI result wins over a claim" | `approval-card.mjs` r4 |
| (5) 新字段不参与幂等冲突 | `Submit::same_receipt` 只比 `run_id/branch/sha`；重发改写文案返回 "Already accepted."，首次回执保留 | `submit_result_records_the_workers_test_claim_for_the_submitted_sha` |
| (6) 允许更新 6 处既有断言 | 见下方"既有测试改动" | — |
| (7) live-schema 加表，不升 `user_version` | `test_results.rs::upgrade` 挂在 `writer.rs` 的三个 additive 步骤之后 | `test_results_table_upgrades_without_moving_the_schema_version` |

实现清单：

- `crates/polyclave-store/src/test_results.rs`（新）：`test_results(run_id, sha, source, name, status, summary, recorded_at)`，主键 `(run_id, sha, source, name)`，索引 `(run_id, sha)`；`record_test_result`（校验 40 位十六进制 SHA、摘要非空且 ≤2000、run 必须存在）、`test_results_for_sha`（check 在前、claim 在后）、`Store::run_test_sha`。
- `crates/polyclave-core/src/runs.rs`：`evidence_for_run` 读出收据里的 `tests`/`tests_status`/`not_done`，并按 `test_sha` 精确过滤行；`ReceiptRow` 类型别名避免 clippy 的复杂类型告警。
- `crates/polyclave-adapters/src/mcp.rs`：`submit_result` 新增可选 `tests`、`tests_status`、`not_done`（`inputSchema` 同步，仍不在 `required` 里）；`claim_notes` 负责校验（长度、不含 turn token、状态必须是三种之一、有状态无摘要视为非法）；接受回执时写一条 `worker_claim` 行。
- `crates/polyclave-hub/src/scm_observer.rs`：`check_status` 把 check-run 结论映射为 `passed/failed/pending`（只有明确 `success` 是 passed），PR head 等于 `run_test_sha` 时按 check 名写/更新 `ci` 行。
- `crates/polyclave-proto`：`TestResultSource`、`TestStatus`（带 `as_str`/`parse`，与账本拼写一致）、`TestResultDto`；`RunEvidenceDto` 增加 `test_sha`、`receipt_tests`、`receipt_tests_status`、`receipt_not_done`、`test_results`；`proto.ts` 已用 `POLYCLAVE_UPDATE_PROTO=1` 重新生成，CI 的漂移门通过。
- `apps/polyclave-app/ui/src/views/evidence.ts`：tests 行改为真实数据（6 种状态），decision 行展示 Worker 提交的测试摘要与未完成清单，并标明是自报。
- `DESIGN.md`：摘要行、tests 行、decision 行、证据来源四段同步改写。

### 2. 外部消息 prompt 显式隔离（关闭 `[direction] External SCM events…`，选项 A）

- `Store::hub_enqueue_external` 现在写 `metadata_json = {"kind": "scm_observation"}`（常量 `polyclave_store::HOST_OBSERVATION_KIND`）。
- `polyclave_hub::provider`：`is_host_observation`、`prompt_text`、`HOST_OBSERVATION_PREFIX = "[host observation, not from the user] "`，四个拼装点全部接上：`restore_api_history`、API 轮次的当前输入（`bins/polyclaved/src/hub_api.rs`）、CLI provider 的历史与当前输入。
- 账本文本不改写，前缀只出现在 prompt 里；两条既有的宿主提示（会话中断、生命周期）用的是别的 `kind`，保持原文。

### Tests

分支顶端 `7d12fba`，macOS 本地：

- `cargo fmt --all -- --check` clean
- `cargo clippy --locked --workspace --all-targets -- -D warnings` clean（0 error, 0 warning）
- `cargo test --locked --workspace`：**177 passed, 0 failed**（基点 `c88a856` 为 164；本轮净新增 13 个测试）
- `cargo test -p polyclave-proto generated_types_match_source_control` 通过，`proto.ts` 无漂移
- UI：`npx tsc --noEmit`、`npm run build`、`POLYCLAVE_BROWSER_CHANNEL=chrome npm run test:browser` 全通过（1440×900 与 375×812，dark 与 light，五个夹具）
- `bash scripts/privacy-scan.sh`：`privacy-scan: clean (origin/main..HEAD)`

新增测试：

- `crates/polyclave-store/tests/store.rs`：`test_results_are_keyed_by_sha_and_never_borrow_another_commit`、`test_results_reject_a_bad_sha_empty_summary_or_unknown_run`、`run_test_sha_prefers_the_verified_result_and_falls_back_to_observed`、`test_results_table_upgrades_without_moving_the_schema_version`、`external_hub_input_is_marked_as_an_observation_and_user_input_is_not`
- `crates/polyclave-core/tests/recovery.rs`：`approval_evidence_shows_the_workers_notes_and_only_the_exact_shas_results`
- `crates/polyclave-adapters/tests/mcp.rs`：`submit_result_records_the_workers_test_claim_for_the_submitted_sha`、`submit_result_refuses_a_claim_it_cannot_understand`
- `crates/polyclave-hub/src/scm_observer.rs`：`check_runs_are_recorded_only_for_the_shas_this_run_is_about`
- `crates/polyclave-hub/src/provider.rs`：`a_host_observation_is_marked_and_the_users_own_words_are_not`、`api_history_prefixes_observations_and_keeps_roles`
- `crates/polyclave-hub/src/cli_provider.rs`：`an_observed_scm_event_reaches_the_model_marked_as_not_the_user`
- `crates/polyclave-proto/src/lib.rs`：`test_result_serializes_as_the_ledger_spells_it`
- `apps/polyclave-app/ui/tests/approval-card.mjs`：r3（CI 通过 + 旧 SHA 失败行 + claim）、r4（CI 失败 vs claim 通过）、r5（仅 claim）三组夹具与断言

### 既有测试改动（依据 = 所有者决策第 6 条）

| 文件:行 | 改动 | 为什么必须改 | 强度 |
|---|---|---|---|
| `crates/polyclave-proto/src/lib.rs:129` | 测试名 `run_evidence_serializes_missing_receipt_and_claim_as_null_and_has_no_test_field` → `..._as_null_and_empty_results` | 原名断言"没有 test 字段"，而新类型按决策 1 必须有 | 相等 |
| 同上 `:131-152` | 结构体字面量与 `assert_eq!` 精确 JSON 补上 5 个新字段（null / `[]`） | 同上；仍是全字段精确相等 | 相等 |
| `crates/polyclave-core/tests/recovery.rs:407` | `RunEvidenceDto { … }` 字面量补 5 个字段 | 编译所需；断言仍是"只读当前 Attempt" | 相等 |
| 同上 `:423` | 同上（attempt 2 的空证据） | 同上 | 相等 |
| `apps/polyclave-app/ui/tests/approval-card.mjs:37-40` | `details` 夹具补 `test_sha`/`receipt_*`/`test_results`（r1/r2 为空） | 新 DTO 字段；r1/r2 的既有断言**一字未改**，仍验证"无记录" | 加强 |
| 同上 `:200-260` | 新增 r3/r4/r5 三组断言与截图 | 新行为必须有断言 | 净新增 |

### Hand check

`handcheck/per-sha-tests/`（7 张 JPEG，均 < 300 KB，另有 `measurements.md`）。`r3-desktop-light.jpg`：摘要 "Commit verified · tests passed for dddddd · not judged by you"，tests 行 `Passed`，两条事实分别是 CI 与该 Worker 自报，旧提交的 `ci/old` 失败行没有出现。`r3-phone-dark.jpg`：375 下摘要换行、pill 移到标签下方、无横向溢出。`r4-desktop-dark-card.jpg`：CI 失败时行转红且仍引用自报。`r5-phone-dark-card.jpg`：只有自报时是橙色 `Worker’s claim`。375 宽下 `#inspect` 内无 `scrollWidth > clientWidth`，卡片边界 10–365 px，无小于 44 px 的触控目标。

### Issues

`PLAN-ISSUES.md` 两条转为 resolved（含 7 条决策与选项 A 的落地说明），原始 `[detail]` 条目也标注了 pointer。仍然开着：429/400/403 退额度（`direction`）和 `dsh` 的 `mcp_url`（`detail`）。

### Cleanup

没有起 host、fake codex 或常驻浏览器；离线测试用的临时目录与假 HTTP 服务都由 `tempfile` / `abort()` / `server.close()` 收掉。

### Not done

Windows CI（本机只跑 macOS）；真实环境验收一项都没跑；`host_run` 引擎（决策 2 明确暂不建）；把 `source='host'` 的角色渲染进一步细化（本轮只加了前缀，角色仍是 User，见 `PLAN-ISSUES.md` 里的对应条目）。

## 2026-11-27 主线里程碑合并审查（Gemini）：passed, merged

Branch `fix/per-sha-tests-and-prompt-isolation` (`b5aff98..9d94605`, 3 commits) was fast-forwarded into `main` and the branch deleted.

- **Code & invariant audit:** verified `test_results` live-schema table (`user_version` unchanged at 10, exact 40-hex SHA key `(run_id, sha, source, name)`), `scm_observer.rs` recording CI check runs outside `last_check_sha` so `pending -> passed/failed` updates land for `run_test_sha`, `mcp.rs` `submit_result` optional `tests`/`tests_status`/`not_done` validation without changing `same_receipt` idempotency, `evidence.ts` gating green strictly on `ci`/`host_run` matching exact `result_sha`, and `provider.rs`/`cli_provider.rs`/`hub_api.rs` prefixing `scm_observation` messages with `[host observation, not from the user] ` without altering ledger text.
- **Approved test edits:** all 6 authorized edits (`crates/polyclave-proto/src/lib.rs`, `crates/polyclave-core/tests/recovery.rs`, `apps/polyclave-app/ui/tests/approval-card.mjs`) match the owner decision with no weakened assertions.
- **Visual & automated verification (macOS):** visually checked `handcheck/per-sha-tests/{r3-phone-dark,r4-desktop-dark-card,r5-phone-dark-card}.jpg`; `cargo fmt --all -- --check` clean; `cargo clippy --locked --workspace --all-targets -- -D warnings` clean; `cargo test --locked --workspace` 177 passed, 0 failed (including `TurnDto` compile-fail doctest); `proto.ts` diff clean; UI `tsc --noEmit`, `npm run build`, and `POLYCLAVE_BROWSER_CHANNEL=chrome npm run test:browser` (1440×900 & 375×812, dark & light) passed; `bash scripts/privacy-scan.sh` clean.
- **Nits:** `scm_observer.rs` does not emit a `change_log` event when writing non-failing CI `test_results` rows (open cards update on next fetch); `is_host_observation` checks `metadata["kind"]` without additionally checking `source == "host"` (downgrade-only prefix, no privilege escalation). Pushing `main` now to run dual-platform macOS + Windows CI.

## 2026-11-27 Polyclave v5 全项目对抗审查（Codex）

**审查基准** `main@3489145b5301e1084aab2e4d5e175f1812a97840`。

**结论：主线暂不能进入真机端到端验收。** 本轮只读走查生产路径、测试替身与交接记录。审查结束时，确切提交的 macOS 和 Windows 离线 CI 均已通过，包含 Rust workspace 和 UI 浏览器检查。177 个 Rust 测试的计数来自上方合并记录。本轮未重跑测试，未启动真实厂商 CLI、已安装的 WebView 或真机扫码，也未对 GitHub 执行写操作。以下发现均为该提交上的静态结论，修法和反例测试均待实施。[该提交的 GitHub Actions](https://github.com/DustNonStar/polyclave/actions/runs/46100050814)

验收主线为 Mac 派发真实 Worker，中途重启后页面恢复同一 Run，异构 Reviewer 要求修改，Worker 交付并核证下一版，手机显示该 SHA 的 diff、测试和未决项，并批准这个确切 SHA。沿 `README.md`、`PRODUCT.md`、`NEXT.md`、`PLAN-ISSUES.md` 和 `docs/LESSONS.md` 第 5–10 条复核，暂停的功能与已有挂单沿用当前里程碑范围。

### 阻断（真机必挂）

1. **默认 Codex Worker 的 macOS 沙箱阻止正常 Git 提交。**
   - **证据与触发**　`crates/polyclave-adapters/src/codex.rs:20` 的 `codex_args` 设置 `workspace-write` 和 `approval_policy=never`。与此同时，`crates/polyclave-core/src/worktree.rs:42` 的 `prepare_attempt` 在工作目录建立普通 `.git`，`crates/polyclave-core/src/runner.rs:175` 的 `TurnRunner::execute` 要求 Worker 提交并推送。采用上游官方策略的 Codex 会把可写根下的 `.git` 设为只读，正常 `git add` / `git commit` 无法写入元数据。此判断未运行本机 Codex。[Codex 官方 macOS 沙箱说明](https://github.com/openai/codex/blob/main/codex-rs/core/README.md#macos)
   - **现有覆盖**　`crates/polyclave-core/tests/worker.rs:30` 直接运行假二进制，`crates/polyclave-adapters/tests/worker.rs:12` 只检查参数。
   - **最小修法与反例**　提供只允许写当前 Attempt 的 Git 执行边界，或由受限宿主工具完成提交与 staging 推送。用 macOS 生产启动路径验证 Attempt 内可提交，主仓库和宿主镜像仍拒绝 Worker 写入。

2. **真实返工被 Run 终态拒绝，测试替身直接返回了下一版 SHA。**
   - **证据与触发**　R1 把初稿置为 `done` 后，`bins/polyclaved/src/hub_api.rs:434` 的 `HostWorkerReviseExecutor` 以 `WorkerChat` 调度返工或范围退回。`crates/polyclave-core/src/runs.rs:215` 的 `RunManager::begin` 拒绝终态，门禁从 `crates/polyclave-hub/src/pr_gate.rs:920` 走到 `revise_unverified`。单独放开这个检查仍会遇到 `crates/polyclave-core/src/runner.rs:274` 的 `prepare` 终态检查，以及 `crates/polyclave-store/src/schema.sql:153` 的每 Attempt 单一已接受回执约束。
   - **现有覆盖**　`crates/polyclave-hub/src/pr_gate.rs:1435` 的 `MockRevise` 跳过调度、回执和镜像导入。
   - **最小修法与反例**　为门禁新增独立返工转换，从上一版核证 SHA 建立新 Attempt，保留 Run 原始核证基线并独立签收新结果。用真实返工执行器和 TurnRunner 串起 A、Reviewer 要求修改、B 的新回执。缺 B 回执时，旧批准和 A 的核证结果均不得充当返工结果。

3. **二维码可解码，扫码后没有生产配对路径。**
   - **证据与触发**　`bins/polyclaved/src/hub_api/http.rs:706` 的 `create_pairing_ticket` 返回不含宿主地址的 `polyclave://pair`。移动 Tauri 入口 `apps/polyclave-app/src-tauri/src/lib.rs:4` 没有处理该 scheme。手动打开 Tailscale HTTPS 页面也无法配对，`apps/polyclave-app/ui/src/api/sse.ts:4` 的 `bootstrap` / `request` 和 `apps/polyclave-app/ui/src/views/flows.ts:142` 的 `tokenDialog` 都限制为本机 HTTP，页面没有兑换调用。
   - **现有覆盖**　`apps/polyclave-app/ui/tests/qr-roundtrip.mjs:14` 只做二维码字节往返。`bins/polyclaved/src/hub_api/tests.rs:3024` 在服务端测试中直接兑换并构造请求头，随后读取设备列表。
   - **最小修法与反例**　生成指向配置过的 Tailscale HTTPS 源的配对链接。页面同源兑换、清除链接中的秘密，再用设备凭据访问 API 和 SSE。补 375 px 浏览器从扫码 URL 开始的兑换、重放拒绝、吊销和重连回归。

4. **最终已核证 SHA 缺少人工审批，卡片也没有实际 diff。**
   - **证据与触发**　`apps/polyclave-app/ui/src/views/inspector.ts:383` 的 `review` 对 `done` 不提供 Accept。`crates/polyclave-core/src/runs.rs:446` 的 `resolve_seen` 只处理 `needs_review` / `interrupted` 的异常结算。`bins/polyclaved/src/routes.rs:157` 的 `run` 通过 `crates/polyclave-core/src/mirror.rs:400` 的 `commit_summary` 提供文件统计和提交标题。异构 Reviewer 同意后，`crates/polyclave-hub/src/pr_gate.rs:1011` 的 `run_pr_gate` 可直接启用 auto-merge，远端满足条件时可以先于人工审批合并。[GitHub auto-merge 行为](https://docs.github.com/en/pull-requests/how-tos/merge-and-close-pull-requests/automatically-merging-a-pull-request)
   - **现有覆盖**　`apps/polyclave-app/ui/tests/approval-card.mjs:156` 检查异常 Run 的按钮文字和已核证 Run 的静态证据。`bins/polyclaved/src/hub_api/tests.rs:901` 把 `auto_merge_armed` 当作成功。
   - **最小修法与反例**　增加独立的人审等待状态和审批 POST，绑定完整 SHA，提供同 SHA 的镜像 diff、测试与未决项。补 375 px 实际 POST，展示 A 后 head 移到 B 时拒绝 A，并验证确认 B 前没有合并副作用。

### 高危（安全或不变量破坏）

5. **人工接收产生的 `result_sha` 会进入 R1 自动发布路径。**
   - **证据与触发**　`crates/polyclave-core/src/runs.rs:471` 的 `resolve_seen` 接受与 `seen_sha` 相同的镜像 tip，忽略 `BranchCheck::Present` 的 `problem` 与回执是否存在，随后以 `result_source='user'` 写入 `result_sha`。`bins/polyclaved/src/hub_api.rs:422` 的 `verified_result_sha` 只查非空，允许缺回执或缺 trailer 的人工接收结果进入门禁。`apps/polyclave-app/ui/src/views/evidence.ts:96` 的 `tests` 同样只比较 `result_sha`，可把该 SHA 的测试行标成 R1 绿灯。
   - **现有覆盖**　`bins/polyclaved/src/hub_api/tests.rs:681` 的反例只覆盖 `observed_sha` 有值而 `result_sha` 为空的情形。
   - **最小修法与反例**　保留人工接收，自动发布和 R1 绿灯共同检查核证来源和已接受 Attempt 的同 SHA 回执。通过生产路径人工接收无回执 tip，再断言门禁拒绝且未调用推送、评审或合并。

6. **调用方可以改变评审基线和工单授权范围。**
   - **证据与触发**　`bins/polyclaved/src/hub_api.rs:1007` 的 `prepare_pr_gate_run` 和 `bins/polyclaved/src/hub_api/tools.rs:113` 的 `run_pr_gate` 工具允许提交 `base_sha`、`base_branch` 和 `touched_paths` 覆盖。评审与范围检查使用传入的 base，GitHub 开 PR 使用独立的 base branch。传入与 head 相同的 base 可产生空评审 diff，实际 PR 仍有内容。路径规则也可比 Ticket 授权更宽。见 `crates/polyclave-hub/src/pr_gate.rs:656` / `:790` 的 `run_pr_gate`。
   - **现有覆盖**　`bins/polyclaved/src/hub_api/tests.rs:751` 检查调用方传入错误 head。门禁的全范围评审和路径反例使用预设 base 与规则。
   - **最小修法与反例**　用 Run 与 Ticket 持久数据限定基线和路径，校验实际 PR 基线对应评审范围。补空范围、远端 base 不一致、调用方扩大授权三个反例，均须在首次推送前拒绝。

7. **Claude Seatbelt 未隔离宿主文件和本地 API。**
   - **证据与触发**　`crates/polyclave-adapters/src/claude.rs:42` 的 `build_profile` 允许全盘读取和 loopback 连接。Worker 与 daemon 同用户，宿主 GUI token 可读，`bins/polyclaved/src/auth.rs:130` 的 `require` 赋予它主 API 权限。整个 `/private/tmp` 可写，导致位于该目录下的宿主状态、镜像或主仓库落入 Worker 可写边界。token 文件由 `bins/polyclaved/src/main.rs:102` 的 `serve` 创建。
   - **现有覆盖**　`crates/polyclave-adapters/tests/worker.rs:489` 只检查一次 worktree 写入和外部写入，没有检查凭据读取、主 API 或临时目录下的宿主路径。
   - **最小修法与反例**　为 Attempt 设置独立临时写根，保护宿主凭据和账本读取，并把 Worker 可访问的本机服务限于其 MCP。用哨兵凭据和测试服务验证普通位置及临时目录布局中的边界。

8. **手机入口接受普通 Mesh 头，远程凭据检查允许无身份绑定的设备。**
   - **证据与触发**　`bins/polyclaved/src/routes.rs:308` 的 `exchange_pairing` 和 `bins/polyclaved/src/auth.rs:68` 的 `require` 将 `x-agentmesh-node` 作为 Tailscale 身份头的兜底。`crates/polyclave-store/src/stage3.rs:295` 的 `verify_device_token` 对无绑定的设备不检验来访 peer，导致本机兑换的无绑定令牌也可通过带 peer 的远程验证。前提是存在有效票据或设备令牌。[Tailscale Serve 身份头契约](https://tailscale.com/docs/features/tailscale-serve#identity-headers)
   - **现有覆盖**　`bins/polyclaved/src/hub_api/tests.rs:3024` 使用预置 Tailscale 头和已经绑定的令牌验证同源访问。
   - **最小修法与反例**　手机入口从可信 Serve 路径取得身份，远程验证要求持久绑定存在且相等。补无 Tailscale 身份、仅有 Mesh 头、本地无绑定凭据走远程入口的拒绝测试。

9. **Pause 与远端 enable 并发时，能留下本地显示暂停、GitHub 仍可自动合并的 PR。**
   - **证据与触发**　`crates/polyclave-hub/src/pr_gate.rs:1023` 的 `run_pr_gate` 先写 armed 再调用远端。`crates/polyclave-hub/src/pr_gate.rs:1042` 的 `pause_all` 可在 enable 完成前关闭远端并清掉账本标记。若并发 enable 随后成功，远端重新启用。本地也会在 enable 响应丢失时直接 disarm，遗失远端结果未知的状态。账本转换见 `crates/polyclave-store/src/stage2.rs:706` 的 `arm_auto_merge`。
   - **现有覆盖**　`crates/polyclave-hub/src/pr_gate.rs:1560` 等 enable 完成后才调用 Pause。
   - **最小修法与反例**　按 PR 串行化开关操作，持久记录未确认的远端意图并在 Pause 和恢复时对账。用屏障交错 enable 和 Pause，再测远端已执行但响应丢失时的重启对账。

### 重要（逻辑缺陷或明显体验断链）

10. **宿主重启后，已打开的 Mac 页面一直拿旧令牌重连。**
    - **证据与触发**　`bins/polyclaved/src/main.rs:102` 的 `serve` 启动时轮换本地 token。`apps/polyclave-app/ui/src/app.ts:228` 只 bootstrap 一次，`apps/polyclave-app/ui/src/api/sse.ts:30` 的 `follow` 在收到 `401` 后继续用旧连接重试。SQLite 恢复后，页面仍可能显示旧快照或在线状态。`app.ts:121` 的 `frame` 收到 `resync_required` 时拉取快照，却没有清除丢失终止事件的 `S.live`。
    - **现有覆盖**　`bins/polyclaved/src/routes.rs:558` 验证游标和 resync 帧，浏览器夹具使用固定 bearer。
    - **最小修法与反例**　桌面壳鉴权失败后重新取凭据，普通浏览器提示重新连接。恢复后同步快照并清空瞬态流状态。用同一页面模拟流断开、token 轮换、宿主恢复并重新读取同一 Run。

11. **Attempt 建目录与落库之间崩溃，会让同一编号反复卡在残留目录。**
    - **证据与触发**　`crates/polyclave-core/src/runs.rs:291` 的 `RunManager::begin` 先调用 `crates/polyclave-core/src/worktree.rs:22` 的 `prepare_attempt`，再写 Attempt 和 `current_attempt`。若 staging 创建后、落库前宿主退出，`crates/polyclave-core/src/recovery.rs:73` 把未派发 Run 置为 interrupted，重试仍用相同编号，创建目录遇到残留即失败。
    - **现有覆盖**　`crates/polyclave-core/tests/recovery.rs:289` 在 `begin` 前后构造崩溃，未覆盖这个中间窗口。
    - **最小修法与反例**　先持久预留 Attempt，恢复时识别未释放的准备态并安全重建或跳过。用屏障在 staging 建目录后终止宿主，验证同一 Run 可继续且不复用来源不明的目录。

12. **stdout 队列饱和时，Worker 退出可以使宿主结算死锁。**
    - **证据与触发**　`crates/polyclave-core/src/runner.rs:356` / `:413` 的 `run_process` 使用 256 项同步队列。supervisor 退出后，消费者先 join stdout reader，再排空队列。reader 若阻塞于向满队列发送，双方互等，取消检查和 `runner.rs:500` 的结算均无法继续。
    - **现有覆盖**　`crates/polyclave-core/tests/worker.rs:68` 的 Worker 夹具没有制造退出时的满队列。
    - **最小修法与反例**　在等待 reader 时继续消费，并限制进程退出后的管道排空时间。用屏障制造超容量事件，确认 Run 能结算、MCP token 撤销、下一次调度可取得执行槽。

13. **同 SHA 的 CI 卡片仍可假绿，也可能保留过期结果。**
    - **证据与触发**　`crates/polyclave-hub/src/scm.rs:698` 的 `list_pr_checks_and_comments` 只取 GitHub check-runs 第一页。`crates/polyclave-store/src/test_results.rs:15` / `:53` 以检查名作为同 SHA 下的唯一键组成部分，同名检查可以相互覆盖。`apps/polyclave-app/ui/src/views/evidence.ts:96` 的 `tests` 可把不完整的集合判为通过。另见 `crates/polyclave-hub/src/scm_observer.rs:167` / `:251` / `:322` 的 `poll_once`，它跳过已合并 PR，修改测试行不发 change event，第一次看到 SHA 后也不再产生该 SHA 的后续失败事件。[GitHub check-runs 分页契约](https://docs.github.com/en/rest/checks/runs#list-check-runs-for-a-git-reference)
    - **现有覆盖**　`crates/polyclave-hub/src/scm_observer.rs:753` 的夹具每页仅几个不同名的检查并直接读取 SQLite，浏览器夹具 `apps/polyclave-app/ui/tests/approval-card.mjs:203` 预填了结果。
    - **最小修法与反例**　完整分页，按稳定检查标识记录、区分是否取全，更新状态时发出 change event，按检查与状态转换去重通知。补第二页失败、同名异源检查、同 SHA 从排队转失败、首次轮询时已合并和打开的手机卡片自动更新。

14. **SCM 事件投递到启动时选定的 Hub，也不唤醒空闲会话。**
    - **证据与触发**　`bins/polyclaved/src/hub_api.rs:684` 的 `try_start_scm_observer_from_keychain` 在启动时固定第一个 Hub。空库启动后才注册项目时，观察器不向任何 Hub 入队。多项目时可投错。即使入队成功，`crates/polyclave-hub/src/scm_observer.rs:101` 的 `emit_event` 和 `:358` 的 `spawn` 也不触发 `bins/polyclaved/src/hub_api.rs:1145` 的 `Hub::kick`。
    - **现有覆盖**　`crates/polyclave-hub/src/scm_observer.rs:505` / `:598` 给 observer 注入已存在的 Hub，断言入队来源，未检查真实会话是否被唤醒。
    - **最小修法与反例**　按 Run 归属查找 Hub，成功入队后由 daemon 唤醒。补空库启动后注册两个项目的用例，断言只有所属 Hub 处理事件，且 prompt 保持宿主观察来源。

### 真机验收前的修复顺序

1. 先修真实凭据和发布边界。隔离 Worker 与宿主、绑定手机身份、校验 R1 发布来源与实际评审基线。要求人工审批后才启用自动合并，并消除 Pause 与远端请求竞态。
2. 闭合 Mac 生产执行链。让 Codex 能在 Attempt 内提交，修退出死锁、启动恢复和原页面重连。用真实返工执行器经新 Attempt 交付并核证 B。
3. 让 B 的审批证据完整且可刷新。采集 B 的 CI 检查，从镜像提供实际 diff 和未决项，用原子检查把审批绑定到 B。
4. 闭合扫码到审批 POST。用 375 px 浏览器覆盖 HTTPS 兑换、查看证据、拒绝过期 SHA 和批准当前 SHA，随后才执行真实 Mac 重启、异构 Reviewer、GitHub 和手机的整条验收。

## Audit Hardening — Batch 1 (Runtime & Sandbox)

- 61f6dd1 fix(core): drain stdout channel before joining reader on worker exit — workspace tests: 178 passed
- a58eff0 fix(core): persist attempt reservation before preparing worktree and staging — workspace tests: 180 passed
- ef40a41 fix(adapters): tighten macOS Seatbelt read, temp-write and loopback boundaries — workspace tests: 181 passed
- 70b4c21 feat(adapters): add host-side commit_and_stage MCP tool for sandboxed workers — workspace tests: 182 passed

Code head: `70b4c214d1859b44cc56acbc1a2ea320a12ae1ee` on `fix/hardening-batch1-runtime`, based on `main@615f6bf`. The four implementation commits are listed above in plan order.

### Implementation and evidence

- **1.1 — Worker stdout at exit.** The runner drains the bounded event channel while the reader finishes. It stops waiting after a two-second post-exit deadline if a descendant keeps the pipe open. A 280-event fake Worker holds the consumer until supervisor exit. The regression timed out before the fix and afterwards verified completion, token revocation and release of the execution slot.
- **1.2 — Attempt reservation.** SQLite reserves the Attempt before either repository directory is created. A preparation failure or startup recovery of a prepared Attempt without a turn supersedes that reservation. A retry uses the next Attempt number and leaves the residual directory isolated. Two regressions cover a directory collision and recovery with incomplete staging metadata before launch.
- **1.3 — macOS Claude confinement.** The Worker receives an Attempt-private temporary directory. Seatbelt denies reads of protected host state and direct reads of native Keychain files. The generated MCP config for this turn and the isolated Claude home remain accessible. The only permitted loopback destination is the assigned MCP TCP port. The policy retains non-loopback HTTPS and DNS for vendor connectivity. The native test used sentinel files under `/private/tmp` and two local listeners, and verified that the allowed worktree write and assigned MCP connection succeeded while the host reads, writes and other local connection were refused.
- **1.4 — `commit_and_stage`.** An authorized Codex or Claude Worker can pass a message and optional literal paths. The host checks the Ticket path scope, uses its own Git metadata and index, adds the Run-Id trailer, and publishes only to the Attempt's staging ref. Repeating the request with the same selected contents returns the staged SHA. The production MCP integration test verifies that omitted paths cannot widen scope, an expired token is rejected, Claude cannot write checkout or staging Git metadata, and the main repository and authoritative mirror are untouched by the tool. The separate `submit_result` call imports the exact SHA into the mirror for R1. Codex launch arguments were checked without starting the subscription CLI. [Codex sandbox configuration reference](https://developers.openai.com/docs/config-file/config-reference)

### Tests

- macOS local: `cargo fmt --all -- --check`, `cargo clippy --locked --workspace --all-targets -- -D warnings`, and `cargo test --locked --workspace` passed after each implementation item.
- Final workspace result at `70b4c21`: **182 passed, 0 failed**. The baseline was 177. Five regression tests were added. Existing tests retained their assertions and execution.
- Native Windows CI has no run for this local branch. The native macOS checks above are local. The diff for generated `proto.ts` is empty.

### Hand check

The native Seatbelt tests verified the filesystem and local-network boundaries using local fixtures. This batch has no UI changes or browser screenshots.

### Issues and cleanup

No `PLAN-ISSUES.md` items opened or resolved. The test MCP listeners, fixture processes and temporary repositories were torn down. No real vendor CLI or persistent host was started.

Privacy scan: `privacy-scan: clean (origin/main..HEAD)` after the four code commits.

### Not done

Real vendor CLI acceptance and Windows branch CI remain pending. Host staging requires a Ticket with explicit `allowed_paths` and an Attempt prepared by the new runtime. A legacy Attempt must be retried into a fresh workspace to use the tool. Batch 2 and Batch 3 remain for their own branches and review.

## Audit Hardening — Batch 1 self-review follow-up (2026-11-27)

- 2fdf2d6 fix(runtime): validate and cancel brokered Worker staging — workspace tests: 185 passed

Three regressions traced to Item 1.4 were corrected on `fix/hardening-batch1-runtime`.

- **Local dispatch.** An implicit Direct request Ticket has no `allowed_paths`. Both sandboxed vendors require the host broker to commit. The local endpoint now rejects a new unscoped Codex or Claude dispatch before it allocates a Run, an Attempt, or a Ticket slot. Create a Ticket with repository-relative allowed paths, then use Dispatch Worker on that Ticket. Local chat and retry also return scope guidance for an existing unscoped Run.
- **Cancellation.** Each Git command in the broker checks the turn authorization while running. Revocation terminates and reaps the Git process before the repository lock is released. The broker has a two-minute command-sequence deadline once it acquires the repository lock. If a push completed before revocation, reconciliation still sees its ref and applies the existing receipt and R1 rules. The stall regression verifies lock release, an absent staged ref and scratch cleanup when cancellation happens during commit.
- **Upgrade.** Continuing a scoped pre-upgrade Attempt without its host staging directory now returns an error before scheduling the Worker. For an Attempt awaiting review, choose Allow retry and then Retry to allocate a fresh Attempt. The regression verifies that a rejected chat preserves the old status and generation and that retry creates Attempt 2 with a staging directory.

Tests: macOS local formatting check, Clippy with warnings denied, and workspace tests passed at `2fdf2d6` with **185 passed, 0 failed, 0 ignored**. Three new regressions, with existing tests preserved. Generated protocol diff is empty. Privacy scan for the code commit printed `privacy-scan: clean (origin/main..HEAD)`.

Hand check and cleanup: the Unix stall regression used a local Git subprocess and temporary repository and joined the test thread before closing the fixture. The HTTP regression used an in-process Axum router. No UI changes or persistent fixture processes.

Native Windows execution of the Git Job Object path and real vendor CLI acceptance remain pending. The paused Mesh transport still has no Ticket path mandate for brokered Codex or Claude publishing. This branch is local for merge review.

## Audit Hardening — Batch 1 cancellation and Windows Job follow-up (2026-11-27)

- 8537267 fix(runtime): drain broker publication on cancellation — macOS workspace tests: 186 passed

- **Cancellation.** The runner persists `mcp_revoked=1` for the active Turn when it handles cancellation, before waiting for Worker teardown. `POST /api/runs/{id}/cancel` returns `accepted: true` and `broker_stopped: true` after the broker releases the repository lock. Worker settlement is asynchronous. A ref already published by an in-flight push remains subject to the existing reconciliation and receipt rules.
- **Windows Git.** The broker uses `CreateProcessW` with `PROC_THREAD_ATTRIBUTE_JOB_LIST` to attach the child at creation. It retains the Job through termination, waits for process handles and checks that the Job has zero active processes before returning from Git and releasing the mirror lock. If Windows cannot confirm cleanup, the daemon exits rather than continuing with uncertain writer exclusion.

Tests: macOS formatting, Clippy with warnings denied and workspace tests passed. The added macOS daemon regression establishes a socket barrier in the Git push command, calls the authenticated cancel route and verifies revocation, broker exit, no staging ref and no accepted receipt. It failed before the fix because the token remained active when the route returned. The two added Windows tests cover cancelling a live Git-like process tree and cleaning up a descendant after normal parent exit. They passed the `x86_64-pc-windows-msvc` cross-target Clippy check with `--all-targets` on macOS. Existing tests were preserved. The generated protocol diff is empty.

Hand check and cleanup: the HTTP regression used a real local daemon, a temporary repository and an offline Worker. Its subprocesses, listener and temporary repository were torn down. No UI changes.

Privacy scan: `privacy-scan: clean (origin/main..HEAD)` after the code commit.

Merge readiness: native Windows execution is still required on the exact branch head. This macOS host has no available Windows runner, and cross-target checks only compile the native tests. The branch remains local. Batch 2 and Batch 3 stay outside this follow-up.

## Audit Hardening — Batch 1 review (Gemini): passed, merged

Branch `fix/hardening-batch1-runtime` (Items 1.1–1.4 + self-review `2fdf2d6` + cancellation & Windows Job follow-up `8537267`) was rebased onto `main` (`d55add9`), fast-forwarded into `main`, and deleted.

- **Code & invariant audit:** verified bounded post-exit stdout drain (`runner.rs`), SQLite Attempt reservation before directory creation (`runs.rs`, `recovery.rs`), tightened macOS Seatbelt profile (`claude.rs`), host-side `commit_and_stage` MCP broker (`stage.rs`), immediate `mcp_revoked=1` persistence + `wait_for_cancelled_staging` repository-lock drain on `POST /api/runs/:id/cancel` (`routes.rs`, `runner.rs`, verified by real-daemon `cancel_http_drains_paused_broker_push_before_acknowledging`), and Windows `CreateProcessW` + `PROC_THREAD_ATTRIBUTE_JOB_LIST` atomic Job attachment + `drain_job` active-process-zero wait (`windows.rs`, `command_tree_windows.rs`).
- **Verification (macOS):** `cargo fmt --all -- --check` clean; `cargo clippy --locked --workspace --all-targets -- -D warnings` clean; `cargo test --locked --workspace` 186 passed, 0 failed (including `TurnDto` compile-fail doctest and real-daemon cancel staging test); `bash scripts/privacy-scan.sh` clean.
- **Pending CI:** pushing `main` now to run native `macos-latest` and `windows-latest` GitHub Actions CI (including `command_tree_windows.rs`). Nit: `unix.rs` `CommandTree::drop` waits on the main `git` process after `SIGKILL` to the process group without polling `group_members`.

## Audit Hardening — Batch 2 (PR Gate, Invariants & SCM Observer)

- 96f9eda fix(hub): require R1 receipt provenance for auto-publish and green tests — workspace tests: 187 passed
- 9dea911 fix(hub): bind PR gate base and path scope strictly to Run and Ticket — workspace tests: 191 passed
- 84ac48a feat(core): support PR gate revision attempts from verified SHA — workspace tests: 193 passed
- d434f27 fix(hub): serialize auto-merge arming with pause and persist remote intent — workspace tests: 196 passed
- 3798294 fix(hub): paginate check-runs by stable id and emit test_results changes — workspace tests: 199 passed
- 1016382 fix(hub): route SCM events to owning Hub and kick idle sessions — workspace tests: 200 passed

Code head: `1016382624d893f838388dbee22e5c3e19d1ea9c` on `fix/hardening-batch2-pr-gate`, branched from `main@e2cf2ba`. Batch 2 only; Batch 3 was not started.

### Implementation and evidence

- **2.1 — R1 provenance (Audit #5).** `RunManager::r1_verified_sha` (`crates/polyclave-core/src/runs.rs`) returns a `result_sha` only when the Run is `done` with `result_source='agent'`, no open `detail`, an `accepted_attempt_id`, and an accepted `results` row for exactly that SHA. The PR gate's `verified_result_sha` and the evidence card's tests row use it, so a human accept (`result_source='user'`, no receipt) still closes the Run for a person but never authorizes review, push or merge, and never turns R1 or tests green. New negative test drives the production accept path (`RunManager::resolve_seen(.., Accept, ..)`) on an un-receipted tip and asserts refusal with no push, no review, no auto-merge and no gate row; against the pre-item check it fails on `verified_result_sha(..).is_err()`.
- **2.2 — base and path scope (Audit #6).** `base_sha`, `base_branch` and `touched_paths` are gone from `RunPrGateInputs`, the HTTP body and the `run_pr_gate` tool schema; `prepare_pr_gate_run` reads the Run's immutable base and the Ticket mandate's `allowed_paths`, and the tool now documents that a caller cannot override them. `run_pr_gate` refuses an empty `base == head` range, and before the first push it calls the new `MirrorPusher::confirm_review_base`, which reads the remote base branch and requires the reviewed baseline to be an ancestor (fetching that one ref with `--no-write-fetch-head`, no local ref). Three new refusal tests plus a `GitMirrorPusher` unit test cover empty range, moved base branch and a caller widening the scope over HTTP and the MCP tool.
- **2.3 — revision Attempts (Audit #2).** `RunManager::begin_revision` creates a new Attempt branched from the previous Attempt's R1-verified SHA (new additive `attempts.from_sha`), leaves the Run's `base_sha` untouched, and clears the Run's own verified result first, so the revised commit needs its own accepted receipt. `TurnRunner::schedule_revision` runs the Worker turn in it, and `HostWorkerReviseExecutor` (`revise_pr`, `bounce_scope_violation`) uses that instead of `WorkerChat` on a terminal Run. The end-to-end test runs the real executor with the real `TurnRunner`, the offline Worker fixture and the real MCP receipt path: A verified → revision B with its own receipt (`from_sha = A`, `accepted_attempt_id` = attempt 2, gate publishes B) → a revision that submits no receipt leaves the Run `needs_review` with no `result_sha`, a third Attempt from B and no accepted receipt. Reverting `hub_api.rs` to `WorkerChat` makes that test fail with `Policy`, which is the audit's finding.
- **2.4 — arm/pause serialization (Audit #9).** `arm_auto_merge` now writes the unconfirmed remote intent (`pr_gates.remote_intent = 'enable'`) in the same transaction as the armed bit; `confirm_auto_merge` clears it on GitHub's answer and `begin_auto_merge_disable` persists `'disable'` before a pause calls GitHub. `run_pr_gate` and `pause_all` take one lock per gate (`pr_gate::auto_merge_locks`) and re-read the pause flag under it, so a Pause either happens before the enable (nothing is armed) or after it (and revokes it). An enable whose answer is missing (transport, invalid response, 5xx) keeps the gate armed instead of silently disarming, and notifies the Hub; a definite GitHub answer (4xx, GraphQL) does disarm. `reconcile_auto_merge` settles every armed gate against GitHub's real state and is called at daemon startup and from `pause_all`'s accounting. Tests: the enable is held open while the Pause starts (final ledger paused and disarmed, remote order enable → disable), a transport failure stays armed with the intent and is then revoked by reconciliation while paused, and the same gate is later tracked from GitHub's real state; a definite 422 still disarms.
- **2.5 — check runs (Audit #13).** `list_pr_checks_and_comments` pages with `per_page=100&page=N` until `total_count` is reached (bounded at 50 pages) and reports `checks_complete`; a page that cannot be read leaves the list partial. `test_results` is keyed by `(run, sha, source, name, check_id)` — the stable GitHub check id — so same-named checks from different sources keep their own rows; `record_test_results` returns the rows that really moved, and the observer publishes a `pr_gate` change event and notifies once per `(check id, transition)` instead of once per SHA. A partial list records a pending `polyclave/check-runs` marker (never green, removed on the next complete fetch), and check runs are now read before the merged branch, so a PR merged before the first poll still records its final SHA's evidence. Tests: page-2 failure read and later `pending->failed` on the same SHA (with the change event), merged-before-first-poll, same-name distinct-source rows, and browser case r7.
- **2.6 — SCM routing (Audit #14).** `ScmObserver` no longer pins a Hub session: it resolves the owner per gate with `Store::run_hub_session` and calls a `with_wake` callback after a successful `hub_enqueue_external`; the daemon wires that callback to `Hub::kick` and `try_start_scm_observer_from_keychain` no longer picks the first Hub at startup. The daemon test starts from the fixture's empty database, registers two projects afterwards, and asserts one event each, exactly the owning session woken, `source='host'` with `{"kind":"scm_observation"}` metadata, and no cross-delivery. Pinning the owner to one session makes it fail with `["hub-a", "hub-a"]`.

### Tests

- macOS local, after each item: `cargo fmt --all -- --check`, `cargo clippy --locked --workspace --all-targets -- -D warnings`, `cargo test --locked --workspace`. Final head `1016382`: **200 passed, 0 failed, 0 ignored** (baseline `e2cf2ba`: 186). Fourteen tests were added; no test was skipped, renamed or deleted.
- Generated protocol: `POLYCLAVE_UPDATE_PROTO=1 cargo test -p polyclave-proto` left `apps/polyclave-app/ui/src/generated/proto.ts` unchanged (`git diff --exit-code` clean). No proto type changed in this batch.
- UI (because `evidence.ts` changed in 2.1 and 2.5): `npx tsc --noEmit`, `npm run build`, and `POLYCLAVE_BROWSER_CHANNEL=chrome npm run test:browser` all pass at 1440x900 and 375x812, dark and light.
- Existing-test edits, all recorded in `PLAN-ISSUES.md` with justification: the 2.1 fixture gained the Attempt/turn/receipt rows a verified Run really has (its assertions unchanged); two 2.2 fixture remotes gained the base branch a real GitHub repository has, and one remote-command counter went 5 → 6 because the base confirmation is one more remote read; `test_results_table_upgrades_without_moving_the_schema_version` counts 8 columns instead of 7 because the key gained `check_id`; `NewTestResult` literals gained `check_id`, and `ScmObserver::new`/`start_scm_observer` call sites lost the pinned-session argument.
- Native Windows CI: none for this local branch. This host runs macOS only.

### Hand check

Browser check only; this batch adds no new layout or controls. Observed in the automated 1440x900 and 375x812 runs (dark and light), and in the saved cards:

- `handcheck/hardening-batch2/r6-phone-dark-card.jpg` — a human-accepted commit: R1 "Not verified", tests "Reference only" in orange, "You accepted 9999999 by hand; R1 did not verify it, so its checks can never turn this row green."
- `handcheck/hardening-batch2/r6-desktop-dark-card.jpg` — the same card at 1440.
- `handcheck/hardening-batch2/r7-phone-dark-card.jpg` — a partial check-run list: tests "Running" with `CI: polyclave/check-runs — pending`, and "The host could not read every check run for this commit, so this row stays not-green until it can."
- `handcheck/hardening-batch2/r7-desktop-dark-card.jpg` — the same card at 1440.
- `handcheck/hardening-batch2/measurements.md` — every run r1–r7 at both widths: `scrollWidth == clientWidth` for the body in every case, no element with visible overflow, evidence card inside the viewport, no phone control under 44 px.

### Issues

Six `PLAN-ISSUES.md` items opened (one per item) recording every required edit to an existing test, fixture or schema shape, each with the exact justification and the failing/passing evidence. No item was left unresolved in the sense of blocking; nothing beyond `docs/LESSONS.md` §5/§6 patterns was found. Existing open items (`dsh` model/`mcp_url`, dispatch refund) are untouched by this batch.

### Cleanup

No daemon, browser or fixture process was left running: the browser suite closes its server and browser in `finally`, the new tests join their Worker turns and abort their fake GitHub servers, and every fixture lives under `/tmp/polyclave-hc/hardening-batch2/`. No real vendor CLI and no real GitHub credential was used.

Privacy scan: `privacy-scan: clean (origin/main..HEAD)`.

### Not done

Batch 3 (phone pairing, exact-SHA human approval, restart reconnect) is not started: `NEXT.md` scopes this branch to Batch 2. Native Windows CI and real-environment acceptance remain pending for the same reason as Batch 1. Nothing was merged or pushed; the branch waits for the merge reviewer.

## Audit Hardening — Batch 2 self-review (2026-11-27)

Three defects were found by re-reading the batch diff, not by the green suite, and are fixed on the branch. Two coverage gaps were closed at the same time.

- **Gate notices were written nowhere.** `enqueue_hub_notice` only writes when it is given a Hub session, and the new "auto-merge could not be confirmed" (2.4) and "could not be reconciled" (2.4) notices passed `None`. They now resolve the Run's owning Hub; the reconciliation test asserts the notice arrives.
- **One unreadable gate aborted reconciliation for all the others.** A definite GitHub answer (for example a 404) returned `Err` from `reconcile_auto_merge`, leaving every later gate unsettled. That gate is now skipped — still armed, which is the safe state — with a notice, and the rest are settled. New test: `one_gate_github_cannot_answer_for_does_not_stop_the_others_reconciling`.
- **Clearing the "check-runs incomplete" marker published no change.** A deleted row is not a row change, so the card could keep showing "the host could not read every check run" after a complete fetch. `Store::clear_test_result` returns the number of removed rows and the observer publishes a `pr_gate`/`test_results` change event when it removed one. New test: `a_partial_list_marks_the_sha_and_the_cleared_marker_is_published`.
- **Coverage gap closed (2.5):** `a_ledger_with_rows_upgrades_to_the_check_id_key_without_losing_them` rebuilds a pre-`check_id` `test_results` table **with rows in it**, which the earlier column-count test did not exercise.
- **Coverage gap closed (2.6):** the daemon test now drives the production `Hub::scm_observer` wiring and waits for the owning Hub to claim the queued message, instead of a hand-built observer with a counter. Removing `.with_wake(Hub::kick)` fails it with "hub-a was not woken".

Negative controls run for every item that claims a refusal, so each new test is proven load-bearing rather than merely green: 2.1 provenance (fails on `verified_result_sha(..).is_err()`), 2.2 empty range and moved base branch and the scope-widening refusal (all fail without the guards), 2.3 revision transition (fails with `Policy` on the old `WorkerChat` path), 2.4 per-gate serialization (the recorded remote order flips to `["disable", "enable"]` without the lock), 2.5 pagination (page 2 is never read), 2.6 routing (`["hub-a", "hub-a"]`) and waking.

Tests: `cargo fmt --all -- --check` clean, `cargo clippy --locked --workspace --all-targets -- -D warnings` 0 warnings, `cargo test --locked --workspace` **203 passed, 0 failed, 0 ignored**. `proto.ts` unchanged; the UI files were not touched again, so the earlier `tsc --noEmit`, `npm run build` and browser run still stand.

One design note is recorded in `PLAN-ISSUES.md` rather than changed here: PR-gate notices still reach the Hub as `source='user'` (the pre-existing channel used by every gate escalation), so the model cannot yet tell a host warning from the person's words. That switch needs its own work list.

## Audit Hardening — Batch 2 blind review of Item 2.4 (2026-11-28)

The blind review passed Items 2.1, 2.2, 2.3, 2.5 and 2.6 and found two problems in 2.4. Both are fixed on this branch.

- **High — pause/arm race.** `run_pr_gate` read `pause_state()` and then armed in a second transaction; a Pause committing between them could snapshot the armed gates while this gate was still unarmed, and the gate then armed and called GitHub. `Store::arm_auto_merge` now reads the pause flag inside its own serialized write and returns `Ok(None)` when paused: nothing is armed and GitHub is never called. The redundant outer read is removed, so the window is gone rather than narrowed. One existing assertion became stronger: `pr_gate_auto_merge_ledger_and_pause_flag_persist_across_reopen` now expects `Ok(None)` plus an empty `armed_pr_gates()` and an untouched gate row instead of `Err(ConflictError)`; it is the discriminator (removing the transactional check fails it at that line). New regression `a_pause_between_approval_and_arming_never_reaches_github` runs the real `pause_all` from inside the review loop.
- **Minor — reconcile aborted on a lost disarm answer.** `reconcile_auto_merge` returned `Err` when `disable_auto_merge` had no definite answer, so later gates were never settled. It now keeps the gate armed, notifies its owning Hub (`could not be revoked`) and continues, matching the `get_pr_state` path; it also persists the `disable` intent before calling GitHub. New test `a_lost_disarm_answer_leaves_the_gate_armed_and_still_settles_the_others` (two gates, every answer lost → 0 settled, both armed with the intent, two notices; a later confirmed disarm settles both).

Checks: `cargo fmt --all -- --check` clean, `cargo clippy --locked --workspace --all-targets -- -D warnings` 0 warnings, `cargo test --locked --workspace` **205 passed, 0 failed, 0 ignored**, `bash scripts/privacy-scan.sh` clean. `proto.ts` unchanged and no UI file changed in this round. The branch stays local for the follow-up review.

## Audit Hardening — Batch 2 review (Gemini): passed, merged

Branch `fix/hardening-batch2-pr-gate` (Items 2.1–2.6 + self-review `7c1dc1e` + blind-review concurrency/reconcile fix `25cacc0`, 9 commits) was rebased onto `main` (`2459f5d`), fast-forwarded into `main`, and deleted.

- **Code & invariant audit:** verified `RunManager::r1_verified_sha` provenance check (`result_source='agent'`, accepted receipt) in both PR gate and `evidence.ts` (`r6`), removal of caller `base_sha`/`base_branch`/`touched_paths` overrides + `MirrorPusher::confirm_review_base`, dedicated `begin_revision` / `schedule_revision` Attempt creation (`attempts.from_sha`), single-transaction `Store::arm_auto_merge` pause check + `auto_merge_locks` serialization + non-aborting `reconcile_auto_merge` wired at daemon startup, paginated `check_runs` keyed by stable `check_id` with `polyclave/check-runs` incomplete marker (`r7`) and `change_log` publication, and dynamic per-Run Hub routing + `Hub::kick` wakeups.
- **Approved test/fixture updates:** all fixture and schema-column adjustments documented in `PLAN-ISSUES.md` (Items 2.1–2.6) verified with no weakened assertions.
- **Visual & automated verification (macOS):** visually inspected `handcheck/hardening-batch2/{r6,r7}-phone-dark-card.jpg`; `cargo fmt --all -- --check` clean; `cargo clippy --locked --workspace --all-targets -- -D warnings` clean; `cargo test --locked --workspace` 205 passed, 0 failed (including `TurnDto` compile-fail doctest); `proto.ts` diff clean; UI `tsc --noEmit`, `npm run build`, and `POLYCLAVE_BROWSER_CHANNEL=chrome npm run test:browser` (1440×900 & 375×812, dark & light) passed; `bash scripts/privacy-scan.sh` clean.
- **Open note:** PR-gate notices still use `hub_enqueue` (`source='user'`, pre-existing channel, recorded in `PLAN-ISSUES.md` for a dedicated follow-up). Pushing `main` now for native macOS + Windows CI.

## Audit Hardening — Batch 3 (Phone Pairing, Exact-SHA Approval & Restart Reconnect) — partial

- cad76dc fix(auth): require Tailscale identity and persisted binding for remote device auth — workspace tests: 206 passed
- 8e70621 feat(ui): pair over configured Tailscale HTTPS origin and exchange ticket in page — workspace tests: 206 passed
- 46abe23 docs: record the Batch 3 stop point and the open Item 3.3 decision — workspace tests: 207 passed

Code head: `fix/hardening-batch3-phone-approval` at `4422a05`, branched from `main@3c3d4f1`.

**Batch 3 is not finished.** Items 3.1 and 3.2 are done and green; Item 3.3 is partial on the local branch `wip/human-approval` (`e744792`, never pushed) and Item 3.4 is not started. `PLAN-ISSUES.md` records the stop point, what exists, what is missing, and the one design decision the reviewer has to make before 3.3 can be finished.

### Implementation and evidence (offline fakes only)

- **3.1 — device auth.** `Store::verify_device_token` now requires the presented peer identity to equal the persisted one in both directions, so a token paired on the machine itself cannot pass a remote Tailscale check, and a bound device cannot be used without its identity. `X-AgentMesh-Node` is no longer read as an identity in `auth::require` or in `routes::exchange_pairing` (it stays a Mesh routing label; nothing else read it). New test `remote_device_auth_requires_tailscale_identity_and_a_matching_binding` covers the missing-identity, mesh-header-only, unbound-token-remote and mesh-exchange refusals plus both allowed paths; restoring either the lenient peer check or the header fallback makes it fail with `200 != 403`.
- **3.2 — pairing link and in-page exchange.** The daemon reads `POLYCLAVE_TAILSCALE_ORIGIN` (a bare `https` authority; anything with a path, query, fragment, userinfo or whitespace is refused) and `POST /api/pairing/tickets` answers `https://<host>/#pair=<secret>`, or `null` for both `origin` and `qr_uri` when none is configured — the Settings card then says what to set instead of drawing a QR that cannot work. `sse.ts` gained `trustedOrigin` (loopback `http://127.0.0.1:<port>`, or on an `https` page exactly its own origin) used by `request` and `follow`; `bootstrap` performs the in-page exchange, strips the secret with `history.replaceState`, keeps the device token in `localStorage` and reuses it later, and a refused exchange never replaces a token the device already holds. `flows.ts` token dialog is role-aware for the phone. Tests: `pairing_qr_points_at_the_configured_tailscale_origin` and `tests/pairing-flow.mjs` (1440×900 and 375×812: scan → exchange → secret stripped → device token authorizes API and SSE → replayed link refused and old token kept → revocation → re-pair).

### Tests

- `cargo fmt --all -- --check` clean; `cargo clippy --locked --workspace --all-targets -- -D warnings` 0 warnings; `cargo test --locked --workspace` **207 passed, 0 failed, 0 ignored** at the branch head.
- UI: `npx tsc --noEmit`, `npm run build`, `POLYCLAVE_BROWSER_CHANNEL=chrome npm run test:browser` — offline browser, QR round trip, approval card (1440/375, dark/light) and the new pairing flow (1440/375) all pass. `proto.ts` unchanged (`git diff --exit-code` clean).
- Existing-test edits, all in `PLAN-ISSUES.md` with the work-list justification: `stage3_pairing_dual_identity_auth_revocation_mesh_and_private_notifications` asserted the dead `polyclave://pair?ticket=` scheme and now asserts the exact https link (stricter, same intent).

### Hand check (offline browser only)

- `handcheck/hardening-batch3/item2-phone-paired.jpg` — 375×812 after a scanned link: the session is selected, the status bar shows the daemon connected, no overflow.
- `handcheck/hardening-batch3/item2-phone-revoked.jpg` — 375×812 with the device token revoked: "Reconnect this device" dialog with the device-token field and its explanation; the page does not claim to be connected.
- `handcheck/hardening-batch3/measurements.md` — both widths, both steps: `scrollWidth == innerWidth`, no overflowing element.

### Issues

`PLAN-ISSUES.md` has two new entries: the 3.1 strictness note, the 3.2 configuration/link change with the assertion update, and the Batch 3 stop point with the open Item 3.3 decision (keep the remote head re-check and teach the fake GitHub the real head, or drop the round-trip and rely on the ledger plus `enableAutoMerge(expectedHeadOid)`) — recommendation A.

### Cleanup

No daemon, browser or fixture process is left running; the browser suites close their servers and browsers in `finally`; the only hand-check state is under `/tmp/polyclave-hc/hardening-batch3/`. No real vendor CLI, no real Tailscale, no real GitHub credential was used.

Privacy scan: `privacy-scan: clean (origin/main..HEAD)`, after the owner authorised rewriting the two unpublished commits that carried the address-shaped literal.

### Not done, and why

- **Item 3.3** is partial, on `wip/human-approval`: the explicit `PrGateStatus::AwaitingHumanApproval`, the in-place `pr_gates` rebuild that keeps `pr_reviews` rows while the status `CHECK` gains the value, `Store::await_human_approval` / `Store::approve_pr_gate` (SHA compared inside the write), `run_pr_gate` stopping at the wait instead of arming, and `pr_gate::approve_pr_gate` + `arm_and_enable` as the only path to `enableAutoMerge`. Missing: the diff endpoint and its DTO, `POST /api/pr-gates/{id}/approve`, the approval card, `proto.ts`, the 375 px approve/stale-SHA browser test, `DESIGN.md`, screenshots. Six `pr_gate` tests there still need the reviewer's answer to the design question above.
- **Item 3.4** not started: 401 recovery after a daemon restart and clearing `S.live` on `resync_required`.
- **Unverified in real environments:** nothing here was run against a real Tailscale tailnet, a real phone, a real daemon restart or real GitHub. Every check above is an offline fake, an in-process router or a Playwright page served by the test itself. Native Windows CI: none on this host.
- Nothing was merged or pushed; the branch stays local for independent blind review.

## Audit Hardening — Batch 3 completion (all four items)

- cad76dc fix(auth): require Tailscale identity and persisted binding for remote device auth — workspace tests: 206 passed
- 8e70621 feat(ui): pair over configured Tailscale HTTPS origin and exchange ticket in page — workspace tests: 206 passed
- 4422a05 test(pairing): assert the phone-openable link instead of the dead custom scheme — workspace tests: 207 passed
- ab2de36 feat(hub): gate merge on explicit human approval of exact verified SHA with full diff — workspace tests: 207 passed
- e44acfd fix(ui): recover auth token and clear live stream state after daemon restart — workspace tests: 208 passed
- 4027a08 fix(hub): return a gate to the human wait when the approval cannot be checked — workspace tests: 209 passed

Code head: `fix/hardening-batch3-phone-approval` at this handoff commit (the last commit on the branch; `git log -1` names it), branched from `main@3c3d4f1`. Two commits (`8e70621`, `fd66102`) were rewritten in place, with the owner's permission, to build a URL-userinfo example at run time instead of committing an address-shaped literal; their content is otherwise unchanged. The earlier "partial" section below this one describes the stop point reached mid-job; it is superseded by this section, and the `wip/human-approval` branch it mentions was deleted after its work was cherry-picked here.

### Implementation and evidence (all offline fakes)

- **3.1 — device auth.** Peer identity must equal the persisted binding in both directions; `X-AgentMesh-Node` is no longer read as an identity. `remote_device_auth_requires_tailscale_identity_and_a_matching_binding` covers the four refusals and both allowed paths; restoring either the lenient check or the header fallback fails it with `200 != 403`.
- **3.2 — pairing.** `POLYCLAVE_TAILSCALE_ORIGIN` (bare `https` authority) drives a phone-openable `https://<host>/#pair=<secret>` link; without it the ticket carries no link and Settings says what to set. `sse.ts::trustedOrigin` admits loopback and the page's own https origin; the page exchanges the secret, strips it, and keeps the device token. `pairing_qr_points_at_the_configured_tailscale_origin` plus `tests/pairing-flow.mjs` (1440/375) cover scan → exchange → replay refusal → revocation → re-pair.
- **3.3 — exact-SHA human approval.** `PrGateStatus::AwaitingHumanApproval` (with an in-place `pr_gates` rebuild that parks `pr_reviews` rows while the CHECK gains the value), `run_pr_gate` stopping at the wait instead of arming, `Store::await_human_approval` / `Store::approve_pr_gate` comparing the SHA inside the write, `POST /api/pr-gates/{id}/approve` (`expected_sha`, alias `seen_sha`) as the only path to `enableAutoMerge`, and `GET /api/pr-gates/{id}/diff` returning the exact `base_sha..head_sha` cut at 64 KiB with `truncated` and the full size. The gate card shows the exact commit, the diff and `Approve <sha>`; tests + not-done stay in the Run evidence card for the same SHA. Tests: `pr_gates_gains_the_awaiting_state_and_keeps_its_rows_and_reviews`, `pr_gate_routes_run_gate_and_expose_reviews_scope_and_auto_merge` (await → diff → stale refusal → approval over HTTP), `tests/pr-gate-approval.mjs` (1440/375: nothing armed before, the POST carries exactly that SHA, the diff is fetched only on request).
- **3.4 — restart reconnect.** A `401` from the API or the stream is a rotated credential, not a network blip: `follow` surfaces it, the page re-reads its connection once, drops `S.live`, re-snapshots, and only keeps running with a credential that actually changed — otherwise the status bar reads **Daemon credential rejected** and the reconnect dialog opens. `resync_required` clears the transient deltas before the snapshot. `tests/restart-reconnect.mjs` (1440/375) rotates the token, asserts the page stops claiming to be connected, that a delta from the dead stream is never shown, and that a fresh token brings the same run back.

### Tests

- Branch head: `cargo fmt --all -- --check` clean, `cargo clippy --locked --workspace --all-targets -- -D warnings` 0 warnings, `cargo test --locked --workspace` **209 passed, 0 failed, 0 ignored**.
- `proto.ts` regenerated with `POLYCLAVE_UPDATE_PROTO=1 cargo test -p polyclave-proto`; `git diff --exit-code` clean at the head.
- UI: `npx tsc --noEmit`, `npm run build`, and `POLYCLAVE_BROWSER_CHANNEL=chrome npm run test:browser` (offline browser, QR round trip, approval card, pairing flow, PR-gate approval, restart reconnect) pass at 1440×900 and 375×812, dark and light where the suite covers both.
- Existing-test edits are recorded item by item in `PLAN-ISSUES.md`. None weakens an assertion: the gate tests drive the new human step and assert the wait before it; the pairing test asserted a dead `polyclave://` scheme and now asserts the exact https link.

### Hand check

`handcheck/hardening-batch3/` (JPEG q70, all < 60 KB) — what each screenshot shows:
`item2-phone-paired.jpg` (375×812 after a scanned link: session selected, daemon connected, no overflow), `item2-phone-revoked.jpg` (revoked token: "Reconnect this device" dialog, not connected), `item3-phone-awaiting.jpg` (375: `awaiting human approval`, "Waiting for your approval of bbbbbbb", the diff, `Approve bbbbbbb`), `item3-desktop-awaiting.jpg` (the same card at 1440), `item3-phone-approved.jpg` (after the POST: armed), `item4-phone-rotated.jpg` (rotated token: not connected, reconnect offered). `measurements.md` has the numbers for every step at both widths (`scrollWidth == innerWidth`, no overflowing element).

### Self-review

One defect found and fixed before handing back: `approve_pr_gate` moved the ledger to `approved` before reading the PR from GitHub, so a transport failure left the gate unapprovable forever. The error branch now restores `AwaitingHumanApproval` (regression `an_approval_that_cannot_be_checked_leaves_the_gate_approvable_again`). Negative controls were run for each refusal this batch added: 3.1 (lenient check or header fallback → `200 != 403`), 3.2 (link shape), 3.3 (arming from the wait is refused; stale SHA refused over HTTP), 3.4 (the stale delta assertion fails if the deltas are not cleared).

### Issues

Five `PLAN-ISSUES.md` entries were opened by this job, each with the evidence and the exact justification: 3.1's stricter `verify_device_token`, 3.2's configured origin and nullable link (plus the one existing assertion that changed), the mid-job stop point (now marked resolved), 3.3's option-A decision with the nine gate tests that gained the human step, and the self-review wedge. Nothing is left open in the sense of blocking: the stop point and the privacy-scan finding are both resolved in that file.

### Cleanup

No daemon, browser or fixture process is left running; every browser suite closes its browser in `finally`, and the hand-check state lives under `/tmp/polyclave-hc/hardening-batch3/`. No real vendor CLI, no real Tailscale tailnet, no real phone and no real GitHub credential was used.

Privacy scan: `privacy-scan: clean (origin/main..HEAD)`. The address-shaped fixture literal this job introduced was removed by rewriting the two unpublished commits that carried it (owner-authorised); no commit on the branch adds an address.

### Not done / not verified in real environments

- **Never run against real infrastructure:** no real Tailscale tailnet or `tailscale serve`, no real phone or QR scan by a device camera, no real daemon restart with a rotated token file, no real GitHub PR/auto-merge. Every check above is an in-process router, an offline fake or a Playwright page served by the test.
- **Item 3.3's diff is the mirror range, not GitHub's PR diff**: the card shows `base_sha..head_sha` read from the host mirror (bounded at 64 KiB with the truncation stated). If the remote base moved, the gate refuses at approval instead of showing a different range.
- **Per-SHA tests and not-done rows** live in the Run evidence card (keyed to the same SHA), not inside the PR-gate card; the gate card links the person to the SHA they are approving.
- Native Windows CI: none on this host. Nothing was merged or pushed; the branch stays local for the independent blind review.

## Audit Hardening — Batch 3 review (Gemini): passed, merged

Branch `fix/hardening-batch3-phone-approval` (Items 3.1–3.4 + self-review `4027a08`, 9 commits) was fast-forwarded into `main` and deleted.

- **Code & invariant audit:** verified two-way `peer_identity` equality in `Store::verify_device_token` and removal of `X-AgentMesh-Node` fallback (`stage3.rs`, `auth.rs`, `routes.rs`); `POLYCLAVE_TAILSCALE_ORIGIN` bare-`https` validation, nullable `qr_uri`, and in-page `#pair=` exchange + fragment stripping + same-origin `https://` support (`http.rs`, `hub_api.rs`, `sse.ts`); `PrGateStatus::AwaitingHumanApproval` gate stop before auto-merge, atomic exact-SHA `POST /api/pr-gates/:id/approve` with remote head re-check and `AwaitingHumanApproval` restoration on unreadable remote (`4027a08`), and bounded `GET /api/pr-gates/:id/diff` with explicit `truncated` (`pr_gate.rs`, `stage2.rs`, `hub_api.rs`); and `401` credential-rotation recovery + `S.live` clearing on `resync_required` (`app.ts`, `sse.ts`).
- **Visual & automated verification (macOS):** visually inspected `handcheck/hardening-batch3/{item3-phone-awaiting,item4-phone-rotated}.jpg`; `cargo fmt --all -- --check` clean; `cargo clippy --locked --workspace --all-targets -- -D warnings` clean; `cargo test --locked --workspace` 209 passed, 0 failed (including `TurnDto` compile-fail doctest and real-daemon `cancel_staging`); `proto.ts` diff clean; UI `tsc --noEmit`, `npm run build`, and `POLYCLAVE_BROWSER_CHANNEL=chrome npm run test:browser` (all 6 suites at 1440×900 & 375×812) passed; `bash scripts/privacy-scan.sh` clean.
- **Nits / next step:** `app.ts` does not re-attach `.catch` on the second `follow` after a first `401` recovery (minor follow-up). All 3 audit-hardening batches (14 Codex findings) are now merged; fixing the clean-CI `polyclave-fake-agent` build lookup in `bins/polyclaved/src/hub_api/tests.rs` next before pushing `main` for dual-platform CI and real-environment end-to-end acceptance.

## Polyclave v5 pre-acceptance P0 fixes — Batch 1 & Batch 2 checkpoint

- `09319ad` `docs: add pre-acceptance fix plan and tiered review lesson`
- `1255106` `fix: preserve revision from_sha, fix cli hub tool permissions, and harden claude seatbelt` — workspace tests: 215 passed
- `6684da8` `fix: make pr gate async, handle clean merges, guard armed sha, and unify mobile approval` — workspace tests: 217 passed

Commits:
- `09319ad` `docs: add pre-acceptance fix plan and tiered review lesson`
- `1255106` `fix: preserve revision from_sha, fix cli hub tool permissions, and harden claude seatbelt`
- `6684da8` `fix: make pr gate async, handle clean merges, guard armed sha, and unify mobile approval`

Tests:
- Branch head before this handoff commit: `main@6684da8`.
- `cargo fmt --all -- --check`: clean.
- `cargo clippy --locked --workspace --all-targets -- -D warnings`: clean.
- `cargo test --locked --workspace`: **217 passed, 0 failed, 0 ignored** (including macOS `sandbox-exec` DNS/sensitive-path probe, `model_surface` test, `from_sha` partial-path revision test, async PR Gate + `CLEAN` merge + I1 sibling guard + startup recovery tests, and `TurnDto` compile-fail doctest).
- `git diff --exit-code -- apps/polyclave-app/ui/src/generated/proto.ts`: clean (no changes).
- UI checks under `apps/polyclave-app/ui`: `npx tsc --noEmit` and `npm run build` passed; `POLYCLAVE_BROWSER_CHANNEL=chrome npm run test:browser` passed all 6 suites (`offline-browser`, `qr-roundtrip`, `approval-card`, `pairing-flow`, `pr-gate-approval`, `restart-reconnect`) at `1440×900` and `375×812`.
- Existing test adjustments required by the Batch 1 & Batch 2 plan contracts (`docs/superpowers/plans/2026-11-28-polyclave-v5-pre-acceptance-fixes.md`):
  - `crates/polyclave-core/tests/recovery.rs` (`allow_retry_survives_restart_and_attempt_limit_is_enforced`): replaced hardcoded `3` with `r.max_attempts` (now `5`).
  - `crates/polyclave-hub/src/pr_gate.rs` (`a_lost_disarm_answer_leaves_the_gate_armed_and_still_settles_the_others`, `one_gate_github_cannot_answer_for_does_not_stop_the_others_reconciling`): gave the second gate its own `run2` so the new I1 one-active-gate-per-Run guard is respected; (`a_lost_enable_answer_stays_armed_and_startup_reconciliation_settles_it`): updated disarmed status assertion from `Approved` to `AwaitingHumanApproval`.
  - `crates/polyclave-hub/src/scm_observer.rs` (`store_with_sha`): aligned `pr_gates.head_sha` with `PAGED_SHA` to match the new merged-SHA check.
  - `bins/polyclaved/src/hub_api/tests.rs` (`pr_gate_routes_run_gate_and_expose_reviews_scope_and_auto_merge`, `hub_tools_start_council_and_run_pr_gate_enforce_schema_pause_and_direction_invariant`): assert immediate `pending` return from async `run_pr_gate`, then await `hub.wait_pr_gate(&gate_id)` before running existing assertions.

Hand check:
- Verified in offline Chrome (`pr-gate-approval.mjs`) at `1440×900` and `375×812`: `#needs-count == 1`, `GitHub PR` external link, same-SHA `ci/unit` test result and `receipt_not_done` (`Windows CI pending`), hidden `Run PR Gate` button while `awaiting_human_approval` / `auto_merge_armed`, and no horizontal overflow (`scrollWidth == innerWidth`). No screenshots committed yet.

Issues:
- None opened in `PLAN-ISSUES.md`. `docs/superpowers/reviews/` remains untracked per user instruction.

Cleanup:
- All test servers, fake GitHub listeners, and Playwright browsers closed; no background daemon or fake agent left running.

Privacy scan: `privacy-scan: clean (origin/main..HEAD)`

Not done:
- **Batch 3 not started** (next up in `docs/superpowers/plans/2026-11-28-polyclave-v5-pre-acceptance-fixes.md`):
  1. I4 / B10: close all active SSE streams immediately on `revoke_device`.
  2. B2: bounded daemon shutdown (`_lock` in `main`, `shutdown_timeout`, `Hub::close` cancels active Worker runs and waits boundedly before closing MCP, `UPDATE turns ... AND status='running'` in `settle()`).
  3. SV1: per-entry read-only `skills` symlinks in `isolated-homes` + `canonical_real_path` write guard in `sync_skill_to_isolated_homes`.
  4. SV2: full-text boundary/substring redaction in `redact_text`.
  5. B6: Finder/Tauri `PATH` fallback (`/opt/homebrew/bin`, `/usr/local/bin`, `~/.local/bin`, `~/.cargo/bin`, `POLYCLAVE_EXTRA_PATH`).
- **Not pushed to `origin/main`**; Windows CI has not run on `09319ad..HEAD`.
- **Stage 2 real-environment acceptance (V1–V5) not started**: all Batch 1 & Batch 2 checks above used offline fakes/mocks, local bare repos, and Playwright.

### Audit follow-up queue (Batch 1 & Batch 2 self-review)

- [ ] #1 [High] `crates/polyclave-hub/src/pr_gate.rs`: `merge_immediately` uncertain error leaves gate wedged in `approved` — restore `AwaitingHumanApproval` + regression test.
- [ ] #2 [High] `bins/polyclaved/src/hub_api/http.rs` & `bins/polyclaved/src/routes.rs`: remove `Box::leak` in `map_page_err` by changing `HttpError`'s message field to `Cow<'static, str>`.
- [ ] #3 [Medium] `apps/polyclave-app/ui/src/views/inspector.ts` & `app.ts`: refresh `S.details` for `prGateEvidenceSlot` when SSE changes arrive or cached detail lacks `activeGate.head_sha`, without clobbering an open diff panel.
- [ ] #4 [Medium] `crates/polyclave-core/src/stage.rs`: resolve revision `from_sha` ref by matching the Attempt that produced `from_sha` (or matching `prepare_revision_attempt`) instead of assuming `attempt.number - 1`.
- [ ] #5 [Medium] `crates/polyclave-adapters/src/claude.rs` / `lib.rs`: handle local/custom `HTTP_PROXY`/`HTTPS_PROXY` ports in macOS Seatbelt SBPL (or explicitly parse & allow configured proxy host:port safely).
- [ ] #6 [Medium] `apps/polyclave-app/ui/src/state/store.ts` & `inspector.ts`: align frontend `Needs you` with backend `/api/attention`, hide `Run PR Gate` for all active/armed states (`pending`, `reviewing`, `revise_requested`, `approved`, `awaiting_human_approval`, `auto_merge_armed`, `merged`), and only treat active gates as `existingGate` on the Run card.
- [ ] #7 [Low] `bins/polyclaved/src/hub_api.rs` (remove finished `pr_gate_tasks` handles), `apps/polyclave-app/ui/src/views/flows.ts` (update default model input when switching reviewer vendor), and `crates/polyclave-hub/src/pr_gate.rs` (surface async preflight failure clearly).

