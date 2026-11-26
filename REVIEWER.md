# Reviewer handover

You are taking over the **reviewer** role on Polyclave: you plan the work, write the jobs, review what implementers deliver, and keep the project moving. The previous reviewer wrote this on 2026-11-26. Read it once in full, then use it as a reference.

`AGENTS.md` holds the rules every agent follows, the file table, the loop and the report format. This file adds what only the reviewer needs: the current state, how to review, how to write a job, the decisions already made, and the lessons learned.

**You work only from GitHub:** `https://github.com/DustNonStar/polyclave`, branch `main`. You never touch the user's machine. Since 2026-11-26 you also write the code yourself, on a branch `dot/<job>`; Claude reviews it in a browser on the user's machine and merges it (`AGENTS.md`, "Dot's branch flow"). When the user gives a job to a local implementer instead, you send it a message as `AGENTS.md` "The loop with a local implementer" describes.

## 1. Read in this order

1. `PRODUCT.md`: what Polyclave is, who uses it, and the product principles.
2. `AGENTS.md`: roles, files, the loop, implementer rules and the report format.
3. `docs/superpowers/specs/2026-11-25-agent-console-design-v4.md`: the design. It is written in Chinese. Lines ending in "（2026-11-2x 用户决定）" are decisions the user made.
4. `docs/superpowers/plans/2026-11-25-polyclave-phase1.md`: the phase 1 plan, its Global Constraints, the rules R1–R3, and the Freeze and Writers rules. These rules always apply.
5. `HANDOFF.md`: the full history. Read the last four sections closely.
6. `NEXT.md` and the work list it names (currently `docs/superpowers/plans/2026-11-26-page-review-3.md`).
7. `DESIGN.md`: the page's design system. Read it before judging any page work.
8. `PLAN-ISSUES.md`: one resolved item, to show the expected format.


## 2. Where things stand (2026-11-26)

- **Phase 1 is built and accepted.** Gate A and Gate B passed. The real-Codex test runs only with `POLYCLAVE_REAL_CODEX=1`. CI (`.github/workflows/check.yml`) runs `pnpm check` on macOS with Node.js 26.x only; older Node.js versions are no longer supported. The host is still macOS-only. See the latest Actions run for current CI status.
- **Page review rounds:**
  - Round 1 (`2026-11-26-page-review-fixes.md`) is done and accepted.
  - Round 2 (`2026-11-26-page-review-2.md`) and its follow-up are done and accepted.
  - Round 3 (`2026-11-26-page-review-3.md`) was done by Gemini, accepted by you, and your follow-up branch was merged (`ed2bc12`).
  - **Resolved:** The no-SHA HTTP test cleanup and the scheduled-run `Runs.idle()` fix were both reviewed and merged; see `HANDOFF.md`.
- **UI critique scores:** 25 → 26 → 26 out of 40 (the method is in section 5). Each round fixed what it targeted. The score stays flat because each fresh reviewer surfaces older defects, and reviewers differ by 4–5 points. Judge progress by the issue list, not by the number.
- **Git:** all work is on `main` (user decision 2026-11-26: implementers push straight to `main`, you review `main`; a job that fails review is fixed in the next job, not reverted unless the user says so). The old `feat/phase1` branch on GitHub is retired; ignore it.
- **Git history:** phase 1 was built locally in about 70 commits and then squashed into one commit before its first push. The squash removed a local path from the history.
  - **Commit hashes mentioned in `HANDOFF.md` before the "Branch published" note are local only and do not exist on GitHub.** Ignore them.
  - Commits after that note are on GitHub.

## 3. Your loop

Default since 2026-11-26: plan the job, do it yourself on `dot/<job>`, wait for green CI, append your report to `HANDOFF.md` on the branch, and tell the user the branch name in Chinese. The steps below are for when the user gives the job to a local implementer instead.

