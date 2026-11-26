# Polyclave Phase 1 (Codex host, local web page, SQLite) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Revision 3 (2026-11-25).** Revision 2 was rewritten after the GPT review (`docs/superpowers/plans/2026-11-25-polyclave-phase1-gate-a-revisions.md`); revision 3 applies the Gemini review (`docs/superpowers/plans/2026-11-25-polyclave-phase1-gemini-review.md`, items B1–B8, M1–M5, L1–L3). The user chose the conservative recovery option: whenever an agent that was allowed to run is hard-stopped (host death, cancel) on the same boot, the session is frozen and the user resolves it.

**Goal:** A local host that owns Codex sessions: chat from a local web page, resume across host restarts, cancel, run push-a-branch tasks in their own worktrees that report through a host MCP tool, and after a forced kill never repeat an external action, never mark unfinished work done, and freeze anything it cannot prove safe until the user resolves it.

**Architecture:** The repository root is one standalone package. One Node process holds a SQLite store, a host MCP server (streamable HTTP, JSON responses, one tool `submit_result`), a turn runner that starts one `codex exec` / `codex exec resume` process per turn under a supervisor that waits for the host's go-ahead, a run manager that gives every task its own worktree, branch and fixed base commit, a recovery pass that runs before anything is dispatched, and an HTTP+SSE API serving one static page. The probe scripts in `docs/superpowers/research/probes/` are references for Codex flags, event shapes and the MCP handshake.

> **Superseded runtime (user decision 2026-11-26):** Polyclave now supports Node.js 24.x only (`engines.node` `^24.0.0`, `@types/node` 24.x, CI on macOS Node 24). Where this plan says Node 22, read Node 24.

**Tech Stack:** TypeScript run directly by Node type stripping (Node `^22.19.0 || >=24.0.0`, no build step), `node:sqlite`, `node:http`, `node:net`; zod 4.4.3; dev: vitest 4.1.8, typescript 6.0.3 (typecheck only), @types/node 22.20.0; git CLI; Codex CLI 0.172.0.

**Spec:** `docs/superpowers/specs/2026-11-25-agent-console-design-v4.md` — §6.4–§6.8, §7, §8.2–§8.3, §9.1–§9.2, §13.1, §17 stage 1, §17.1, §19, §20. Probe record: `docs/superpowers/research/2026-11-24-t3-probe-results.md` §8–§9. Orca note: `docs/superpowers/research/2026-11-25-orca-comparison.md`.

## Global Constraints

- Code lives at the repository root (`src/`, `tests/`, `public/`). No `@deepseek-ai/*` dependency. `docs/` changes only where a task says so.
- Style: ESM, single quotes, no semicolons. Relative imports end in `.ts`.
- **Paths are real paths.** Every path the host stores or passes to Codex (session cwd, state dir, worktree, git common dir, bare repo, `CODEX_HOME`) goes through `fs.realpathSync` first; tests create temp dirs with `realpathSync(mkdtempSync(...))`. Codex's macOS sandbox rejects writable roots that are relative or contain a symlink component (`/tmp`, `/var`) (Gemini B8, verified).
- `pnpm check` (= `tsc --noEmit && vitest run`) passes after every task.
- State directory default `~/.polyclave` (`--state-dir`): `polyclave.sqlite`, `host.pid`, `codex-home/`, `worktrees/<runId>/`.
- HTTP and MCP bind `127.0.0.1` only. HTTP default port `4310`; MCP on an ephemeral port.
- Codex is started only through `buildCodexArgs` (Task 2), which puts `--` before the positional arguments: `--json --skip-git-repo-check --ignore-user-config`, `approval_policy="never"`, `sandbox_mode="workspace-write"`, `sandbox_workspace_write.network_access=false`, writable roots given per turn, `mcp_servers.host.url`, `mcp_servers.host.bearer_token_env_var="POLYCLAVE_MCP_TOKEN"`, `mcp_servers.host.default_tools_approval_mode="approve"`.
- `CODEX_HOME=<stateDir>/codex-home` holds exactly two symlinks: `auth.json → ~/.codex/auth.json` and `skills → ~/.codex/skills` (spec §13.1). The Codex environment never contains `GH_TOKEN`, `GITHUB_TOKEN` or any `POLYCLAVE_*` variable except `POLYCLAVE_MCP_TOKEN`.
- **Identity.** Each host start, after taking the lock, creates a random `hostInstanceId`. Ownership of turns is decided by instance id, never by pid. Each turn also records the machine boot time (`sysctl -n kern.boottime`).
- **Launch handshake.** Every agent runs under the supervisor (Task 3). The supervisor starts the agent only after the host has committed the turn as `authorized` with its pgid and start time and then sent `go`. If the control channel closes before `go`, the agent never starts.
- **Writers.** A turn whose agent was authorized and then hard-killed (cancel, crash, host death) may still have tool processes writing: Codex runs commands in new sessions on macOS (spec §20), so an empty process group proves nothing. Only two facts exclude a writer: the turn was never authorized, or the machine has rebooted since (boot time differs). The agent exiting on its own also excludes it.
- **Freeze.** Any authorized turn that is hard-stopped on the same boot freezes its session: recovery finding an old instance's authorized turn, and any user cancel (chat or run; the user is present and can unfreeze at once). The freeze is written in the same transaction that ends the turn. A run on that turn becomes `needs_review`. A frozen session starts no turns at all — no chat, no queue drain, no run start, no correction, no retry — until the user unfreezes it. Frozen state survives restarts.
- **Runs.** Accepting a run is idempotent on `(sessionId, requestId)`. A run gets a fixed `baseSha`, branch `agent/<runId>`, worktree `<stateDir>/worktrees/<runId>` and its own Codex thread. Agents push with `git push <absolute bare-repo path> refs/heads/<branch>:refs/heads/<branch>`; the host checks that same path.
- **Rules R1–R3:**
  - **R1 — Pushed is not done.** `done` only when there is exactly one accepted `submit_result` for the run, the bare repo's tip of the run branch equals its sha, `baseSha` is an ancestor of the tip, the range `baseSha..tip` is non-empty, and every commit in it has the git trailer `Run-Id: <runId>`. Every decision goes through `classifyRun` (Task 4).
  - **R2 — A run holds its session** from acceptance until it is `done`, `failed`, `needs_review` or `interrupted` (database-enforced). User messages meanwhile are stored `queued`.
  - **R3 — One target.** Phase 1 accepts a run only if the session repo's `origin` has one fetch URL and one push URL, both the same absolute path to a local bare repository whose `receive.denyDeletes` and `receive.denyNonFastForwards` are both `true`. The host never changes that repo's config.
- Tokens are per turn, valid only while the turn is `running` and `authorized` and owned by the current instance. They never appear in API responses, SSE, logs or the fake's log.
- SQLite unique violations are detected by `error.errcode === 2067` (`SQLITE_CONSTRAINT_UNIQUE`); `error.code` is only `'ERR_SQLITE_ERROR'`. Hand-written transactions roll back with `try { db.exec('ROLLBACK') } catch {}` so a failed rollback cannot mask the original error. Child-process output from Node 22 may include an `ExperimentalWarning` on stderr; tests parse stdout only.
- **Test-only hooks** exist only behind environment variables named `POLYCLAVE_TEST_*` and are documented where they are used.
- Ids: runs `r` + 8 hex, sessions `s` + 8 hex, host instances 16 hex.

## Review Focus

- Host killed after an agent was authorized, process group already empty, a detached tool still writes later → session frozen, run `needs_review`, no new prompt, retry and queue blocked until unfreeze (Task 8 row 2).
- Same request submitted twice, or resent after a lost response → one run id (Task 1 `accepts a run once per request id`; Task 8 row 1).
- Agent pushes, then exits non-zero after an accepted submit → `done`, not `failed` (Task 6 `pushed, accepted, then non-zero exit is done`).
- User cancels during a run → run `needs_review` with detail `cancelled while the agent was running`, session frozen, no correction, no automatic retry (Task 6).
- Remote unreadable on restart → run stays `interrupted` with `remote check failed: …`; retry re-checks first (Task 7).

## Handoff and Acceptance

Implemented by other agents; accepted by a reviewer who did not write the code.

Rules for implementers:
- Branch `feat/phase1`, tasks in order, one commit per task with the given message.
- Test names and assertions are the contract. Do not rename, weaken or skip them. If one is wrong, write why in `PLAN-ISSUES.md` and continue only with tasks that do not depend on it.
- No dependencies beyond the Tech Stack line. No changes to `docs/` except the probe-record section named in Task 11. Never run the real `codex` except in Task 11 Step 4.
- Handover: stop only at a task boundary (commit the task or discard its partial changes); write `HANDOFF.md` (last task and commit, verbatim `pnpm check` output, `PLAN-ISSUES.md` items, quirks) and commit it as `chore: handoff after Task N`. The reviewer does a handover review (Gate A steps 1, 3–6 for finished tasks) and writes findings under `## Review` in `HANDOFF.md`; the next implementer fixes those first.

