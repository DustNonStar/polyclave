# Polyclave

Polyclave is a local daemon that runs coding agents from several vendors and checks their work itself. A Hub session (the manager you talk to) plans the work and hands coding tasks to Worker Runs. A Worker can be Codex, Claude Code, Antigravity (`agy`) or DeepSeek Harness (`dsh`). Polyclave does not take an agent's word that a task is done. A Run is `done` only when the host's own Git mirror agrees with the agent's signed receipt.

This is v5: a Rust daemon (`polyclaved`), a Tauri v2 desktop shell, and a TypeScript UI. The Node v4 prototype was removed from the tree in `01c8645`; see the last entry of `HANDOFF.md` for what that removal does and does not prove.

The governing design is `docs/superpowers/specs/2026-11-26-polyclave-v5-rust-design.md`.

## Layout

| Path | What it is |
|---|---|
| `bins/polyclaved` | The daemon: HTTP and SSE API on `127.0.0.1`, host MCP server, Hub, PR gate, mesh routes |
| `crates/polyclave-proto` | Shared data types; exports `apps/polyclave-app/ui/src/generated/proto.ts` |
| `crates/polyclave-os` | Process supervision, locks and private files on macOS and Windows |
| `crates/polyclave-store` | The SQLite (WAL) store, FTS5 memory, change log |
| `crates/polyclave-adapters` | Worker adapters for `codex`, `claude`, `agy`, `dsh`, and the host MCP server |
| `crates/polyclave-core` | Mirrors, Attempt worktrees, Run classification, turn runner, recovery |
| `crates/polyclave-hub` | Hub providers (API and CLI), council, PR gate, SCM observer, triage, memory curation, evolution |
| `crates/polyclave-mesh` | Deny-by-default mesh dispatch between machines |
| `apps/polyclave-app/src-tauri` | Tauri v2 desktop shell (tray, keychain, starts the daemon) |
| `apps/polyclave-app/ui` | The page, in TypeScript, served by the daemon from `ui/dist` |

## Build and run

You need Rust stable, Node.js 26 (for the UI build, as in CI), and `git`. Worker and CLI Hub runs also need the vendor CLI on `PATH` and signed in: `codex`, `claude`, `agy` or `dsh`.

Build the UI first:

```sh
cd apps/polyclave-app/ui
npm ci --ignore-scripts
npm run build
```

Then build the daemon and the desktop shell from the repository root:

```sh
cargo build --locked -p polyclaved -p polyclave-app
```

Run the daemon on its own:

```sh
cargo run --locked -p polyclaved
```

It prints one line, `http://127.0.0.1:<port>/#token=<token>`. Open that URL in a browser. The token in the fragment is the local client key.

Or run the desktop shell (`target/debug/polyclave-app`). It looks for a running daemon. If none answers on port 4310, it starts the `polyclaved` binary that sits next to its own executable. Closing the window does not stop the daemon.

### Settings

`polyclaved` takes no command-line flags. It reads two environment variables:

- `POLYCLAVE_STATE_DIR`: the state directory. Default: the platform local data directory plus `polyclave`, which is `~/Library/Application Support/polyclave` on macOS and `%LOCALAPPDATA%\polyclave` on Windows.
- `POLYCLAVE_PORT`: the loopback port. Default `4310`; `0` picks a free port. The desktop shell always uses `4310`.

Only one daemon can own a state directory at a time.

Vendor CLIs keep their normal login. The adapters read it from `CODEX_HOME`, `CLAUDE_CONFIG_DIR`, `AGY_HOME` (or `GEMINI_HOME`) and `DSH_HOME`, falling back to `~/.codex`, `~/.claude`, `~/.gemini` and `~/.dsh`.

### State directory

| Entry | What it holds |
|---|---|
| `polyclave.sqlite` | The only durable state: sessions, Runs, Attempts, turns, messages, memory, Hub ledger, pairing, mesh |
| `host.lock` | Empty file; the daemon holds an OS lock on it while it runs |
| `local-auth.token` | The local client key for this daemon instance; removed on a clean exit |
| `mirrors/<hash>.git` | The host-owned bare mirror for each repository (`receive.denyDeletes`, `receive.denyNonFastForwards`) |
| `staging/<runId>-a<n>.git` | The bare repository each Attempt pushes to; never an R1 target |
| `worktrees/<runId>-a<n>/` | The checkout each Attempt works in |
| `isolated-homes/<vendor>/` | Private vendor homes. They link only the login files and `skills` from your own home, and keep vendor history across Runs |
| `tmp/` | Per-turn MCP config files (`mcp-turn-<id>.json`) and release markers; cleared when a turn ends, is cancelled, or is recovered |
| `hub-read-only/` | Scratch space for the Hub's read-only repository tools |
| `mesh-root.key` | This node's mesh signing key |
| `empty-hooks/` | An empty hooks directory used for host Git commands |

## Platforms

macOS and Windows are the supported hosts. CI (`.github/workflows/check.yml`) runs on `macos-latest` and `windows-latest`: UI typecheck, build and offline browser checks, `cargo fmt`, Clippy with warnings denied, `cargo test --locked --workspace`, the generated `proto.ts` check, and the privacy scan.

Linux is not in CI. The desktop shell prints a message and exits on Linux. A Claude Worker needs verified write confinement: on macOS this is Seatbelt; on every other platform the Claude Worker refuses to start.

## Runs and Attempts

A Run is one coding task on one repository. Its status is one of:

