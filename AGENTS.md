# Instructions for implementing agents

1. The work is `docs/superpowers/plans/2026-11-25-polyclave-phase1.md`. Read its Global Constraints, the rules R1–R3, and "Handoff and Acceptance" before writing code, and follow them exactly. The design behind it is `docs/superpowers/specs/2026-11-25-agent-console-design-v4.md`.
2. Work on branch `feat/phase1`, tasks in order, one commit per task with the message the task gives.
3. Test names and assertions in the plan are the acceptance contract. Do not rename, weaken or skip them. If one looks wrong, write why in `PLAN-ISSUES.md` and move on only if later tasks do not depend on it.
4. If `HANDOFF.md` exists, continue from the task after the last one it records, and fix everything under its `## Review` section first.
5. If you are close to a usage limit, stop at a task boundary and write `HANDOFF.md` as the plan describes.
6. Stop after Task 8 and wait for review (Gate A).
7. Do not add dependencies beyond the plan's Tech Stack line, and do not run the real `codex` CLI except in Task 11 Step 4.