**Gate A — after Task 8.** The reviewer:
1. runs `pnpm check` on a clean checkout, on Node 22.19+ and Node 24+ where available;
2. runs `pnpm test tests/milestone.spec.ts` three times in a row (no flakes);
3. reads `classifyRun`, `recover`, `Runs` completion, `submit_result` and every place that starts a turn against R1–R3, Freeze and Writers; any write of `status = 'done'` outside a `classifyRun` verdict or a user `accept` resolution fails the gate;
4. checks every agent start goes through `spawnSupervised` and `buildCodexArgs`, and that the supervisor cannot start the agent without `go`;
5. checks tokens never leave the host and no push target comes from user input;
6. reads `PLAN-ISSUES.md`.
A run marked safe to retry on the evidence "process group empty" alone fails the gate. Tasks 9–11 start only after Gate A passes.

**Gate B — after Task 11.** Repeat 1–6, run `POLYCLAVE_REAL_CODEX=1 pnpm test tests/real-codex.spec.ts` once, start `pnpm start` and do Task 10's manual check including `kill -9` of the host during a run, restart, and resolving the frozen session from the page.

---

## File Structure

| Path | Responsibility |
|---|---|
| `package.json`, `tsconfig.json`, `vitest.config.ts`, `.gitignore` | package setup |
| `src/ids.ts` | ids |
| `src/store.ts` | schema, transactions, `Store` |
| `src/codex.ts` | `buildCodexArgs`, `parseCodexLine`, `prepareCodexHome` |
| `src/proc.ts` | supervised spawn, group liveness, start/boot times, host lock |
| `src/supervisor.ts` | waits for `go`, runs the agent in its group, ends the group when the host is gone |
| `src/git-effect.ts` | target preflight, base sha, worktree preparation, branch check, `classifyRun` |
| `src/mcp.ts` | host MCP server, `submit_result` |
| `src/runner.ts` | `TurnRunner` — turns, queue, cancel, shutdown |
| `src/runs.ts` | `Runs` — accept, start, settle, retry, resolve |
| `src/recovery.ts` | `recover` |
| `src/http.ts`, `src/main.ts` | API, SSE, entry point |
| `public/index.html`, `public/app.js` | the page |
| `tests/fixtures/fake-codex.ts` | scriptable Codex stand-in |
| `tests/fixtures/host-harness.ts` | host without HTTP, for crash tests |
| `tests/helpers.ts` | temp dirs, repo fixture, barriers, `waitFor` |

---

### Task 1: Package skeleton and store

**Files:** Create `package.json`, `tsconfig.json`, `vitest.config.ts`, `src/ids.ts`, `src/store.ts`; modify `.gitignore`; tests `tests/store.spec.ts`.

**Interfaces — Produces:**
```ts
type TurnStatus = 'running' | 'completed' | 'cancelled' | 'interrupted' | 'failed'
type TurnPhase = 'prepared' | 'authorized'
type EndReason = 'exit' | 'cancel' | 'host_lost' | 'never_started' | 'dispatch_closed'
type ClaimMode = 'chat' | 'start' | 'correction' | 'retry'
type WriterCheck = 'excluded' | 'possible'
type RunStatus = 'pending' | 'running' | 'done' | 'interrupted' | 'failed' | 'needs_review'
type MessageSource = 'user' | 'agent' | 'host'
interface SessionRow { id: string; name: string; cwd: string; agentSessionId: string | null; frozenReason: string | null; frozenAt: string | null; createdAt: string }
interface TurnRow { id: number; sessionId: string; runId: string | null; status: TurnStatus; phase: TurnPhase; hostInstanceId: string; bootTime: string; agentPgid: number | null; agentStartedAt: string | null; mcpToken: string; exitCode: number | null; endReason: EndReason | null; writerCheck: WriterCheck | null; startedAt: string; endedAt: string | null }
interface MessageRow { id: number; sessionId: string; turnId: number | null; source: MessageSource; text: string; queued: boolean; at: string }
interface RunRow { id: string; sessionId: string; requestId: string; instructions: string; remote: string; base: string; baseSha: string; branch: string; worktreePath: string; agentSessionId: string | null; status: RunStatus; corrections: number; retries: number; detail: string | null; resultSha: string | null; observedSha: string | null; resultSource: 'agent' | 'user' | null; resolution: string | null; updatedAt: string }
interface ResultRow { id: number; runId: string; turnId: number; accepted: boolean; payload: string; error: string | null; at: string }
type SettleOutcome = 'accepted' | 'already_accepted' | 'different_result_accepted' | 'turn_not_active'
class BusyError extends Error {}      // session has a running turn or an active run
class FrozenError extends Error {}    // session is frozen
class ConflictError extends Error {}  // same request id, different input
class StaleStateError extends Error {} // compare-and-set lost, or the user saw an outdated sha
class Store {
  constructor(path: string)
  createSession(i: { name: string; cwd: string }): SessionRow
  getSession(id: string): SessionRow | undefined
  listSessions(): SessionRow[]
  setSessionThread(id: string, agentSessionId: string): void
  freezeSession(id: string, reason: string): void
  unfreezeSession(id: string): void
  replayRun(i: { sessionId: string; requestId: string; instructions: string; base: string }): RunRow | undefined   // ConflictError if the key exists with other input
  acceptRun(i: { sessionId: string; requestId: string; instructions: string; remote: string; base: string; baseSha: string; worktreePath: (runId: string) => string }): { run: RunRow; created: boolean }
  claimTurn(i: { sessionId: string; mode: ClaimMode; runId?: string; previousTurnId?: number; messageId?: number; hostInstanceId: string; bootTime: string }): TurnRow
  authorizeTurn(turnId: number, pgid: number, startedAt: string | null): boolean
  endTurn(turnId: number, status: Exclude<TurnStatus, 'running'>, f: { exitCode: number | null; endReason: EndReason; writerCheck: WriterCheck; freezeReason?: string }): boolean   // only if still running
  lastChatTurn(sessionId: string): TurnRow | undefined
  lastRunTurn(runId: string): TurnRow | undefined
  turnByToken(token: string, hostInstanceId: string): TurnRow | undefined
  getTurn(id: number): TurnRow | undefined
  runningTurns(): TurnRow[]
  runningTurn(sessionId: string): TurnRow | undefined
  lastTurn(sessionId: string): TurnRow | undefined
  addMessage(i: { sessionId: string; turnId: number | null; source: MessageSource; text: string; queued?: boolean }): MessageRow
  listMessages(sessionId: string): MessageRow[]          // ordered by id (receipt order)
  nextQueued(sessionId: string): MessageRow | undefined
  addEvent(turnId: number, type: string, json: string): void
  getRun(id: string): RunRow | undefined
  listRuns(): RunRow[]
  activeRun(sessionId: string): RunRow | undefined       // status pending or running
  updateRun(id: string, patch: Partial<Omit<RunRow, 'id' | 'sessionId' | 'requestId'>>, expect: RunStatus[]): RunRow   // throws StaleStateError
  setRunThread(id: string, agentSessionId: string): void
  addRejectedResult(i: { runId: string; turnId: number; payload: unknown; error: string }): void
  lastRejectedError(runId: string): string | null
  settleResult(i: { token: string; hostInstanceId: string; runId: string; branch: string; sha: string }): SettleOutcome
  acceptedResult(runId: string): ResultRow | undefined
}
```

Transactions (`BEGIN IMMEDIATE`) and indexes the store must use:
- `acceptRun`: one transaction. If `(session_id, request_id)` exists: same normalized input (`instructions`, `base`) → return it with `created: false` (whatever its status); different → `ConflictError`. Otherwise frozen session → `FrozenError`; another run `pending`/`running` → `BusyError`; else insert `pending` with `worktreePath(runId)`.
- `replayRun`: read-only lookup of `(session_id, request_id)`; same normalized input → the row; different → `ConflictError`; none → `undefined`. Works on busy and frozen sessions.
- `claimTurn`: one transaction. Frozen → `FrozenError`; a running turn on the session → `BusyError`. By `mode` (Gemini B1): `chat` — no run; with `messageId`, that message must be `queued` and unbound, bind it and clear `queued`. `start` — run must be `pending` → `running`. `correction` — run must be `running` and its latest turn must be `previousTurnId` (so a second loop cannot slip in between turns). `retry` — run must be `interrupted` with `retries < 3` → `running`, `retries + 1`, `corrections = 0`. Otherwise `StaleStateError`. Inserts the turn as `running`/`prepared` with a fresh 48-hex token.
- `authorizeTurn`: sets `phase = 'authorized'`, pgid and start time only if the turn is still `running` and `prepared`; returns whether it did.
- `endTurn`: updates only `WHERE id = ? AND status = 'running'` and returns whether it did, so a late exit callback can never overwrite a cancel or a recovery result (Gemini B2). With `freezeReason` it freezes the session in the same transaction. With `endReason = 'never_started'` it also returns any message bound to that turn to `queued = true, turn_id = null` (Gemini M4).
- `settleResult`: one transaction. Re-checks the token's turn is `running`, `authorized`, owned by `hostInstanceId`, with `runId` equal → else `turn_not_active`. If an accepted row exists: same branch and sha → `already_accepted`; else `different_result_accepted`. Otherwise insert the accepted row → `accepted`.
- Indexes: `UNIQUE (session_id) WHERE status = 'running'` on turns; `UNIQUE (session_id) WHERE status IN ('pending','running')` on runs; `UNIQUE (session_id, request_id)` on runs; `UNIQUE (run_id) WHERE accepted = 1` on results.

