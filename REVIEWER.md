# Reviewer handover

You are taking over the **reviewer** role on Polyclave: you plan the work, write the jobs, review what implementers deliver, and keep the project moving. The previous reviewer wrote this on 2026-11-26. Read it once in full, then use it as a reference.

`AGENTS.md` holds the rules every agent follows, the file table, the loop and the report format. This file adds what only the reviewer needs: the current state, how to review, how to write a job, the decisions already made, and the lessons learned.

**You work only from GitHub:** `https://github.com/DustNonStar/polyclave`, branch `main`. You never touch the user's machine. Since 2026-11-26 you also write the code yourself, on a branch `dot/<job>`; Claude reviews it in a browser on the user's machine and merges it (`AGENTS.md`, "Dot's branch flow"). When the user gives a job to a local implementer instead, you send it a message as `AGENTS.md` "The loop with a local implementer" describes.

## 1. Read in this order

1. `PRODUCT.md`: what Polyclave is, who uses it, and the product principles.
2. `README.md`: v5 Rust daemon + Tauri v2 architecture and the per-capability verification status table.
3. `AGENTS.md`: roles, files, the loop, implementer rules and the report format.
4. `docs/superpowers/specs/2026-11-26-polyclave-v5-rust-design.md`: the v5 Rust design spec (plus resolved owner decisions in `PLAN-ISSUES.md`).
5. `PLAN-ISSUES.md`: open and resolved `detail`/`direction` items and their rationale.
6. `HANDOFF.md`: the full history. Read the latest sections closely.
7. `NEXT.md` and any active work list it names.
8. `DESIGN.md`: the page's design system. Read it before judging any page work.
9. `docs/LESSONS.md`: engineering and multi-agent collaboration lessons learned.


## 2. Where things stand (2026-11-27)

- **v5 Rust + Tauri v2 (`polyclaved` + `apps/polyclave-app`):** review Rust on `macos-latest` and `windows-latest` (`cargo fmt --all -- --check`, `cargo clippy --locked --workspace --all-targets -- -D warnings`, `cargo test --locked --workspace`), generated `proto.ts` check, UI `tsc --noEmit` / `npm run build` / `npm run test:browser`, and `scripts/privacy-scan.sh` with full Git history.
- **`legacy-node/` retired:** the archived Node runtime and tests were removed in `01c8645` (recoverable from `01c8645^` if ever needed); historical v4/Node notes are no longer the active runtime or gate.
- **Current milestone (owner decision 2026-11-27):** narrowed to one end-to-end user story on real vendor CLIs, real GitHub, and phone approval (see `PLAN-ISSUES.md`, "Narrow the next milestone to one end-to-end user story"). New feature work on mesh dispatch, desktop control/CUA, and self-evolution/community distillation is paused until it passes.
- **Real-environment acceptance still pending:** real `codex`/`claude`/`dsh` CLI probes, live DeepSeek API calls, installed WKWebView/WebView2 walkthrough, real GitHub push/PR/auto-merge, real phone QR pairing + approval, and two-machine mesh run (see the verification table in `README.md`).
- **Git:** local implementers work on feature branches (`fix/*`) or `main` per `NEXT.md` and push `main` only after review and `scripts/privacy-scan.sh` pass. Ignore commit hashes in `HANDOFF.md` before the initial v4 squash ("Branch published").

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
2. **CI:** the `check` workflow run for the exact new head must be green on both native Rust jobs for v5. A red job means the job is not passed. Name the failing job and test.
3. **Tests:** any removed or changed line in an existing test must be one the work list allowed by name.
4. **Code against the rules:**
   - Every value the page interpolates is escaped.
   - Tokens never leave the host (`TurnDto` excludes `mcp_token`; pairing exchange accepts peer identity only from trusted proxy headers, never request body).
   - Nothing is marked `done` without R1 external verification (`classify_run` / `check_run_branch` on the host-only mirror) or explicit user decision.
   - Attempt isolation (per-Attempt worktree + staging repo) replaces whole-session freeze; no unverified Attempt is silently reused or promoted.
   - PR gate publishes only the exact R1-verified `result_sha` to the admission-time GitHub `origin`, and approval evidence separates R1 delivery, per-SHA CI/host tests, and user decision.
   - No unauthorized dependencies and no external requests from the page.
5. **Screenshots:** open every image in `handcheck/<job>/` and compare each claim in the report with what the image shows. Look at the edges of every changed component at 375 px. Implementers have twice described the work list instead of the screen:
   - a phone banner was reported as "under the tabs" while the screenshot showed it above them;
   - a phone overflow was not mentioned at all.

   Where a claim does not match, write that in the verdict.
6. **Privacy:** the report ends with the privacy scan's last line. Skim the diff for local paths, emails and keys anyway, because the repository is public.
7. **When the evidence is not enough to decide**, ask for more in the next message: a named screenshot, a measurement, or a command output to put in `handcheck/<job>/measurements.md`. If your own environment can clone the public repository and run Rust / browser checks, run them there (section 5). Never ask the user to run anything for you.

