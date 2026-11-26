# Polyclave

A local host that runs coding agents from several vendors (Codex first; Claude, Antigravity and DeepSeek Harness later) as sessions it owns: it starts and resumes them, collects their results through its own MCP tool, records everything in SQLite, and after a crash can say what finished, what can be retried, and what needs a human look. You talk to it from a local web page (later from phones over Tailscale).

**Status:** Phase 1 implemented (local Codex host, SQLite store, crash recovery, HTTP/SSE API, and local web UI).

## Starting the host

Requires Node (`^22.19.0 || >=24.0.0`), `git`, and the `codex` CLI (signed in at `~/.codex/auth.json`) on `PATH`.

```sh
pnpm install
pnpm start
```

Open the URL printed on stdout (default `http://127.0.0.1:4310`). Target repositories for runs must be git work trees whose `origin` remote configures `receive.denyDeletes=true` and `receive.denyNonFastForwards=true`.

### Flags

`node src/main.ts` (or `pnpm start -- ...`) accepts:

- `--state-dir <dir>` — directory holding host state (default `~/.polyclave`). Only one live host process can own a state directory at a time.
- `--port <n>` — loopback port to bind on `127.0.0.1` (default `4310`; pass `0` for an ephemeral port).
- `--codex <cmd...>` — command used to invoke Codex (default `codex`).

## State directory layout

Under `--state-dir` (for example `~/.polyclave`):

- `polyclave.sqlite` — SQLite (WAL) database of sessions, turns, messages, runs, accepted/rejected `submit_result` records, and raw agent JSONL events.
- `host.pid` — lock file containing `{ pid, startedAt, instanceId }`; prevents a second host from starting on the same state directory.
- `codex-home/` — isolated `CODEX_HOME` (`0700`) created by the host with symlinks only to your user `~/.codex/auth.json` and `~/.codex/skills` (never user `config.toml` or MCP servers), so agent turns only reach the host's own MCP server (`host`).
- `worktrees/` — isolated per-run git worktrees (`worktrees/<runId>` on branch `agent/<runId>`).

## Status meanings

- **`frozen` (Session locked)** — An agent turn ended abnormally (`cancel`, `timeout`, `error`, or host crash/loss) while an agent process may still have been writing. Dispatch on that session stops; new runs return `409`, and chat messages wait in the queue until you unlock the session.
- **`needs review` (`needs_review`)** — A task run did not cleanly finish or cannot be trusted automatically (for example, the host stopped while a turn was running, branch checks found unexpected changes, or retries were exhausted). It waits for an explicit human decision.
- **`delivered (verified)` (`done`)** — Both git checks on the target remote (`agent/<runId>` is a fast-forward descendant of `baseSha` with commits carrying the `Run-Id: <runId>` trailer) and a matching `submit_result` payload (verified directly on the remote, or accepted by you) agree.

## How to resolve locks and reviews

1. **Unfreeze a locked session:** Check your machine (or `ps`) to confirm no stray agent or git/test child processes remain, then click **Remove lock…** in the right-hand **Needs you** panel (or `POST /api/sessions/:id/unfreeze` with `{ "note": "..." }`). The note is recorded in the session stream and queued chat messages resume in receipt order.
2. **Resolve a `needs review` run:** Open the run in **Needs you** (or `POST /api/runs/:id/resolve` with `{ "decision": "...", "sha"?: "...", "note": "..." }`) and choose:
   - **Accept** (`accept`) — verify the observed commit on the target remote (supplying `sha` or accepting the current remote tip) and mark the run `done`.
   - **Mark failed** (`fail`) — record the note and mark the run `failed`.
   - **Allow retry** (`allow_retry`) — transition the run back to `interrupted` so you can click **Retry** (`POST /api/runs/:id/retry`).

## Verification

Run typechecking and the automated test suite (uses a deterministic fake-Codex fixture):

```sh
pnpm check
```

Run the gated real-Codex acceptance test (requires a working local `codex` installation with `~/.codex/auth.json`, network access, and explicit opt-in):

```sh
POLYCLAVE_REAL_CODEX=1 pnpm test tests/real-codex.spec.ts
```

## Documents

| What | Where |
|---|---|
| Current design (v4) | `docs/superpowers/specs/2026-11-25-agent-console-design-v4.md` |
| Phase 1 implementation plan | `docs/superpowers/plans/2026-11-25-polyclave-phase1.md` |
| Probe results (T3 rounds, direct calls, host loop, crash recovery) | `docs/superpowers/research/2026-11-24-t3-probe-results.md` |
| Probe scripts used as references | `docs/superpowers/research/probes/` |
| Prior-art survey | `docs/superpowers/research/2026-11-24-prior-art-and-direction.md` |