- [ ] **Step 1: Create the package files.** `package.json`: `"name": "polyclave"`, `"private": true`, `"type": "module"`, `engines.node` `^22.19.0 || >=24.0.0`, `packageManager` `pnpm@11.7.0`; deps/devDeps exactly as the Tech Stack line; scripts `start: node src/main.ts`, `test: vitest run`, `typecheck: tsc --noEmit`, `check: pnpm typecheck && pnpm test`. `tsconfig.json`: `target ES2024`, `module`/`moduleResolution NodeNext`, `lib ["ES2024"]`, `strict`, `skipLibCheck`, `noUncheckedIndexedAccess`, `verbatimModuleSyntax`, `allowImportingTsExtensions`, `rewriteRelativeImportExtensions`, `types ["node"]`, `noEmit`, `erasableSyntaxOnly`; include `src/**/*.ts`, `tests/**/*.ts`. `vitest.config.ts`: `test: { include: ['tests/**/*.spec.ts'], testTimeout: 20000 }`. `.gitignore` keeps its existing lines (`node_modules/`, `*.sqlite`, `*.sqlite-*`, `.DS_Store`, `.state-*/`).

- [ ] **Step 2: Write the failing tests** (`tests/store.spec.ts`)
```ts
it('refuses a second running turn on the same session', () => { /* claimTurn twice → BusyError */ })
it('claims a correction only right after the previous turn of that run', () => { /* mode correction with a stale previousTurnId → StaleStateError */ })
it('claims a retry atomically and at most three times', () => { /* interrupted retries 2 → retry ok (3); again interrupted → retry → StaleStateError */ })
it('endTurn does nothing once the turn has ended', () => { /* endTurn cancelled+possible → true; endTurn failed+excluded → false; row keeps cancelled/possible */ })
it('a never-started turn gives its message back to the queue', () => { /* claim chat with messageId; endTurn never_started → message queued again, unbound */ })
it('replayRun finds the run on a frozen session and rejects changed input', () => { /* … */ })
it('accepts a run once per request id', () => {
  const a = store.acceptRun({ ...req, requestId: 'q1' }); const b = store.acceptRun({ ...req, requestId: 'q1' })
  expect(a.created).toBe(true); expect(b.created).toBe(false); expect(b.run.id).toBe(a.run.id)
  expect(() => store.acceptRun({ ...req, requestId: 'q1', instructions: 'other' })).toThrow(ConflictError)
})
it('lets two connections race on one request id and creates one run', async () => { /* two worker_threads each open the same file and acceptRun the same requestId at a shared start signal → exactly one created */ })
it('refuses a second active run on a session', () => { /* acceptRun q1, acceptRun q2 → BusyError */ })
it('refuses everything on a frozen session and keeps the freeze across reopen', () => { /* freezeSession; acceptRun → FrozenError; claimTurn → FrozenError; reopen Store → frozenReason kept */ })
it('binds a queued message to exactly one turn', () => { /* claimTurn({messageId}) → message.turnId set, queued false; second claimTurn with same messageId → StaleStateError */ })
it('lists messages in receipt order', () => { /* ids ascending regardless of delivery */ })
it('accepts a token only while its turn is running, authorized and owned by this instance', () => { /* prepared → undefined; authorize → found; other instance id → undefined; endTurn → undefined */ })
it('settles one accepted result per run', () => { /* accepted, then same → already_accepted, other sha → different_result_accepted, ended turn → turn_not_active; one accepted row */ })
it('maps the SQLite unique error by errcode 2067', () => { /* violation surfaces as BusyError, never as a raw error */ })
it('counts a retry when an interrupted run is claimed again', () => { /* run interrupted → claimTurn with runId → run running, retries 1 */ })
```

- [ ] **Step 3:** `pnpm test tests/store.spec.ts` → FAIL (module missing).
- [ ] **Step 4: Implement** `ids.ts`, `store.ts` (`DatabaseSync`, `PRAGMA journal_mode = WAL`, `PRAGMA busy_timeout = 5000`, snake_case columns mapped to the interfaces).
- [ ] **Step 5:** `pnpm check` → PASS.
- [ ] **Step 6:** `git add -A && git commit -m "feat: package skeleton and transactional SQLite store"`

---

### Task 2: Codex arguments, event parsing, isolated home

**Files:** Create `src/codex.ts`; test `tests/codex.spec.ts`.

**Interfaces — Produces:**
```ts
type AgentEvent =
  | { kind: 'message.done'; text: string }
  | { kind: 'tool.call'; name: string; input: unknown }
  | { kind: 'tool.result'; name: string; ok: boolean }
  | { kind: 'turn.done'; usage?: unknown }
  | { kind: 'error'; message: string }
interface CodexArgsInput { agentSessionId: string | null; prompt: string; mcpUrl: string; writableRoots: string[] }
function buildCodexArgs(i: CodexArgsInput): string[]
function parseCodexLine(line: string): { threadId?: string; event?: AgentEvent; type?: string } | null
function prepareCodexHome(stateDir: string, userCodexHome?: string): string
function gitCommonDir(cwd: string): string   // realpath of `git -C <cwd> rev-parse --path-format=absolute --git-common-dir`
```

- [ ] **Step 1: Write the failing tests**
```ts
it('starts a new session with exec and the fixed safety flags', () => {
  const a = buildCodexArgs({ agentSessionId: null, prompt: 'hi', mcpUrl: 'http://127.0.0.1:5/mcp', writableRoots: ['/repo/.git', '/r.git'] })
  expect(a[0]).toBe('exec')
  expect(a).toEqual(expect.arrayContaining(['--json', '--skip-git-repo-check', '--ignore-user-config']))
  for (const v of ['approval_policy="never"', 'sandbox_mode="workspace-write"', 'sandbox_workspace_write.network_access=false',
    'sandbox_workspace_write.writable_roots=["/repo/.git","/r.git"]', 'mcp_servers.host.url="http://127.0.0.1:5/mcp"',
    'mcp_servers.host.bearer_token_env_var="POLYCLAVE_MCP_TOKEN"', 'mcp_servers.host.default_tools_approval_mode="approve"']) expect(a).toContain(v)
  expect(a.slice(-2)).toEqual(['--', 'hi'])
})
it('resumes with exec resume <id> <prompt>', () => { /* a.slice(0,2) = ['exec','resume']; a.slice(-3) = ['--','01a2','hi'] */ })
it('puts -- before a prompt that starts with a dash', () => { /* prompt '- fix login' → a.slice(-2) = ['--','- fix login'] */ })
it('gitCommonDir is absolute and has no symlink component', () => { /* repo under realpath(tmpdir) → result === realpathSync(result), absolute */ })
it('keeps paths with spaces and quotes intact', () => { /* root '/a b/"c"' → writable_roots value is JSON.stringify of it; prompt with quotes is one argv element */ })
it('parses thread.started, agent_message, mcp_tool_call, command_execution, turn.completed, turn.failed', () => { /* as in probe §8–§9; command_execution → tool.call 'shell' {command} on start, tool.result 'shell' ok=exit_code===0 on completion */ })
it('returns null for blank or non-JSON lines', () => { /* '' and 'warn: x' */ })
it('prepares a codex home with auth.json and skills symlinks and nothing else', () => { /* exactly ['auth.json','skills'], both symlinks; idempotent; no skills dir → only auth.json */ })
```
- [ ] **Step 2:** run → FAIL. **Step 3: Implement** (`JSON.stringify(roots)` for roots; `prepareCodexHome` default user home `process.env.CODEX_HOME ?? ~/.codex`, dir mode `0o700`). **Step 4:** `pnpm check` → PASS.
- [ ] **Step 5:** `git commit -m "feat: codex argument builder, event parser, isolated CODEX_HOME"`

