# Page review 2 fixes (after the second UI critique)

**Source:** the second UI critique on 2026-11-26 (26/40, snapshot in `.impeccable/critique/`, not committed). The user chose: the three P1 issues plus the unlock experience, and for unlocking, **the page shows the evidence the user decides on; the rule does not change** (the user still removes the lock with a note; the host does not unlock by itself).

**Applies on top of:** `AGENTS.md`, `DESIGN.md`, the phase 1 plan, and `2026-11-26-page-review-fixes.md`. Keep the look. Page-only work: `public/index.html`, `public/app.js`, `DESIGN.md`. No change to `src/` or `tests/` is needed or allowed. Everything the page needs already comes from the API: `GET /api/sessions` (`queuedCount`, `frozenReason`), `GET /api/runs` (`worktreePath`, `observedSha`), `GET /api/sessions/:id/turns` (`id`, `runId`, `agentPgid`, `agentStartedAt`, `endedAt`, `endReason`).

**Order:** items 1 → 4, one commit each with the message given. `pnpm check` after every commit.

---

## 1. Desktop stream never widens past its column (P1) — `fix: desktop stream keeps to its column`

Measured at 1440 × 900 with a session whose cwd is about 150 characters: `.main` has `scrollWidth` 1336 inside a 764 px column. The user bubbles, the queued bubble and Send slide under the right panel. The cause: `.main` is a grid with no column template, and `.under .mono` is `nowrap`, so the long path sizes the implicit column.

- `.main` gets `grid-template-columns: minmax(0, 1fr)`, its children `min-width: 0`; `.work` uses `284px minmax(0, 1fr) 392px`.
- The cwd under the composer ellipsises inside the column (it already has `title`).
- Hand check at 1440 with a cwd of at least 150 characters: `.main` and `#stream-log` have `scrollWidth === clientWidth`; the user bubble, queued bubble and Send are fully visible left of the right panel.

## 2. Live updates do not break typing (P1) — `fix: live updates keep focus and caret`

Every `session`/`run`/`message`/`turn` SSE event runs `refreshAll()` → `render()`, which rebuilds the side panel with `innerHTML`. A user typing an unlock note or a decision note loses focus and caret (and the phone keyboard closes) whenever any session reports anything.

- Before each render, remember the focused element if it is a `textarea` or `input` inside `#side-body`, `#new-session-box` or the composer: a stable key for it (its `id`, or for the unlock note `data-unlock-form` + session id), `selectionStart`, `selectionEnd` and its `scrollTop`. After the render, if an element with the same key exists, focus it and restore selection and scroll. Give every such field a stable `id` if it lacks one.
- Note text already survives in state; keep that.
- Hand check: start a fake codex turn on one session whose script emits several replies with `sleepMs` between them; while it runs, type in the unlock note of another (locked) session, in a decision note, and in the New task instructions. The caret stays where it was and no character is lost; on the phone emulation the field keeps focus.

## 3. The confirm row keeps the sha the user saw (P1) — `fix: confirm uses the sha shown when the row opened`

`state.pendingDecision` stores only `{ runId, decision }` and `doResolve` sends `run.observedSha` read at confirm time, so a re-render between the first tap and Confirm can change the sha being accepted.

- First tap on Accept stores `{ runId, decision, sha: run.observedSha }`. The confirm label and the request use that stored sha.
- On every render while the row is open: if `run.observedSha` differs from the stored sha, close the row and show, under the status line, "The branch tip moved to <new short sha> while you were confirming. Review it, then accept again." Focus returns to the Accept pill.
- Hand check: open the confirm row, then in the console set the run's `observedSha` in `state.runs` to another value and call `render()`. The row closes with that message and the pill now names the new sha. Confirm on an unchanged row still sends the shown sha (check the request body in the network log).

## 4. Unlock shows evidence and what happens next (unlock experience) — `feat: unlock shows evidence and its effect`

The lock card asks the user to vouch that no agent process is left, but shows nothing to check. Show what the host recorded, and say what removing the lock does.

- For each locked session, load `GET /api/sessions/:id/turns` (only for locked sessions, once per change of `frozenReason`) and take the turn named in the reason, or the session's last turn.
- The lock card gains a small key-value block (DESIGN.md key-value style, Text Tertiary labels):
  - **Agent process:** "process group <agentPgid>, started <agentStartedAt as the host gives it>" or "never started" when `agentPgid` is null.
  - **Turn:** "Turn <id>, ended <endedAt local time> (<endReason in words>)".
  - **Working directory:** the run's `worktreePath` when the turn has a `runId`, else the session's `cwd`; one line, ellipsised, full text in `title`.
  - Under it, one line in Text Tertiary: "To check on the Mac: `ps -g <agentPgid>` shows nothing, and `git -C <path> status` shows no change you did not expect." Omit the `ps` part when there is no pgid.
- Under the note field in the unlock form, the consequence in Text Secondary, built from data: "Removing the lock lets <session> run again." then, when present, "Run <id> will then wait for your decision." (each `needs_review` run of the session) and "<n> queued message(s) will be sent." (`queuedCount`).
- The submit button reads "Remove lock with this note" (was "Sign & remove lock"). Escape inside the form closes it like "Keep locked" and returns focus to "Remove lock…".
- The lock line at the end of a locked session's stream gets a quiet "Remove lock…" button. It opens that session's unlock form in the right panel (on a phone it switches to the Needs you tab) and focuses the note.
- After a successful unlock, show one confirmation line where the lock card was, until the next render of another view: "Lock removed on <session>." plus "Run <id> now waits for your decision." when one does.
- Update DESIGN.md (lock card evidence block, consequence line, button text, stream lock-line button) in this commit.
- Hand check at 1440 and 375 × 812 on a session locked by a `kill -9` of the host during a pushed task: the card shows process group, turn, working directory and the check line; the form shows the consequence lines with the right run id and queued count; Escape closes the form; the stream's "Remove lock…" opens the form (on the phone, on the Needs you tab) with focus in the note; after unlocking, the confirmation line appears.

---

## Hand check and report

Do each item's hand check, screenshots under `.handcheck/page-review-2/`. Report in `HANDOFF.md` in the `AGENTS.md` format; commit it as `chore: page review 2`.