1. The user says an implementer has finished.
2. Review the work on GitHub (section 4).
3. Write **one message** for the user to paste to the implementer, in the format `AGENTS.md` describes:
   - **Verdict:** your review, in the `HANDOFF.md` heading style `## <job> review (<your name>): passed / passed except … / not passed`.
   - **Files:** every file to add or replace, as full text under its path. This is usually `NEXT.md`, plus a new work list in `docs/superpowers/plans/` when the job needs one.
   - **Do:** usually "Do NEXT.md."
4. Above that message, tell the user in Chinese what you found and what comes next.

Keep the message self-contained: the implementer sees only the message and the repository.

## 4. How to review a job (from GitHub)

1. **Commits:** compare `main` from your last verdict to the new head, then read the commit list and the full diff.
2. **CI:** the `check` workflow run for the new head must be green on all four jobs. A red job means the job is not passed. Name the failing job and test.
3. **Tests:** any removed or changed line in an existing test must be one the work list allowed by name.
4. **Code against the rules:**
   - Every value the page interpolates is escaped.
   - Tokens never leave the host.
   - Nothing is marked `done` without a `classifyRun` verdict or a user accept.
   - Nothing writes during a frozen session.
   - No new dependencies and no external requests from the page.
5. **Screenshots:** open every image in `handcheck/<job>/` and compare each claim in the report with what the image shows. Look at the edges of every changed component at 375 px. Implementers have twice described the work list instead of the screen:
   - a phone banner was reported as "under the tabs" while the screenshot showed it above them;
   - a phone overflow was not mentioned at all.

   Where a claim does not match, write that in the verdict.
6. **Privacy:** the report ends with the privacy scan's last line. Skim the diff for local paths, emails and keys anyway, because the repository is public.
7. **When the evidence is not enough to decide**, ask for more in the next message: a named screenshot, a measurement, or a command output to put in `handcheck/<job>/measurements.md`. If your own environment can clone the public repository and run Node.js 26.x, you may run `pnpm check` or a scratch host there (section 5). Never ask the user to run anything for you.

## 5. Recipes (for implementers, or for your own sandbox if you have one)

**Scratch host with demo data.** Work under `/tmp/polyclave-hc/<job>/`.

1. Create the fixtures:
   - a bare `remote.git` with `receive.denyDeletes true` and `receive.denyNonFastForwards true`;
   - a clone `work` with one commit pushed to `main`;
   - `user-codex/auth.json` containing `{}`.
2. Write a `script.json` for the fake codex. Its keys are **global turn numbers in start order**:

   ```json
   {
     "1": [{"commit":{"file":"hello.txt","content":"hi\n","trailer":true}},{"push":true},{"submitPushed":true},{"reply":"done"}],
     "2": [{"commit":{"file":"web.txt","content":"web\n","trailer":true}},{"push":true},{"submitPushed":true},{"wait":"crash"}],
     "3": [{"wait":"chat"},{"reply":"never finished"}],
     "default": [{"reply":"ok"}]
   }
   ```

3. Start the host:
   - Set the environment variables `POLYCLAVE_TEST_USER_CODEX_HOME`, `FAKE_CODEX_SCRIPT`, `FAKE_CODEX_LOG` and `FAKE_CODEX_BARRIER_DIR`.
   - Run `node src/main.ts --state-dir <dir>/state --port <port> --codex node <absolute path>/tests/fixtures/fake-codex.ts`.
   - The fake codex path must be absolute. Turns run inside the task worktree, so a relative path fails with exit code 1.
4. Seed data through the API: `POST /api/sessions {name, cwd}`, `POST /api/runs {sessionId, requestId, instructions}`, `POST /api/sessions/:id/messages {text}`.
5. To get a locked session with a `needs_review` run: wait for `<barriers>/crash/reached`, `kill -9` the host, then start it again on the same state directory.
6. Make the scratch path longer than 100 characters. Short paths hid a desktop overflow bug for two rounds.
7. Stop the host with SIGINT or SIGTERM. The first signal waits for running turns, by design; a second signal stops them.

**UI critique.** This uses the impeccable `critique` method:
- Assessment A is a design review: Nielsen scores, cognitive load, personas, and P0–P3 issues.
- Assessment B is detector and browser evidence: overflow, targets under 44 px, and contrast.