---

### Task 3: Supervisor, process identity, host lock, fake codex

**Files:** Create `src/proc.ts`, `src/supervisor.ts`, `tests/fixtures/fake-codex.ts`, `tests/helpers.ts`; test `tests/proc.spec.ts`.

**Interfaces — Produces:**
```ts
interface Supervised { child: ChildProcessByStdio<null, Readable, Readable>; pgid: number; go(): void; abort(): void }   // types from node:child_process and node:stream (Gemini B6)
/** Starts `node src/supervisor.ts <ownerPid> -- <command…>` detached with a 4th stdio pipe as control channel. */
function spawnSupervised(command: string[], o: { cwd: string; env: NodeJS.ProcessEnv; ownerPid?: number }): Promise<Supervised>   // resolves after the supervisor reports `ready`
function processStartTime(pid: number): string | null     // `ps -o lstart= -p <pid>` with env LC_ALL=C TZ=UTC, trimmed; null when gone (exit 1 with empty stdout and `kill(pid, 0)` gives ESRCH, including pids above 99999); throws only on other failures
function bootTime(): string                               // `process.env.POLYCLAVE_TEST_BOOT_TIME` if set, else `<sec>.<usec>` parsed from `sysctl -n kern.boottime` (the date text after it depends on the time zone)
type Liveness = 'live' | 'gone' | 'unverifiable'
function groupLiveness(pgid: number, leaderStartedAt: string | null): Liveness
function killGroup(pgid: number, graceMs?: number): Promise<void>
function acquireHostLock(stateDir: string): { instanceId: string; release(): void }   // creates host.pid with openSync(path, 'wx'); if it exists and its process is gone, replaces it atomically (write temp + rename); throws HostRunningError while that process lives
class HostRunningError extends Error { pid: number }
```
Supervisor protocol (Gemini B6): it opens fd 3 as `new net.Socket({ fd: 3, readable: true, writable: true })`, writes `ready\n`, and waits for `go\n`. EOF or `abort` before `go` → exit 0 without starting anything. On `go` it destroys the socket; the host's `go()` writes `go\n` and ends its side. It spawns the command with `stdio: ['ignore', 'inherit', 'inherit']`, not detached (same group), and polls `process.ppid !== ownerPid` every 500 ms.
Termination (owner lost, or SIGTERM/SIGINT/SIGHUP, which it handles once): send SIGTERM to `-process.pid` (ignoring its own copy), arm a 2 s timer, and every 50 ms check whether any group member other than itself remains (`pgrep -g`). When the direct child has exited and no other member remains, clear the timer and exit with the agent's code. When the timer fires, SIGKILL every member except itself (by pid), then exit. Without a termination request it exits with the agent's code when the agent exits.

`groupLiveness`: `pgrep -g` exit 1 → no members → `gone`; exit ≥ 2 → `unverifiable`; leader alive with a different start time → `gone`; leader alive with the same start time → `live`; leader start time `null` → run `pgrep -g` again: no members → `gone`, members → `unverifiable` (their ownership cannot be proven); `ps` failure → `unverifiable`.

`tests/helpers.ts`: `tempDir()`, `makeRepo(): { work, remote, refLog }` (all paths real paths; bare `remote.git` with `main` + one commit, `receive.denyDeletes=true`, `receive.denyNonFastForwards=true`, a `post-receive` hook appending `<ref> <old> <new>` lines to `refLog`; clone `work` with local user name/email), `barrier(dir, name)` → `{ wait(): Promise<void>, release(): void }` using marker files, `waitFor(fn, ms = 10000)`, `FAKE_CODEX`.

Fake codex contract: it claims its turn number atomically (a `mkdirSync` lock next to the script), so concurrent fakes never share a number (Gemini L1). On start it appends `{event: 'prompt_started', turn, resumed, prompt, cwd}` to `FAKE_CODEX_LOG` before anything else, and `{event: 'turn_end', turn}` on normal completion. Script from `FAKE_CODEX_SCRIPT`, keyed by turn number. Steps:
- `{ reply: string }`, `{ sleepMs: number }`, `{ wait: barrierName }`, `{ exit: number; stderr?: string }`;
- `{ commit: { file; content; trailer: boolean } }` — commits in cwd on the current branch with `git commit -m <summary> [--trailer "Run-Id: <runId>"]` (run id parsed from `Task r…` at the start of the prompt);
- `{ push: true }` — logs `{event: 'push_attempt'}`, then runs the push command found in the prompt;
- `{ submitPushed: true }` — calls `submit_result` with the run id from the prompt, the current branch and `git rev-parse HEAD`;
- `{ submitRaw: object }` — calls `submit_result` with exactly that object;
- `{ detachedWriter: { barrier: string; file: string } }` — spawns a process with `detached: true, stdio: 'ignore'` and `unref()` (its own group, holding none of the pipes) that waits for the barrier, then writes `file` into cwd and attempts the push; logs its pid.
It never writes the token to the log.

- [ ] **Step 1: Write the failing tests**
```ts
it('does not start the agent before go', async () => { /* spawnSupervised(['sh','-c','touch started']) then abort → no 'started' file, exit 0 */ })
it('does not start the agent if the host dies before go', async () => { /* owner script spawns, prints pgid, is SIGKILLed before go → no 'started' file */ })
it('runs the agent after go, passes stdout and exit code', async () => { /* ['sh','-c','echo hi; exit 3'] → 'hi\n', code 3 */ })
it('kills a member that ignores SIGTERM after its parent already exited', async () => { /* sh starts a child that traps TERM and sleeps; terminate → that pid gone within 2.5 s; supervisor exits */ })
it('exits promptly when everything stops on SIGTERM', async () => { /* terminate a plain sleep → supervisor gone in well under 2 s */ })
it('ends the agent group within 3 s after its owner is SIGKILLed', async () => { /* grandchild ready marker first, then kill owner → that grandchild pid gone within 3000 ms */ })
it('kills the whole group on killGroup', async () => { /* … */ })
it('groupLiveness: live, gone, reused leader, leaderless members', async () => { /* live; gone after kill; wrong start time → gone; leader exited with member left → unverifiable */ })
it('a detached child escapes the group', async () => { /* agent spawns a detached sleeper; group killed → groupLiveness 'gone' while the sleeper pid is still alive (documents spec §20) */ })
it('refuses a second host lock while the first lives and takes over a dead or reused one', () => { /* host.pid {pid: 999999} → taken over; own pid with another start time → taken over */ })
it('bootTime and processStartTime do not depend on the time zone or locale', () => { /* TZ and LC_ALL changed in process.env → same values */ })
it('fake codex logs prompt_started before running its steps', async () => { /* script with {sleepMs: 5000}; kill it after 200 ms → log has prompt_started */ })
```
- [ ] **Step 2:** run → FAIL. **Step 3: Implement.** **Step 4:** `pnpm check` → PASS.
- [ ] **Step 5:** `git commit -m "feat: supervised launch handshake, process identity, host lock, fake codex"`

---

### Task 4: Target preflight, branch check, verdicts, host MCP

**Files:** Create `src/git-effect.ts`, `src/mcp.ts`; tests `tests/git-effect.spec.ts`, `tests/mcp.spec.ts`.

**Interfaces — Consumes** `Store`. **Produces:**
```ts
function preflightTarget(cwd: string): Promise<{ remote: string }>          // R3; rejects with the reason
function resolveBaseSha(remote: string, base: string): Promise<string>
function prepareWorktree(i: { cwd: string; remote: string; baseSha: string; branch: string; path: string }): Promise<void>   // idempotent; WorktreeConflictError
class WorktreeConflictError extends Error {}
type BranchCheck = { state: 'absent' } | { state: 'present'; sha: string; problem: string | null }
function checkRunBranch(i: { remote: string; baseSha: string; branch: string; runId: string }): Promise<BranchCheck>   // rejects when unreadable
type Facts = { writer: WriterCheck; accepted: ResultRow | undefined; remote: BranchCheck | { state: 'error'; message: string } }
type RunVerdict =
  | { verdict: 'done'; sha: string }
  | { verdict: 'retryable' }
  | { verdict: 'needs_review'; reason: string; observedSha: string | null; freeze: boolean }
  | { verdict: 'check_failed'; message: string }
function classifyRun(f: Facts): RunVerdict
interface HostMcp { url: string; close(): Promise<void>; setOpen(open: boolean): void }
function startHostMcp(d: { store: Store; hostInstanceId: string; check: typeof checkRunBranch }): Promise<HostMcp>
```

