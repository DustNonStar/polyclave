# Polyclave

A local host that runs coding agents from several vendors (Codex first; Claude, Antigravity and DeepSeek Harness later) as sessions it owns: it starts and resumes them, collects their results through its own MCP tool, records everything in SQLite, and after a crash can say what finished, what can be retried, and what needs a human look. You talk to it from a local web page (later from phones over Tailscale).

**Status:** design done, phase 1 not implemented yet.

## Documents

| What | Where |
|---|---|
| Current design (v4) | `docs/superpowers/specs/2026-11-25-agent-console-design-v4.md` |
| Phase 1 implementation plan | `docs/superpowers/plans/2026-11-25-polyclave-phase1.md` |
| Probe results (T3 rounds, direct calls, host loop, crash recovery) | `docs/superpowers/research/2026-11-24-t3-probe-results.md` |
| Probe scripts used as references | `docs/superpowers/research/probes/` |
| Prior-art survey | `docs/superpowers/research/2026-11-24-prior-art-and-direction.md` |

Earlier designs (a T3 Code based draft v3.1, an 11-20 console design and the v5.3 dev workflow) came from an earlier private prototype and are not published. The v4 design and the phase 1 plan are what count; where v4 reuses something from them, it is written out in v4.

## For implementing agents

Read `AGENTS.md` first.