Run A and B one after the other, never in parallel, because they share one browser. Check every P1 against the code before you report it; one claim in round 2 was false. Without a browser of your own, you can ask the implementer for a critique run as a job.

**Changing host behaviour.** Before a work list changes host behaviour, find out which existing tests break:
- If you can run the tests, try the change and run `pnpm check`.
- If you cannot, make the first item of the work list a spike: the implementer applies the change, runs `pnpm check` and lists every failing test, and stops if any existing test fails. You then decide, in the next message, what change each test may get.

Round 2 once had to stop because a third affected test was missed.

## 6. Decisions: yours or the user's

**Direction belongs to the user:** what the product does, changes to the spec, and trade-offs the user would care about. Ask in Chinese, give 2–3 concrete options, and put your recommendation first. Record each answer in the spec with "（YYYY-MM-DD 用户决定）", by putting the spec edit in the Files part of your message.

**Details belong to you:** how something is implemented, test changes that keep what the test proves, layout and copy within `DESIGN.md`, and `PLAN-ISSUES.md` items marked `detail`. Decide them and write down why.

**Decisions already made.** Do not reopen these unless the user does:
- A frozen session's runs can be resolved only after the lock is removed. The host returns 409.
- Unlocking stays the user's call, with a note. The page shows evidence (process group, start and end times, working directory, check commands) and says what happens next. The host never unlocks by itself. The critique agents keep suggesting a host self-check; the user declined it.
- Before accepting, the page shows the commit subject and the changed files, not a full diff. This is round 3, item 1.
- Phone first: desktop and phone (375 px) are equally important.
- The look is the industrial gray-black with fluorescent green and orange from `DESIGN.md`. Fluorescent colour is used only for motion.
- Implementers (DeepSeek or Gemini) commit, hand-check and push `main` themselves after `scripts/privacy-scan.sh` passes. They never force-push and never touch other branches.
- Hand-check screenshots are committed to `handcheck/<job>/`, with fixtures under `/tmp` so that no user name appears in them.

## 7. Standing rules for the reviewer

- **You write code on branches only** (`AGENTS.md`, "Dot's branch flow"): one branch `dot/<job>` per job, never `main`, never force-push. Claude reviews and merges it on the user's machine when the user asks. Commits carry the project account (its profile name is fine). **No AI contributors:** never add `Co-authored-by:` or "Generated with …" lines; `scripts/privacy-scan.sh` rejects them (user decision 2026-11-26).

- **Never ask for anything outside the loop without the user's explicit yes:** a pull request, a merge, a push of another branch, a force-push, rewriting history, or deleting files or branches.
- Never ask for, read, print or repeat an API key, token or password. The user sets keys themselves.
- The repository is public. Any doc you write must contain no local user paths, account names or emails, subscription or plan names, names of other private projects, machine names, or secrets.
- Talk to the user in Chinese. Messages to implementers and repository documents are in English.
- Threat model: the agents on the machine are trusted. The rules guard against mistakes, not attackers.

## 8. Backlog (not yet scheduled)

From the critiques, the user has not chosen these yet:
- There are no keyboard shortcuts at all: no session switching, no jump to Needs you, no arrow keys in the session tablist.
- There is no `aria-live` region, so new stream items, unlock results and decision outcomes are not announced.
- A long "Waiting for agent events…" gives no hint whether the agent has stalled.
- Jargon still reaches the user: "Session thread …", "Writer check possible", "Turn #3" vs "Turn 2".
- There is no help for terms like turn, tip and base. The sidebar help icon from the spec is missing.
- Headings in the run detail skip from h1 to h3.
- These phone targets are under 44 px: New task (34), Stop turn (34), the run crumb (22), and the stream lock line's "Remove lock…" (28).
- `.impeccable/design.json` is out of date against `DESIGN.md`.

Other open items:
- Phase 2 scope (more vendors, phone over Tailscale) is not planned yet.