`preflightTarget`: fetch URL (`git remote get-url origin`) and push URLs (`git remote get-url --push --all origin`) must be exactly one each, both absolute paths with the same `realpath`, a bare repository, with `receive.denyDeletes` and `receive.denyNonFastForwards` both `true`. Returns the realpath. Reasons: `origin must be a local bare repository`, `origin push and fetch targets differ`, `origin has several push targets`, `remote must set receive.denyDeletes=true and receive.denyNonFastForwards=true`.

`prepareWorktree` (called before every run turn that starts a run or a retry; Gemini B4): fetch `baseSha` from `remote` into the session repo, then `git worktree add -b <branch> <path> <baseSha>`. If the path already exists it is kept only when `gitCommonDir(path) === gitCommonDir(cwd)`, it is on `<branch>`, and `baseSha` is an ancestor of its `HEAD` (uncommitted changes and local commits are kept); anything else → `WorktreeConflictError` (never deletes). Other failures reject with the git error.

`checkRunBranch` reads directly in the bare repo: tip by `git rev-parse --verify --quiet refs/heads/<branch>` (absent if exit 1 with no output; other failures reject); `problem` is the first of: `base <sha8> is not an ancestor of <tip8>`, `no commits after base`, `commit <sha8> lacks trailer Run-Id: <runId>` (trailers read with `git log --format=%H%x00%(trailers:key=Run-Id,valueonly,separator=%x2C) <base>..<tip>`; a `Run-Id:` line in the body that is not a trailer does not count).

`classifyRun` table, first matching row wins:

| Facts | Verdict |
|---|---|
| `writer = possible` | `needs_review`, reason `an agent process may still be writing`, `freeze: true` |
| remote `error` | `check_failed` |
| accepted X, remote present tip X, no problem | `done` X |
| remote present without accepted | `needs_review` `branch pushed but no accepted result` (observedSha = tip) |
| accepted X, remote present tip Y ≠ X | `needs_review` `remote tip Y differs from accepted X` |
| accepted X, remote present tip X with problem | `needs_review` the problem |
| accepted X, remote absent | `needs_review` `accepted X but branch is gone` |
| no accepted, remote absent | `retryable` |
All `needs_review` rows except the first have `freeze: false`. In every `needs_review` row `observedSha` is the remote tip when present, else `null` (Gemini B3).

`submit_result` (arguments `{run_id, branch, sha}`, strict zod, sha 40 hex): while MCP is closed (`setOpen(false)`) every call gets 503. Every handler body is wrapped so a parse or git error becomes a JSON-RPC error, never an unhandled rejection. Token lookup via `turnByToken(token, hostInstanceId)`; unknown → 401. A chat turn (`runId === null`) → `Rejected: this session has no active task` with nothing recorded. Schema error, wrong run id (`run_id <x> is not the active task of this session`), wrong branch (`branch must be <branch>`) → `Rejected: …` and `addRejectedResult` with `turn.runId` (after re-checking the token is still valid; if not → 401). Then `check(...)` outside any transaction; tip must equal sha with no problem, else `Rejected: …`. Then `settleResult`: `accepted` → `Accepted and stored by the host.`; `already_accepted` → `Already accepted.`; `different_result_accepted` → `Rejected: a different result was already accepted for this task`; `turn_not_active` → 401. The final run status is decided later by `classifyRun`.

- [ ] **Step 1: Write the failing tests**
```ts
// git-effect.spec.ts
it('preflight accepts the fixture and rejects URL, split push, several push URLs and missing receive settings', async () => { /* five cases, exact reasons */ })
it('preflight is not fooled by changing origin after run creation', async () => { /* the run keeps its stored remote; checks still read that path */ })
it('prepares a worktree from baseSha and keeps existing work on the second call', async () => { /* local commit and uncommitted file survive */ })
it('refuses a worktree path that holds something else', async () => { /* other repo, other branch, or HEAD not descending from baseSha → WorktreeConflictError; nothing deleted */ })
it('checkRunBranch: absent, clean, wrong base, empty range, body-only Run-Id', async () => { /* exact problem strings; a message "subject\\nRun-Id: r…" without a blank line is not a trailer */ })
it('rejects when the bare repo cannot be read', async () => { /* path removed → rejects */ })
it.each(CLASSIFY_TABLE)('classifyRun: %s', (_n, facts, expected) => { expect(classifyRun(facts)).toEqual(expected) })   // all eight rows
// mcp.spec.ts
it('answers 401 to unknown, prepared-only, other-instance and ended tokens', async () => { /* no result rows written */ })
it('answers 503 while closed', async () => { /* setOpen(false) */ })
it('rejects submit_result from a chat turn without recording anything', async () => { /* … */ })
it('survives a git check that throws', async () => { /* bare repo removed during the call → JSON-RPC error, host still serves the next request */ })
it('accepts a result matching the bare repo', async () => { /* … */ })
it('rejects wrong run id, wrong branch, unknown sha and extra fields', async () => { /* … */ })
it('cancel during the git check wins', async () => { /* check waits on a barrier; endTurn(..., 'cancelled'); release → 401, zero accepted rows */ })
it('two identical concurrent submissions give one acceptance', async () => { /* one 'Accepted…', one 'Already accepted.', one accepted row */ })
it('a different sha after acceptance is rejected and the first stays', async () => { /* … */ })
```
- [ ] **Step 2:** run → FAIL. **Step 3: Implement** (`execFile`, arguments as arrays). **Step 4:** `pnpm check` → PASS.
- [ ] **Step 5:** `git commit -m "feat: target preflight, branch check, verdict table, host MCP"`

---

### Task 5: Turn runner

**Files:** Create `src/runner.ts`; test `tests/runner.spec.ts`.

**Interfaces — Consumes** `Store`, `buildCodexArgs`, `parseCodexLine`, `gitCommonDir`, `spawnSupervised`, `killGroup`, `processStartTime`, `bootTime`. **Produces:**
```ts
type HostEvent = { type: 'message'; message: MessageRow } | { type: 'turn'; turn: Omit<TurnRow, 'mcpToken'> } | { type: 'agent'; sessionId: string; turnId: number; event: AgentEvent } | { type: 'run'; run: RunRow } | { type: 'session'; session: SessionRow }
interface TurnOutcome { turn: TurnRow; reply: string; selfExited: boolean }
interface RunnerDeps { store: Store; mcpUrl: string; codexCommand: string[]; codexHome: string; hostInstanceId: string; extraEnv?: NodeJS.ProcessEnv; bus: EventEmitter }
class TurnRunner {
  constructor(d: RunnerDeps)
  setDispatch(open: boolean): void                   // closed → nothing new starts
  sendUser(sessionId: string, text: string): MessageRow   // stores; starts a chat turn now if the session is free, unfrozen and dispatch is open, else leaves it queued
  runTurn(i: { sessionId: string; text: string; mode: ClaimMode; runId?: string; previousTurnId?: number }): Promise<TurnOutcome>   // BusyError / FrozenError / StaleStateError / DispatchClosedError
  isDispatchOpen(): boolean
  cancel(sessionId: string): Promise<boolean>
  drainQueues(): void
  idle(): Promise<void>
  shutdown(): Promise<void>                          // close dispatch, wait for running turns to finish
  terminateGroups(): void                            // second signal: SIGTERM to every known group, no database writes; recovery handles the rest (Gemini L3)
}
class DispatchClosedError extends Error {}
```
Pinned behavior:
- Launch: `isDispatchOpen()` else `DispatchClosedError` → `claimTurn` → `spawnSupervised` → `authorizeTurn(turn, pgid, processStartTime(pgid))` → `go()`. Anything that fails or returns false before `go()` → `abort()` if a supervisor exists, and `endTurn(..., 'interrupted', { endReason: 'never_started', writerCheck: 'excluded' })` (Gemini B4). Test-only: with `POLYCLAVE_TEST_BARRIER_BEFORE_AUTHORIZE=<dir>` the runner writes `<dir>/reached` after spawning and waits for `<dir>/release` before `authorizeTurn` (Gemini B7).
- After the agent exits (or is cancelled), destroy the child's stdout/stderr streams instead of waiting for them to close (a detached grandchild may hold them).
- cwd and thread: chat turns use `session.cwd` and the session thread; run turns use `run.worktreePath` and the run thread (`setRunThread` on first `thread.started`). Writable roots: chat `[gitCommonDir(session.cwd)]`; run `[gitCommonDir(worktreePath), run.remote]`.
- Environment as in Global Constraints with `POLYCLAVE_MCP_TOKEN: turn.mcpToken`.
- Chat interruption note: only for chat turns, only when `lastChatTurn(session)` ended `interrupted` or `cancelled` with `endReason` other than `never_started`, and no host message starting with `Note from the host: your previous turn was interrupted.` was written after that turn (other host messages, such as an unfreeze note, do not count; Gemini B5): prepend `Note from the host: your previous turn was interrupted. Check the current state before continuing.\n\n` once and store it as a `host` message. Run prompts never get this note.
- End: cancel → `endTurn(..., 'cancelled', { endReason: 'cancel', writerCheck: 'possible', freezeReason: 'cancelled while an agent was running (turn <id>)' })` before killing the group, then emit a `session` event; exit ≠ 0 → `failed`, `exit`, writer `excluded`, stderr tail (last 800 chars) as host message `Codex failed: <tail>`; exit 0 → `completed`, `exit`, `excluded`. `selfExited` is true for both exit cases.
- Queue: a message is queued if the session has a running turn, an active run, is frozen, or dispatch is closed. `drainQueues` claims the oldest queued message per eligible session with `claimTurn({ mode: 'chat', messageId })` (never re-sends a message bound to an authorized earlier turn). Chat turns call `drainQueues` when they end; `Runs` calls it when a run leaves `running`; unfreezing calls it.

