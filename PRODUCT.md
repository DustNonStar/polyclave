# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Stack

Plain static HTML, one CSS file and one script, served by the Polyclave host; no framework, no build step, no external requests (phase 1 plan, Task 10; confirmed 2026-11-25).

## Users

The project owner first: a developer who hands coding work to several vendors' agents and checks in from a Mac and from a phone, equally. Others may use it later, so the page must stay understandable to someone new to it, not only to its author.

## Product Purpose

Polyclave runs coding agents from several vendors (Codex first, later Claude Code, DeepSeek Harness, Antigravity) as sessions owned by a local host. The page is where the user chats with sessions, starts tasks, watches what agents do, and decides on anything the host could not settle by itself. Success means the user always knows what each agent is doing, what finished, and what is waiting on them.

## Positioning

The host verifies every task against external state before calling it done, and freezes anything it cannot prove safe until the user resolves it. Similar tools (T3 Code, Orca) trust an agent's own report of completion; Polyclave does not.

## Operating Context

- The host runs on the user's Mac; the page is served on `127.0.0.1` and, later, reached from a phone over Tailscale.
- Sessions, turns, runs and messages come from the host's SQLite store and live updates over SSE.
- States the user must be able to tell apart at a glance: running, queued, frozen; run statuses pending, running, done ("delivered (verified)"), interrupted, failed, needs review.
- User decisions happen on the page: unfreeze a session with a note; accept (with the sha they saw), fail, or allow retry for a run that needs review; cancel a turn; retry an interrupted run.

## Capabilities and Constraints

- Phase 1 page features are fixed by the plan's Task 10: session list with badges, message stream in receipt order with queued messages marked, live agent events, send and cancel, runs with status/detail/sha, new-task form, retry, resolve, unfreeze, a "Needs attention" area, responsive: three tabs under 860 px.
- `done` means the branch matches the accepted result, not that the work meets the request; the page must not overstate it.
- Tokens and secrets never appear on the page.

## Brand Commitments

- Name: Polyclave.
- The user asked for an industrial look: gray-black with fluorescent green and fluorescent orange accents. They like the motion in Orca's UI (orca.dev/bots) — how agent work animates into view — not its structure; the layout is free to follow Polyclave's own needs (confirmed 2026-11-25).

## Evidence on Hand

No real usage data, screenshots or customers yet. Do not invent any.

## Product Principles

1. Never let the page claim more certainty than the host has.
2. What needs the user comes first; everything else can wait.
3. Every irreversible user decision shows exactly what it applies to (which run, which sha) before it happens.
4. Dense enough for daily use, plain enough for a newcomer.
