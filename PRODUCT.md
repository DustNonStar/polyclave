# Product

<!-- impeccable:product-schema 1 -->

## Platform

desktop and web: a Tauri v2 desktop shell on macOS and Windows, and the same page served by the local daemon to a browser.

## Stack

A lightweight TypeScript UI in `apps/polyclave-app/ui`, compiled with `tsc` and copied into `ui/dist` by `build.mjs`; no UI framework, no external requests. The Rust daemon `polyclaved` serves it and the HTTP and SSE API on `127.0.0.1:4310`. The Tauri v2 shell adds the tray, keychain and native notifications, and starts the daemon when it is not running (v5 design §2, decisions 2 and 3).

## Users

The project owner first: a developer who hands coding work to several vendors' agents and checks in from a Mac and from a phone, equally. Others may use it later, so the page must stay understandable to someone new to it, not only to its author.

## Product Purpose

Polyclave runs coding agents from several vendors (Codex, Claude Code, Antigravity, DeepSeek Harness) under a local daemon. A Hub session plans the work and dispatches Worker Runs within a Ticket's budget. The page is where the user talks to the Hub, advisers and Workers, watches what agents do, and decides on anything the host could not settle by itself. Success means the user always knows what each agent is doing, what finished, and what is waiting on them.

## Positioning

Polyclave checks delivery itself instead of taking an agent's report of completion. A Run is `done` only when the agent's signed receipt (one `submit_result` under that turn's one-time MCP token) names a SHA, the Attempt branch tip in the host's own bare mirror equals that SHA, the Run base is its ancestor, and every commit in the range carries a `Run-Id: <runId>` trailer. The PR gate reviews and pushes only that exact R1-verified SHA, and turns on GitHub auto-merge with `expectedHeadOid` set to it, so the approval does not carry over to a branch that has moved. This is what Polyclave checks; it makes no claim about how other tools work.

## Operating Context

- The daemon runs on the user's Mac or Windows machine. The page is on `127.0.0.1`; a phone reaches it over Tailscale after pairing.
- Sessions, Runs, Attempts, turns and messages come from the daemon's SQLite store, with live updates over SSE (`Last-Event-ID` resume).
- Session roles: `hub` (the manager), `manager` (resident advisers), `worker` (one Run). Hub and managers only read repositories; all writes happen in a Worker's Attempt worktree.
- Run statuses the user must tell apart at a glance: `pending`, `running`, `done` ("delivered (verified)"), `interrupted`, `failed`, `needs_review`.
- Attempt isolation (v5 §4.3) replaces v4's whole-session freeze. A cancel or crash holds only the affected Run. When the host cannot prove an old Worker has stopped writing (macOS, after the Worker started), that Attempt is marked `cancelled` or `superseded`, and the next try opens Attempt `n+1` from a clean checkout of the Run base. Stopping a Hub or manager turn holds nothing.
- User decisions happen on the page: accept a `needs_review` Run with the SHA they saw, fail it, or allow a retry; cancel a turn; retry an interrupted Run; confirm a memory before it becomes `verified`; approve or revoke a paired device.

## Capabilities and Constraints

- `done` means the branch matches the accepted receipt, not that the work meets the request; the page must not overstate it.
- Memory search fuses SQLite FTS5 with a local vector score. That vector is a 128-dimension hashed byte-trigram count (`polyclave_store::local_embedding`, re-exported by `crates/polyclave-hub/src/evolution.rs`), not a learned embedding. Do not call it semantic search on the page.
- Worker and Hub memory proposals are `candidate` until the Hub or the user promotes them to `verified`.
- Notifications go only to the local machine or self-hosted channels on the private network (for example `ntfy`), and carry metadata only.
- Tokens and secrets never appear on the page.
- Real-environment acceptance (real vendor CLIs, installed WebView, real GitHub, a real phone scan) is still pending; see the verification table in `README.md`.

## Brand Commitments

- Name: Polyclave.
- The v5 desktop look is fixed by the design source `docs/superpowers/design/polyclave-v5-desktop-ui.fig` and `DESIGN.md`: Deep Obsidian and Daylight Crystal themes, three columns on desktop and tabs on a phone.
- The user likes the motion in Orca's UI (orca.dev/bots), how agent work animates into view, not its structure (confirmed 2026-11-25). Motion code is clean-room.

## Evidence on Hand

No real usage data or customers yet. Do not invent any.

## Product Principles

1. Never let the page claim more certainty than the host has.
2. What needs the user comes first; everything else can wait.
3. Every irreversible user decision shows exactly what it applies to (which Run, which Attempt, which SHA) before it happens.
4. Dense enough for daily use, plain enough for a newcomer.