- [ ] **Step 1: Write the failing tests** (all with `FAKE_CODEX`)
```ts
it('starts a session thread, records events and the reply', async () => { /* … */ })
it('resumes the session thread on the next message', async () => { /* log turn 2 resumed with turn 1 thread id */ })
it('queues a message during a turn and delivers it after', async () => {
  // turn1 [{wait:'b1'},{reply:'one'}], turn2 [{reply:'two'}]; sendUser 'first'; m2 = sendUser 'second'
  // m2 persisted with queued=true before b1 is released; release b1
  // fake log: second prompt_started comes after turn 1 turn_end; listMessages ids ascending: first, second, one, two
})
it('never starts a turn on a frozen session or with dispatch closed', async () => { /* message stays queued with the same id; zero prompts */ })
it('cancels a running turn: cancelled, writer possible, session frozen, queue held', async () => { /* the exit callback afterwards does not overwrite the row */ })
it('a failure before authorization ends the turn as never started and frees the session', async () => { /* worktree missing → turn interrupted/never_started; next claim works; message re-queued */ })
it('fails the turn when codex exits immediately', async () => { /* failed, writer excluded, host message has stderr */ })
it('adds the chat interruption note exactly once, even after an unfreeze note', async () => { /* … */ })
it('uses the run worktree and run thread for run turns and the session cwd and thread for chat', async () => { /* chat → run → chat: cwd and thread ids from the log */ })
it('does not pass GitHub or Polyclave secrets to codex', async () => { /* … */ })
it('shutdown waits for running turns and starts nothing new', async () => { /* … */ })
```
- [ ] **Step 2:** FAIL. **Step 3: Implement.** **Step 4:** `pnpm check` → PASS.
- [ ] **Step 5:** `git commit -m "feat: turn runner with launch handshake, queue, cancel and shutdown"`

---

### Task 6: Runs

**Files:** Create `src/runs.ts`; test `tests/runs.spec.ts`.

**Interfaces — Consumes** `Store`, `TurnRunner`, `preflightTarget`, `resolveBaseSha`, `prepareWorktree`, `checkRunBranch`, `classifyRun`. **Produces:**
```ts
type Resolution = { decision: 'accept'; sha: string; note: string } | { decision: 'fail' | 'allow_retry'; note: string }
class Runs {
  constructor(d: { store: Store; runner: TurnRunner; stateDir: string; check: typeof checkRunBranch; bus: EventEmitter })
  create(i: { sessionId: string; requestId: string; instructions: string; base?: string }): Promise<{ run: RunRow; created: boolean }>
  start(runId: string): Promise<RunRow>
  retry(runId: string): Promise<RunRow>
  resolve(runId: string, r: Resolution): Promise<RunRow>
}
function runBrief(run: RunRow, resumed: boolean): string
function correctionText(run: RunRow, reason: string): string
```
`create`: `replayRun` first (existing → `{created: false}`; changed input → `ConflictError`); else `preflightTarget(session.cwd)`, `resolveBaseSha`, `acceptRun` (worktree `<stateDir>/worktrees/<runId>`). Test-only: with `POLYCLAVE_TEST_BARRIER_AFTER_ACCEPT=<dir>` it writes `<dir>/reached` after `acceptRun` and waits for `<dir>/release` (Gemini B7). `create` does not start the run.

`start` and `retry` call `prepareWorktree` right before claiming the turn: `WorktreeConflictError` → `needs_review` with the error as detail; any other error → `interrupted`, detail `worktree preparation failed: <message>`; both then `drainQueues()`. `Runs` keeps an in-memory set of run ids being started, retried or resolved; a second call for the same id while one is in progress throws `BusyError` (Gemini B1).

Exact copy (`<remote>` shell-quoted):
```
runBrief:
Task <runId>. <instructions>
You are in a worktree on branch <branch>, created from commit <baseSha>. Commit your work there. Every commit needs a summary line, a blank line, and the git trailer "Run-Id: <runId>" at the end; for example: git commit -m "<summary>" --trailer "Run-Id: <runId>". Push with: git push <remote> refs/heads/<branch>:refs/heads/<branch>
When the branch is pushed, call the submit_result tool of the host MCP server with {"run_id": "<runId>", "branch": "<branch>", "sha": "<pushed commit sha>"}. Then reply with one sentence.
(resumed=true prefixes:) This task was interrupted earlier and its branch is not on the remote. Check this worktree first and reuse any existing commits instead of redoing them.

correctionText:
Task <runId>: the host has no accepted result yet (<reason>). Nothing has been pushed for it. Finish the task in this worktree, commit with the trailer "Run-Id: <runId>", push with: git push <remote> refs/heads/<branch>:refs/heads/<branch> and then call submit_result with {"run_id": "<runId>", "branch": "<branch>", "sha": "<pushed commit sha>"}.
```