- `pending`: admitted, not started yet.
- `running`: an Attempt is working.
- `interrupted`: stopped, and safe to retry (nothing was pushed, or the mirror could not be read).
- `needs_review`: the host cannot decide by itself. It waits for you to `accept` (with the SHA you saw), `fail`, or `allow_retry`.
- `done`: verified. The agent's signed receipt names a SHA, the Attempt branch tip in the host mirror equals that SHA, the Run base is its ancestor, and every commit in the range carries a `Run-Id: <runId>` trailer.
- `failed`: closed without a result.

`done` means the branch matches the accepted receipt. It does not mean the work meets the request.

Each Run works through numbered Attempts. Attempt `n` has its own worktree, its own staging repository, and the branch `agent/<runId>/a<n>`. When a turn is cancelled or the host crashes:

- If the host can prove the Worker can no longer write (Windows Job Objects; a Worker that never got the start signal), the Attempt can continue or retry in place.
- If it cannot (a macOS Worker that was already running), that Attempt is marked `cancelled` or `superseded`. The next try starts a new Attempt `n+1` from a clean checkout of the Run base. Nothing from the old worktree is reused.

Only the affected Run is held. Other Runs and sessions keep going. Hub and manager sessions are read-only, so stopping one of their turns never holds anything. This replaces v4's whole-session freeze (v5 §4.3).

Once a Run is `done`, the host fetches the verified SHA into your repository as `refs/heads/polyclave/<runId>`.

## Verification status

What the records show today. "Offline" means tests with fake CLIs, fake HTTP servers and local bare repositories. Native CI last passed on `3dec8e4` (the current `origin/main`). The commits merged locally after it (PR gate fixes, pairing QR, publish target, mesh `request_id`, mesh budget A + C, the `dsh` model fix, the approval evidence card) were checked on macOS only; Windows CI has not run on them.

Nothing below has passed a real-environment acceptance. The pending list is in `HANDOFF.md` under "v5 Phase 1B Hub and optical desktop review handoff" (hand checks 1 to 5) and "v5 Phase 1B review: passed, merged".

| Capability | Offline tests and CI | Pending real-environment acceptance |
|---|---|---|
| Worker Run, Codex | Adapter, handshake, staging import, R1 and recovery tests; native CI on macOS and Windows ("v5 Phase 1A Rust host and native checks") | Real `codex` Worker probe (Phase 1B hand check 4) |
| Worker Run, Claude Code | Adapter and stream tests; refusal without confinement; macOS Seatbelt isolation (`78b9a13`), which has no HANDOFF report | Real `claude` Worker probe on macOS (hand check 4); the refusal elsewhere is not a passed probe |
| Worker Run, Antigravity and DeepSeek Harness | Adapter tests with fake scripts only (`69ca86d`); no HANDOFF report | No real run recorded. The real `dsh headless` has no `--model` flag, so a `dsh` Worker with a model set is refused (`2aec8e4`); how to choose a `dsh` model is an open `detail` item in `PLAN-ISSUES.md` |
| CLI Hub (Codex, Claude) | Thread resume, delta continuation and dual-write tests ("v5 Phase 1B review") | Real CLI Hub probe (hand check 5) |
| API Hub (DeepSeek) | Stream and tool-call parsing, durable reasoning replay against a local fake endpoint ("Final DeepSeek follow-up") | Live `deepseek-flash` and `deepseek-v4-pro` calls (hand check 5, "Local addendum") |
| Desktop app and page | Offline Chrome checks at 1440×900 and 375×812 in CI | Installed WKWebView and WebView2, tray, notifications, keychain (hand checks 1 to 3) |
| PR gate | Pushes the exact R1-verified SHA, full-range review, reviewer preflight, origin-based target, one-shot token helper; tested against local bare remotes ("PR gate fixes review", "PR push target from origin review") | Has never pushed to real GitHub. No real PR, auto-merge, or bot-token login over HTTPS |
| Phone pairing | The pairing QR decodes back to the exact URI in Chrome (`tests/qr-roundtrip.mjs`, "Pairing QR fix review") | Never scanned by a phone. Pairing approval and revocation on a real device not checked |
| Mesh dispatch | `worker_run` and `run_status` end to end in daemon tests, with token scope checks; `request_id` idempotency ("Mesh worker_run idempotency review") | No two-machine run. Remote Runs are charged to the primary Ticket, and each receiver caps concurrent Runs per sending node (default 3); tested in one process over loopback only |
| Memory | FTS5 search, candidate and verified states, fused with a local vector score (store tests) | No real-use record. The vector is a 128-dimension hashed byte-trigram count, not a learned embedding |

## Checks

```sh
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
scripts/privacy-scan.sh
```

UI checks, from `apps/polyclave-app/ui`: `npm run typecheck`, `npm run build`, and `POLYCLAVE_BROWSER_CHANNEL=chrome npm run test:browser`.

## Documents

| What | Where |
|---|---|
| Design (v5) | `docs/superpowers/specs/2026-11-26-polyclave-v5-rust-design.md` |
| Work lists | `docs/superpowers/plans/2026-11-26-polyclave-v5-phase1a.md`, `...-phase1b.md`, `2026-11-27-polyclave-v5-stage2.md`, `...-stage3.md` |
| Test migration from v4 | `docs/superpowers/plans/2026-11-27-polyclave-v5-test-migration.md` |
| How agents work here | `AGENTS.md`, `REVIEWER.md` |
| Progress and verdicts | `HANDOFF.md` |
| Open questions | `PLAN-ISSUES.md` |
| Lessons | `docs/LESSONS.md` |
| Product and design system | `PRODUCT.md`, `DESIGN.md` |
| v4 design (history) | `docs/superpowers/specs/2026-11-25-agent-console-design-v4.md` |