## 5. Recipes (for implementers, or for your own sandbox if you have one)

- **Workspace & UI checks (v5):**
  - Rust: `cargo fmt --all -- --check`, `cargo clippy --locked --workspace --all-targets -- -D warnings`, `cargo test --locked --workspace`.
  - Protocol sync: `git diff --exit-code -- apps/polyclave-app/ui/src/generated/proto.ts`.
  - UI & browser fixtures: from `apps/polyclave-app/ui`, run `npx tsc --noEmit`, `npm run build`, and `POLYCLAVE_BROWSER_CHANNEL=chrome npm run test:browser` (checks 1440×900 and 375×812, QR roundtrip, and approval card states).
- **Scratch host & hand-check fixtures:** always run under `/tmp/polyclave-hc/<job>/` so screenshots carry no user home path; build/run `polyclaved` with `--state-dir` and `--port`, keep scratch repo paths > 100 chars when testing layout truncation, and stop every started daemon/browser cleanly before reporting.
- **UI critique.** Run design review and browser/detector checks sequentially so they do not contend for the same browser instance.
- **Changing host behaviour.** Before a work list changes host behaviour, run `cargo test --locked --workspace` (and UI checks if touching `proto.ts`/UI) to list every affected test, and explicitly authorize any required test edit in `PLAN-ISSUES.md` / the work list.

## 6. Decisions: yours or the user's

**Direction belongs to the user:** what the product does, changes to the spec, and trade-offs the user would care about. Ask in Chinese, give 2–3 concrete options, and put your recommendation first. Record each answer in `PLAN-ISSUES.md` (and sync into the spec when revising it).

**Details belong to you:** how something is implemented, test changes that keep what the test proves, layout and copy within `DESIGN.md`, and `PLAN-ISSUES.md` items marked `detail`. Decide them and write down why.

**Decisions already made (2026-11-27).** Do not reopen these unless the user does:
- **Attempt isolation replaces whole-session freeze:** `allow_retry` marks the old generation superseded and starts a clean worktree from the immutable base; Hub/manager sessions do not freeze.
- **Host-only mirror & per-Attempt staging:** Workers write only to per-Attempt staging; the host imports, verifies R1 against the immutable baseline, and checks `Turn` release markers (`tmp/turn-<id>.released`) at startup.
- **Mandatory external Claude confinement:** fail closed when no verified external sandbox (macOS Seatbelt) is available, including native Windows.
- **PR publish target from admission `origin`:** snapshot credential-stripped `origin` at Run admission; publish only the exact R1-verified `result_sha` to canonical `https://github.com/<owner/repo>.git`.
- **Per-SHA test results & prompt isolation:** only `ci`/`host_run` matching exact `result_sha` can turn the approval card's tests row green; external SCM events write `source='host'` (`kind="scm_observation"`) and render with `[host observation, not from the user] ` in prompts.
- **Mesh `worker_run` budget A + C:** remote Runs charge the primary Ticket once per `request_id` (no refund on transport error) and receivers enforce a per-node concurrency cap (default 3, configurable 1–16).
- **Narrowed milestone:** complete the single end-to-end user story on real CLIs/GitHub/phone before resuming mesh, CUA, or self-evolution features.
- **Phone first & industrial optical theme:** desktop (1440 px) and phone (375 px) are equally important; use `DESIGN.md`.
- **AI attribution allowed:** `Co-authored-by:` / "Generated with …" is allowed; private paths, secrets, and personal emails remain prohibited by `scripts/privacy-scan.sh`.
- **Hand-check screenshots:** committed to `handcheck/<job>/` (JPEG ≤ 70 quality, < 300 KB), with fixtures under `/tmp`.

## 7. Standing rules for the reviewer

- **You write code on branches only** (`AGENTS.md`, "Dot's branch flow"): one branch `dot/<job>` per job, never `main`, never force-push. Claude reviews and merges it on the user's machine when the user asks. Commits carry the project account (its profile name is fine). AI attribution (`Co-authored-by:` / "Generated with …") is allowed (user decision 2026-11-27, reversing the 2026-11-26 ban); private paths, secrets and personal emails stay banned by `scripts/privacy-scan.sh`.

- **Never ask for anything outside the loop without the user's explicit yes:** a pull request, a merge, a push of another branch, a force-push, rewriting history, or deleting files or branches.
- Never ask for, read, print or repeat an API key, token or password. The user sets keys themselves.
- The repository is public. Any doc you write must contain no local user paths, account names or emails, subscription or plan names, names of other private projects, machine names, or secrets.
- Talk to the user in Chinese. Messages to implementers and repository documents are in English.
- Threat model: the agents on the machine are trusted. The rules guard against mistakes, not attackers.

## 8. Backlog (not yet scheduled)

See open items in `PLAN-ISSUES.md` (mesh 429/400/403 Ticket refund, `dsh` `mcp_url` injection and model selection) and the unverified real-environment capabilities in `README.md`. Paused until the single-story milestone passes: new mesh dispatch features, desktop control/CUA, and self-evolution/community distillation.