Settlement after every run turn — one path: build `Facts` (`writer` from the turn's `writerCheck`), `classifyRun`, then:
| Verdict | Turn ended by | Next |
|---|---|---|
| `done` | any | `done`, `resultSha`, `resultSource 'agent'` |
| `needs_review` | any | `needs_review`, detail = reason (cancel adds `cancelled while the agent was running`), `observedSha`; freeze session if `freeze` |
| `check_failed` | any | `interrupted`, detail `remote check failed: <message>` |
| `retryable` | exit 0, dispatch closed (shutting down) | `interrupted`, detail `host stopped before completion; safe to retry`; corrections unchanged (Gemini M1) |
| `retryable` | exit 0 | if `corrections < 2`: increment, send `correctionText` (reason = `lastRejectedError(run)` or `no result submitted`) with `mode: 'correction'` and `previousTurnId`, repeat; else `failed` `no accepted result after 2 corrections` |
| `retryable` | exit ≠ 0 | `failed`, detail `turn failed` |
| `retryable` | cancel | cannot happen (cancel ⇒ writer `possible`) |
| any | `DispatchClosedError` while starting a correction | `interrupted`, detail `host stopped before completion; safe to retry` |
Every exit from `running` uses `updateRun(..., ['running'])` and then `runner.drainQueues()`.

`retry`: only from `interrupted` and an unfrozen session; `retries >= 3` → `needs_review` `retry limit reached`. Otherwise re-check (`writer: 'excluded'` — a run is only `interrupted` when writers were excluded), `classifyRun`: `done`/`needs_review` apply directly; `check_failed` keeps `interrupted`; `retryable` → `prepareWorktree`, then `runTurn` with `runBrief(run, true)` and `mode: 'retry'` (the claim checks `interrupted` and `retries < 3` atomically and increments `retries`). `retry` returns once the first turn is claimed (or the run settled directly); later turns continue in the background.

`resolve` (user decisions on `needs_review` only, else `StaleStateError`): `accept` re-reads the bare repo: the tip must equal `r.sha`, `baseSha` must be its ancestor and the range non-empty; then `done`, `resultSha = r.sha`, `resultSource 'user'`. If the tip differs, update `observedSha` to the new tip and throw `StaleStateError` so the user reviews the new sha (Gemini B3). `fail` → `failed`. `allow_retry` → only if the remote branch is absent (else `ConflictError`) → `interrupted`. Each stores `resolution` = `<decision>: <note> (seen <sha8|none> at <time>)`. Resolving never unfreezes the session. A run whose session is frozen cannot be resolved: `resolve` throws `FrozenError` before any check, so the user removes the lock first (user decision 2026-11-26).

- [ ] **Step 1: Write the failing tests**
```ts
it('completes when the agent pushes and submits', async () => { /* worktree used; done; resultSha = tip */ })
it('replays a request id without a second run or worktree', async () => { /* … */ })
it('two sessions on one repo work in separate worktrees', async () => { /* both edit same file name; contents correct on each branch; user checkout HEAD and dirty file unchanged */ })
it('pushed, accepted, then non-zero exit is done', async () => { /* [{commit},{push},{submitPushed},{exit:1}] → done */ })
it('pushed but never submitted: needs_review at once, no correction, branch kept', async () => { /* one prompt; detail 'branch pushed but no accepted result' */ })
it('sends up to two corrections when nothing was pushed, then fails', async () => { /* three prompts; failed */ })
it('completes after one correction', async () => { /* turn 1 reply only; turn 2 commit+push+submit */ })
it('fails the run when codex exits immediately', async () => { /* failed */ })
it('cancel makes the run needs_review, freezes the session, no correction', async () => { /* detail contains 'cancelled while the agent was running'; frozenReason set */ })
it('submitted, then pushed again: needs_review because the tip moved', async () => { /* … */ })
it('retry reuses the worktree and its local commit', async () => { /* interrupted run, commit X present locally → after retry remote tip = X, accepted X, one push attempt */ })
it('stops after three retries', async () => { /* … */ })
it('resolve accept records the user decision and the seen sha', async () => { /* … */ })
it('resolve accept refuses when the tip moved after the user looked', async () => { /* observedSha X, remote now Y → StaleStateError, observedSha becomes Y, still needs_review */ })
it('two concurrent retries start one turn', async () => { /* second call → BusyError; one prompt */ })
it('a retry after a crash before the worktree existed creates it', async () => { /* worktree dir missing → retry recreates it from baseSha → done */ })
it('shutdown during a run that pushed nothing leaves it interrupted, not corrected', async () => { /* … */ })
it('does not deliver a user message between the turns of a run', async () => { /* prompts: brief, correction, then the user message */ })
```
- [ ] **Step 2:** FAIL. **Step 3: Implement.** **Step 4:** `pnpm check` → PASS.
- [ ] **Step 5:** `git commit -m "feat: runs with worktrees, one settlement table, retry and resolve"`

---

### Task 7: Recovery

**Files:** Create `src/recovery.ts`; test `tests/recovery.spec.ts`.

**Interfaces — Produces:**
```ts
interface RecoveryReport {
  killedGroups: number[]; unverifiableGroups: number[]
  turns: { id: number; sessionId: string; outcome: 'never_started' | 'killed' | 'already_gone' | 'unverifiable' }[]
  frozenSessions: string[]
  runs: { id: string; outcome: 'done' | 'retryable' | 'needs_review' | 'check_failed' | 'unchanged' }[]
}
function recover(d: { store: Store; hostInstanceId: string; bootTime: string; check: typeof checkRunBranch; liveness?: typeof groupLiveness }): Promise<RecoveryReport>
```
Start order (shared by the harness and `main.ts`): `acquireHostLock` → resolve the state dir with `realpathSync` → open store → MCP started closed and runner dispatch closed → `recover` → `mcp.setOpen(true)`, `runner.setDispatch(true)`, `runner.drainQueues()`.

Rules, idempotent (running it twice gives the same state and no new runs, prompts or pushes):
1. For every `running` turn with another `hostInstanceId`:
   - `prepared` → end `interrupted`, `never_started`, writer `excluded` (its message goes back to the queue).
   - `authorized`, recorded `bootTime` ≠ current → end `interrupted`, `host_lost`, writer `excluded` (rebooted).
   - `authorized`, same boot → `liveness(pgid, startedAt)`: `live` → `killGroup`, wait for `gone`; `gone` → nothing; `unverifiable` → no kill. End `interrupted`, `host_lost`, writer `possible`, `freezeReason: host stopped while an agent was running (turn <id>)` (one transaction).
1a. Sessions not frozen whose latest turn has writer `possible` on the current boot and no `Unfrozen by user:` host message after it are frozen with the same reason (covers a cancel or a second-signal exit whose freeze was not recorded).
2. For every run `pending`, `running` or `interrupted` with no running turn: facts from `lastRunTurn(run)` (`writer` = that turn's `writerCheck`, or `excluded` if it never had a turn, or `excluded` if the user resolved the run after that turn ended — a user decision is never undone by a restart; Gate A review item 1) → `classifyRun` → same mapping as Task 6 except `retryable` → `interrupted`, detail `not on the remote; safe to retry`. A frozen session's runs still get the verdict (the first row makes them `needs_review`).
3. Queued messages and messages bound to interrupted turns stay as they are. Nothing is started or retried.

- [ ] **Step 1: Write the failing tests**
```ts
it('a turn that was never authorized is safe: no freeze, run retryable', async () => { /* … */ })
it('an authorized turn on the same boot freezes the session even when its group is gone', async () => { /* liveness 'gone' → frozen; run needs_review 'an agent process may still be writing' */ })
it('kills a live old group, still freezes', async () => { /* … */ })
it('never kills when liveness is unverifiable', async () => { /* … */ })
it('a different boot time clears the writer', async () => { /* bootTime differs → no freeze; run classified from the remote */ })
it('owns turns by instance id, not pid', async () => { /* old turn with hostPid equal to process.pid but another instance → recovered */ })
it('leaves turns of the current instance alone', async () => { /* … */ })
it('recovers a pending run that never got a turn', async () => { /* remote absent → interrupted retryable; request id still maps to it */ })
it('keeps a run interrupted when the bare repo cannot be read', async () => { /* … */ })
it('confirms an accepted result left by the dead host when writers are excluded', async () => { /* never-authorized second turn scenario or reboot → done */ })
it('running recovery twice changes nothing more', async () => { /* … */ })
```
- [ ] **Step 2:** FAIL. **Step 3: Implement.** **Step 4:** `pnpm check` → PASS.
- [ ] **Step 5:** `git commit -m "feat: startup recovery with instance ownership and session freeze"`

---

### Task 8: Milestone — crash experiments, before any UI

This task is a gate. **Stop after it and hand the branch to the reviewer.**

**Files:** Create `tests/fixtures/host-harness.ts`, `tests/milestone.spec.ts`.

`host-harness.ts <stateDir> <command> [args]` (env `FAKE_CODEX_SCRIPT`, `FAKE_CODEX_LOG`, and the `POLYCLAVE_TEST_*` hooks) runs the start order of Task 7, then:
- `start-run <workDir> <requestId> <instructions>`: create session if none, `Runs.create` + `start`, print `{run}` JSON, exit 0;
- `recover`: print `{report, runs, sessions}` JSON, exit 0;
- `serve`: after recovery print `{ready: true, mcpUrl, report}` on stdout, then read JSON-line commands on stdin — `{id, op: 'create-run' | 'start-run' | 'retry' | 'resolve' | 'unfreeze' | 'send', …args}` — answer each with `{id, ok, result | error}` on stdout, run commands concurrently, and exit when stdin closes (Gemini B7);
- `retry <runId>` / `resolve <runId> <decision> <note>` / `unfreeze <sessionId>`: do it, print the row, exit 0.
Each test uses its own state dir, bare repo and logs. Barriers, not sleeps, decide timing; timeouts only fail tests. Push attempts come from the fake log; ref updates from the bare repo's `refLog`.

| Test | Must observe |
|---|---|
| `same request twice, lost response, and a kill before the first turn` | via `serve`: two concurrent `create-run` with one request id → one run id; kill at `POLYCLAVE_TEST_BARRIER_AFTER_ACCEPT` → after restart a resend returns the same id, the run is `interrupted` and only an explicit retry starts a turn (and creates the worktree); two concurrent `retry` → one turn |
| `a detached tool writes after the host and the group are gone` | agent starts `detachedWriter`, test kills harness and confirms the group is empty; recovery → session frozen, run `needs_review`, zero new prompts; then release the writer: its file appears and its push lands — proving an empty group was not proof — and still no new prompt, run unchanged |
| `supervisor killed, a member left behind` | recovery reports it `unverifiable`, does not kill it, session frozen |
| `half done, pushed, host killed` | `needs_review`; ref equals the first push; after two recoveries: push attempts 1, ref updates 1, prompts 1, corrections 0; session frozen |
| `killed before authorization` | kill at `POLYCLAVE_TEST_BARRIER_BEFORE_AUTHORIZE` → no `prompt_started`; recovery → `interrupted`/retryable, no freeze; retry → `done`, one commit |
| `killed after authorization, unknown writer` | frozen; `retry` and queue refused until `unfreeze`; after `unfreeze` and `resolve allow_retry` (remote absent) a retry runs |
| `local commit exists, never pushed, writer excluded` | prepared scenario with `POLYCLAVE_TEST_BOOT_TIME` changed (reboot); retry reuses commit X; tip and accepted sha are X; one push attempt |
| `accepted, then host killed before done was written` | with reboot boot time: `done`, `resultSha = X`, one accepted row; same with the remote moved to Y afterwards → `needs_review` keeping Y |
| `second host refused while the first lives; old token refused after restart` | second harness exits non-zero and changed nothing; after the first dies, `serve` harness answers 401 to the old token, accepted rows unchanged |

- [ ] **Step 1:** write the tests. **Step 2:** `pnpm test tests/milestone.spec.ts` → PASS (fix failures in the owning task with a regression test there). **Step 3:** `pnpm check` → PASS.
- [ ] **Step 4:** `git commit -m "test: milestone crash experiments"` and stop for review.

---

### Task 9: HTTP API, SSE and entry point

**Files:** Create `src/http.ts`, `src/main.ts`; test `tests/http.spec.ts`.

| Method | Path | Body | Result |
|---|---|---|---|
| GET | `/api/sessions` | — | sessions with `busy` (= running turn or active run), `lastTurnStatus`, `frozenReason` |
| POST | `/api/sessions` | `{ name, cwd }` | `201`; `400` if `cwd` is not a git work tree |
| POST | `/api/sessions/:id/unfreeze` | `{ note }` | session; stores the note as a `host` message `Unfrozen by user: <note>`, emits `message` and `session` events, then `drainQueues()` |
| GET | `/api/sessions/:id/messages` | — | messages |
| POST | `/api/sessions/:id/messages` | `{ text }` | `202 { message }` |
| POST | `/api/sessions/:id/cancel` | — | `{ cancelled }` |
| GET | `/api/runs` | — | runs |
| POST | `/api/runs` | `{ sessionId, requestId, instructions, base? }` | `201` new (then `runs.start` in the background), `200` replay; `409` busy, frozen or conflicting replay; `400` preflight failure |
| POST | `/api/runs/:id/retry` | — | `200` once the first turn is claimed or the run settled; `409` when the run is not `interrupted`, the session is frozen or busy, or a retry is already in progress |
| POST | `/api/runs/:id/resolve` | `{ decision, sha?, note }` | `409` when the run is not `needs_review`, its session is frozen, the sha is outdated, or `allow_retry` meets a pushed branch |
| GET | `/api/stream` | — | SSE of `HostEvent`, `: ping` every 15 s |
| GET | `/`, `/app.js` | — | `public/` |

`main.ts`: flags `--state-dir`, `--port`, `--codex <cmd…>`; Task 7 start order; prints `polyclave listening on http://127.0.0.1:<port>`. First SIGINT/SIGTERM: `runner.shutdown()` then exit; a second one: `runner.terminateGroups()` and `process.exit(130)` without database writes. Every error class maps to a status: `BusyError`, `FrozenError`, `ConflictError`, `StaleStateError`, `DispatchClosedError` → 409; preflight failures → 400; unknown ids → 404.

- [ ] **Step 1: Write the failing tests**
```ts
it('never exposes MCP tokens', async () => { /* no token in any GET body or SSE event */ })
it('stores POSTed messages with source user', async () => { /* … */ })
it('replays a run request id and rejects a changed one', async () => { /* 201, 200 same id, 409 */ })
it('returns 409 for a frozen session and accepts unfreeze with a note', async () => { /* … */ })
it('maps busy and frozen to 409, never 500', async () => { /* … */ })
it('POST /api/runs returns before the run finishes', async () => { /* fake turn waits on a barrier; response arrives first */ })
it('unfreeze delivers waiting messages', async () => { /* … */ })
it('streams events over SSE', async () => { /* … */ })
it('returns 404 for an unknown session', async () => { /* … */ })
```
- [ ] **Steps 2–5:** FAIL → implement → `pnpm check` PASS → `git commit -m "feat: HTTP and SSE API and host entry point"`

---

### Task 10: The page

**Files:** Create `public/index.html`, `public/app.js`; `tests/http.spec.ts` gains `serves the page`.

**Design (approved by the user 2026-11-25):** build the page to `DESIGN.md` (tokens, components, the status-glyph system and the motion rules) and match the approved mockup `docs/superpowers/design/page-mockup.html` — same palette, radii, glyphs and motion; the mockup's sample data and demo script are not shipped. Fluorescent colours only for in-progress motion; finished states use the normal colours. The direction note is `.impeccable/surfaces/public-index-html.md`. The mockup lays out three areas plus a status bar on desktop and tabs on phones; Task 10's feature list below is the requirement, the mockup shows how it looks.

Mockup elements in phase 1 scope (build them, fed by the API): brand; session cards with avatar, status corner dot and nested run rows; session tabs for open sessions; the stream (system lines, user and agent messages, "Worked for" blocks with steps from agent events, activity lines with chips, queued messages); the composer with the send button and a read-only label of the model in use; the right panel's "Needs you" detail (title, status, chips, key-values, activity timeline from run and turn rows, the lock card with unlock, and the decision pills mapped to `resolve`/`retry`); the status bar from `/api/sessions` and `/api/runs`.
Mockup elements NOT in phase 1 (leave them out, do not draw dead controls): search, the Runs and Activity nav entries and panel tabs, attach, model switching, the pause-dispatch, settings and help buttons. The design for them is kept for later stages.

Plain HTML + one script, no framework, no external requests, responsive (one column under 860 px, tabs for Sessions / Stream / Needs you):
- Sessions list with `running` / `queued` / `frozen` badges; a frozen session shows its reason and an "Unfreeze" button that asks for a note.
- Messages in receipt order; queued ones marked "waiting"; each delivered user message shows which turn it went to; live agent events collapsible; Send and Cancel.
- Runs: id, status, detail, sha. `done` is labelled **"delivered (verified)"** — the branch matches the accepted result; whether the work meets the request is judged by tests or review. "New task" form (`instructions`, `base`; target shown read-only; the page generates `requestId` once per form submission and reuses it on resend). Retry on `interrupted`. On `needs_review`: Accept / Fail / Allow retry with a note, showing the observed sha; Accept sends that sha, and a `409` shows the new sha to review. "Needs attention" lists frozen sessions and `needs_review` / `interrupted` runs first.

- [ ] **Step 1:** test `serves the page` (200, `text/html`, `<script src="/app.js">`). **Step 2:** FAIL.
- [ ] **Step 3:** implement and check by hand: `node src/main.ts --state-dir "$(realpath "$(mktemp -d)")" --codex node "$PWD/tests/fixtures/fake-codex.ts"` with a reply script; create a session on a scratch repo made like `makeRepo`; two quick messages; cancel; a task to `done`; `kill -9` the host during a task, restart, see the frozen session, unfreeze with a note, resolve the run; 375 px layout.
- [ ] **Step 4:** `pnpm check` → PASS. **Step 5:** `git commit -m "feat: local web page"`

---

### Task 11: Acceptance end to end, then real Codex

**Files:** Create `tests/e2e.spec.ts`, `tests/real-codex.spec.ts`, `README.md`.

- [ ] **Step 1: End-to-end tests** (real `main.ts` child processes, fake codex):
```ts
it('host killed during a run: restart shows the frozen session; unfreeze and resolve work over HTTP', async () => { /* … */ })
it('chat finished, then restart: same thread, no interruption note', async () => { /* … */ })
it('chat killed after thread.started and before the turn ended: frozen; after unfreeze the next message resumes the same thread with the note once', async () => { /* … */ })
it('a second host on the same state dir refuses to start', async () => { /* … */ })
```
- [ ] **Step 2:** `pnpm test tests/e2e.spec.ts` → PASS.
- [ ] **Step 3: Gated real-Codex test** (skipped unless `POLYCLAVE_REAL_CODEX=1`):
```ts
it('real codex: chat, resume, a task delivered and verified', async () => {
  // chat: 'Remember ORCHID-7. Reply ok.' then 'Which word?' → reply contains 'ORCHID-7'
  // run: 'Add a file hello.txt containing exactly hi.' → status done; `git -C <remote> show <resultSha>:hello.txt` is 'hi'
  // codex-home/skills resolves to ~/.codex/skills; no MCP call to any server other than 'host'
}, 300_000)
```
- [ ] **Step 4:** run once: `POLYCLAVE_REAL_CODEX=1 pnpm test tests/real-codex.spec.ts` → PASS (all temp paths are real paths, or Codex's sandbox refuses them); record duration and outcome in `docs/superpowers/research/2026-11-24-t3-probe-results.md` as `## 10. Phase 1 acceptance (real Codex)`.
- [ ] **Step 5:** `README.md`: start, flags, state, what frozen / needs review / delivered (verified) mean, how to resolve, the gated test.
- [ ] **Step 6:** `pnpm check` → PASS. **Step 7:** `git commit -m "test: end-to-end acceptance and real Codex check"`

---

## Out of scope (later stages, spec §17 and §20)

Claude adapter, direction-level council, GitHub PRs, reviewers and shadow-mode merging (stage 2, with spec §20's review rules), fencing old attempts instead of freezing (stage 2), Tailscale and phone apps, push, Antigravity, DSH, flow engine, evolution loop.
