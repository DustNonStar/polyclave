# Page review 3 (after the third UI critique)

**Source:** the third UI critique on 2026-11-26 (26/40, trend 25 → 26 → 26; snapshot in `.impeccable/critique/`, not committed). The user chose: the two P1 and three P2 issues, and **the page shows what a commit contains (subject and changed files) before the user accepts it**. That last item needs a small host change.

**Applies on top of:** `AGENTS.md`, `DESIGN.md`, the phase 1 plan and the two earlier page work lists. Keep the look.

**Reviewer spike:** adding a `commit` field to `GET /api/runs/:id` was tried in a separate worktree; `pnpm check` stayed at 130 passed, 1 skipped. No existing test needs a change for items 1–7, and none may be changed except the one wait item 0 allows. Only the two new tests named in item 1 are added.

**Order:** items 0 → 7, one commit each with the message given. `pnpm check` after every commit.

---

## 0. CI: one milestone test races on a slower machine — `test: wait for the thread id before the token snapshot`

The first macOS CI run for this review failed `tests/milestone.spec.ts` › `second host refused while the first lives; old token refused after restart` at line ~793: the snapshot of `runs` was taken after the fake codex reached its barrier but before the host had stored the run's `agent_session_id` (expected `null`, received `"fake-thread-…"`, and a later `updated_at`). The host is right; the test reads too early.

Allowed change, and the only change to an existing test in this job: in that test, between `await bHold.waitReached()` and the snapshot, wait (with the existing `waitFor`-style polling of the database, not a fixed sleep) until the run row's `agent_session_id` is not null. Nothing else in the test changes. Check: `pnpm test tests/milestone.spec.ts` passes five times in a row locally, and CI is green after the push.

## 1. Host: run detail carries a commit summary — `feat: run detail includes a commit summary`

- New export in `src/git-effect.ts`: `commitSummary(remote: string, sha: string): Promise<CommitSummary | null>` with `interface CommitSummary { sha: string; subject: string; files: { path: string; added: number | null; deleted: number | null }[]; filesTotal: number }`. Use the module's async `git()` helper: `git show --numstat --format=%s <sha>` on the remote. The first line is the subject; each numstat line gives added, deleted and path (`-` for a binary file becomes `null`). Keep at most the first 50 files in `files`; `filesTotal` is the full count. Return `null` when `sha` does not match `/^[0-9a-f]{7,40}$/` or git fails (for example, the object is not on the remote).
- `GET /api/runs/:id` adds `commit: await commitSummary(run.remote, run.observedSha ?? run.resultSha)`, or `null` when both are null. No other route changes.
- Tests, added to `tests/http.spec.ts`:
  - `it('run detail includes the commit subject and changed files', …)`: a run reaches `needs_review` after the fake codex commits `f.txt` and pushes. `commit.sha` equals `run.observedSha`, `commit.subject` equals the subject of that commit read with `git log -1 --format=%s <sha>` on the remote, `commit.files` contains `{ path: 'f.txt', added: 1, deleted: 0 }`, and `commit.filesTotal` is 1.
  - `it('run detail commit is null when there is no sha', …)`: a run with neither `observedSha` nor `resultSha` gets `commit: null`.

## 2. Page: what you accept is shown — `feat: show the commit before accepting it`

- `loadRunDetail` keeps `commit` from the response. In the Host question block of a `needs_review` run, directly above the note field and pills: a "What changed" block with the commit subject (silver 500), then up to 5 files as `path +added −deleted` in mono 12.5px (binary files read "binary"), then "and N more files" when `filesTotal` is larger. Paths ellipsise in the middle, with the full path in `title`.
- If `commit` is null, one line instead: "The host could not read this commit from the remote."
- The same block shows in the detail of a `done` run, under the status line, titled "Accepted commit".
- Update DESIGN.md (Host question: "What changed" block).

## 3. Failed send or stop is never silent (P1) — `fix: failed send or stop keeps the text and says why`

Today the composer clears the input before `POST /api/sessions/:id/messages`, and `catch {}` swallows a failure. Stop turn swallows failures the same way.

- Clear the input only after the POST succeeds. On failure, leave the text in place and show one line under the composer in Orange: "Not sent: <error message>. Your message is still here." The line goes away on the next successful send or when the user edits the text.
- Stop turn failure: the same line style, "Could not stop the turn: <error message>."
- The send button is disabled while a send is in flight, so a double press does not send twice.
- Hand check: stop the host (`kill -9`), type a message, press Send: the text stays and the line shows. Restart the host and press Send: it goes, the input clears and the line goes away.

## 4. The stream follows the run you open (P1) — `fix: opening a run selects its session`

Opening a run (from Needs you, Recent runs, a sidebar run row, or a task chip in a stream) leaves the stream, tab bar and composer on another session. A user can then message the wrong session.

- Opening a run also selects its session: the stream, the session tab, the sidebar highlight, the composer placeholder and, on a phone, the phone header. On a phone the visible tab stays where it is (opening from Needs you keeps you on Needs you).
- This is the reverse of the rule from page review item 6 (selecting a session returns the panel to the list when it showed another session's run); both now hold, so panel and stream always show the same session. Update DESIGN.md in this commit.
- Hand check at 1440 and 375: open `web`'s run from Needs you while `api-server` is selected; the stream, tab and composer now say `web`.

## 5. The question reads right after unlock (P2) — `fix: review question after unlock`

After the lock is removed the Host question still reads "an agent process may still be writing" directly above Accept, which contradicts the unlock the user just signed.

- When the run's session is not locked, the question line reads "Review <sha7>: accept it, mark it failed, or allow a retry." (or, with no observed sha, "Review this run: mark it failed or allow a retry."). The host's `detail` moves to a smaller line under it, "Host note: <detail with a capital first letter>."
- While the session is locked, keep today's lock notice.

## 6. Confirm cannot be reached by a double tap (P2) — `fix: confirm sits away from the first tap`

- In the confirm row, Back takes the place of the pill that was tapped (left), and the confirm pill sits to its right. The confirm pill ignores clicks for 400 ms after it appears (show it at reduced opacity during that time; no layout shift).
- The fine print ("Accept applies to <sha7> only …") stays visible in the confirm state.
- Hand check: a fast double tap on Accept opens the confirm row and does not resolve the run (check the network log: no `resolve` request).

## 7. The phone lock card is short (P2) — `fix: shorter lock card on a phone`

- Paths in the evidence block and the check line are shortened in the middle to the last two segments ("…/worktrees/rc7ef48c3"), with the full path in `title`.
- The check line becomes a `details` element, "How to check on the Mac", closed by default under 860 px and open on desktop. Inside it, the two commands each sit in their own `code` line with the full path.
- Under the evidence, one Text Tertiary line on every width: "Can't check from here? Keep it locked; nothing runs until you remove the lock."
- Hand check at 375 × 812: with the form closed, "Remove lock…" is visible without scrolling on the Needs you tab when one session is locked.

---

## Hand check and report

Each item's hand check at 1440 and 375 × 812 where it applies, plus the overflow check from `AGENTS.md` for every changed component. Fixtures under `/tmp/polyclave-hc/page-review-3/`; screenshots and `measurements.md` under `handcheck/page-review-3/` (committed, see `AGENTS.md` Hand checks). Report in `HANDOFF.md` in the `AGENTS.md` format; commit it as `chore: page review 3`.
