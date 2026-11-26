# How agents work on Polyclave

Several agents build this project. The user relays messages between them by hand. These rules keep each message short and make the work safe to hand from one agent to another.

## Roles

- **User:** decides direction, relays messages between agents, and approves anything destructive.
- **Reviewer:** plans the work, writes the jobs, reviews the work and accepts it, and decides detail-level questions. Claude held this role until 2026-11-26; now it is a remote agent that works **only from GitHub**. It reads the pushed branch, the CI results and the committed screenshots, and it never touches the local machine or writes to the repository. Its handover notes are in `REVIEWER.md`.
- **Implementer (DeepSeek, Gemini or any other agent):** works in the local checkout. It saves the reviewer's messages into the repository, writes the code and tests, runs the hand checks, and pushes. Implementers take turns: only one works in the checkout at a time.

## Files

| File | Written by | What it holds |
|---|---|---|
| `AGENTS.md` | reviewer (saved by the implementer) | these standing rules |
| `REVIEWER.md` | reviewer | the reviewer's handover notes |
| `NEXT.md` | reviewer (saved by the implementer) | the one job to do now |
| `docs/superpowers/plans/*.md` | reviewer (saved by the implementer) | work lists: items, commit messages, tests |
| `docs/superpowers/specs/*.md` | reviewer, from user decisions | the design. Direction changes need the user. |
| `HANDOFF.md` | implementer, append only | progress lines, implementer reports, and the reviewer's verdicts (newest at the end) |
| `PLAN-ISSUES.md` | implementer; resolved by the reviewer or the user | conflicts and questions |
| `DESIGN.md` | implementer, in the same commit as the UI change | the page's design system |
| `handcheck/<job>/` | implementer | hand-check screenshots and measurements, committed with the report |
| `scripts/privacy-scan.sh` | — | must pass before every push |

## The loop

1. The reviewer writes a message to the implementer with up to three parts:
   - **Verdict:** the review of the last job.
   - **Files:** new or replaced files given as full text, each under its path (usually `NEXT.md`, sometimes a work list or a fix to a doc).
   - **Do:** usually "Do NEXT.md".

   The user pastes this message to the implementer, together with: "Read AGENTS.md, then follow the reviewer's message below."
2. **The implementer saves the message first:**
   - Append the Verdict to `HANDOFF.md`, verbatim, under its own heading.
   - Write each file in Files exactly as given.
   - Commit only those paths as `docs: reviewer message <date>`.
   - Then do the job in `NEXT.md`.
3. At the end, the implementer:
   - commits the report and the screenshots;
   - runs `scripts/privacy-scan.sh`;
   - pushes `feat/phase1`;
   - stops.
4. The user tells the reviewer the job is done. The reviewer reads the branch on GitHub (the commits, the CI run, `HANDOFF.md`, `handcheck/<job>/`) and writes the next message.
5. Questions in `PLAN-ISSUES.md` marked `detail` are answered by the reviewer. Questions marked `direction` go to the user.

## Standing rules for implementers

1. **Before you start, read:**
   - `NEXT.md` and the work list it names;
   - the end of `HANDOFF.md`, which holds the latest verdict;
   - any open item in `PLAN-ISSUES.md`.

   The Global Constraints, R1–R3, and the Freeze and Writers rules in `docs/superpowers/plans/2026-11-25-polyclave-phase1.md` always apply.
2. **Branch and commits.**
   - Work on the branch `NEXT.md` names.
   - Make one commit per item, with the message the work list gives.
   - Stage explicit paths with `git add <paths>`. Never use `git commit -a` or `git add -A`.
3. **`pnpm check` must pass after every commit.**
4. **Tests are the contract.**
   - Do not rename, weaken, skip or delete a test.
   - Do not change an existing test unless the work list names that exact change.
   - Timing in tests comes from barriers or process exit events, never from fixed sleeps.
5. **Limits.**
   - Add no dependencies beyond `package.json`.
   - The page makes no external requests.
   - Do not change `docs/`, `AGENTS.md`, `REVIEWER.md` or `NEXT.md`. The one exception is saving a reviewer message verbatim, as step 2 of the loop says.
6. **Pushing.**
   - Push only the branch `feat/phase1`.
   - Push only at the end of a job, or when you stop for a `PLAN-ISSUES.md` item.
   - Before pushing, run `scripts/privacy-scan.sh`. It must print `clean`. If it finds something, fix it with a new commit, never by rewriting history, and run it again.
   - Never push with `--force`. Never push other branches or tags. Never open or merge pull requests.
   - Never run the real `codex` CLI unless the work list says so.
7. **Conflicts.** If the work list conflicts with a test, a rule or the code, stop.
   - Write it in `PLAN-ISSUES.md` with the evidence: command, output, file:line.
   - Mark it `detail` or `direction`, and propose a fix.
   - Commit it, push, and stop. Do not work around it.
8. **Unfinished work.** Finished work that you cannot commit to `feat/phase1` yet goes on a local branch `wip/<item>`. Commit it there and never push it. Say in `PLAN-ISSUES.md` that it exists.
9. **Progress lines.** After each commit, append one progress line to `HANDOFF.md` under the job's heading: `- <sha> <message> — pnpm check: N passed`. If you run out of quota, the next agent continues from that line.
10. **Cleanup.** Stop every host, fake codex and browser you start, and say so in the report.

## Hand checks

- **Where to run them.** Run every hand-check fixture (scratch repos, host state, fake-codex files) under `/tmp/polyclave-hc/<job>/`, never under your home directory. Paths in screenshots then carry no user name.
- **Screenshots.**
  - Save them to `handcheck/<job>/` in the checkout as JPEG at quality 70 or lower. On macOS, `sips -s format jpeg -s formatOptions 70 in.png --out out.jpg` does this.
  - Keep each file under 300 KB, and about 15 files per job at most.
  - Name each file after the item and what it shows, for example `item3-phone-confirm.jpg`.
  - Commit them together with the report.
- **Measurements.** Put the numbers you measured (overflow checks, element sizes, request bodies) in `handcheck/<job>/measurements.md`.
- **Report the screen.** Report only what a screenshot or a command output shows, and name the file. If what you see differs from what the work list asked for, say so. Do not describe the work list instead of the screen. The reviewer compares every claim with the screenshot.
- **Sizes.** Check the desktop at 1440 px and the phone at 375 × 812, unless `NEXT.md` says otherwise.
- **Overflow check.** At 375, for every component you add or change, check that no element has `scrollWidth > clientWidth` with visible overflow. Look at the component's edges in the screenshot, even when the work list does not ask for it.
- **Fake codex.**
  - The turn numbers in `FAKE_CODEX_SCRIPT` count every turn on the host, not per session (the count is stored in `<script>.counter`). Order the script by the order the turns will start.
  - Pass the fake codex as an absolute path.
  - Put `{}` in `<user-codex>/auth.json`.

## Report format (append to `HANDOFF.md`)

```
## <job title from NEXT.md>

Commits: <sha message> per line.
Tests: final `pnpm check` summary, verbatim. Any test you changed, and the work-list line that allowed it.
Hand check: what you saw, each with its file in handcheck/<job>/.
Issues: PLAN-ISSUES.md items opened or resolved.
Cleanup: processes stopped, ports closed.
Privacy scan: its last line.
Not done: anything left, and why.
```

Commit the report and `handcheck/<job>/` as `NEXT.md` says (usually `chore: <job>`). Then run the privacy scan, push `feat/phase1`, and stop.
