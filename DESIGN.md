- **v5 Desktop & Mobile UI (`apps/polyclave-app/ui/`):** Deep Obsidian / Daylight Crystal themes, 3-column workspace (1440×900) and responsive mobile tabs (375×812), zero `backdrop-filter` GPU thrash, zero external requests.
- **Stage 2 additions:**
  - **Global Pause (`#pause-toggle`):** Prominent top-header control (`Pause` / `Paused · Resume`) backed by `/api/pause` and `/api/resume`, mirrored in the bottom status bar.
  - **Review sub-navigation (`Runs | Council | PR Gates`):**
    - **Blind Council:** Anonymous proposal cards (`方案甲` / `方案乙` / `方案丙`) with risk & escalation pills (`touched_paths`, `security_sensitive`, `public_interface_changes`, `new_dependencies`, `open_questions`), decision selector for `direction` / escalated councils, and post-decision vendor/model reveal.
    - **PR Gates & SCM Observer:** Repo/branch/SHA chips, `auto_merge_armed` state, `scope_violations` callout, heterogeneous reviewer verdicts, and Pause-aware `Run PR Gate` control.
  - **Adapters & Credentials:** `Antigravity (agy)` and `DeepSeek Harness (dsh)` added to Worker dispatch; Settings documents distinct OS Keychain `Polyclave.GitHub` (`bot_token` / `reviewer_token`) bindings.
- **Stage 3 additions:**
  - **Mobile Entrypoint (`apps/polyclave-app/src-tauri/src/lib.rs`):** Target-gated `mobile_entry_point` for iOS / Android alongside desktop `main.rs`.
  - **Devices & 10-min QR Pairing:** Standard QR code (ISO/IEC 18004, byte mode, error correction M, smallest fitting version; the daemon's 69-byte URI is version 5) drawn as inline SVG by `views/qr.ts` with the bundled `qrcode-generator` encoder (no external requests). The symbol is black on a white 112 × 112 px tile with `--r-sm` corners; its 4-module quiet zone is part of the SVG viewBox, so the tile has no extra padding. Paired device list with instant `Revoke`, and dual-identity verification over Tailscale / AgentMesh.
  - **AgentMesh Nodes & Private Notifications:** Deny-by-default mesh node visibility/revocation and private self-hosted `ntfy` / UnifiedPush wakeup configuration in Settings.
- **Stage 4 additions:**
  - **CUA Desktop Mutex & Reclaim:** Statusbar lock indicator + 1-click `Reclaim desktop` control in Settings.
  - **Local Evolution, Teach-Once Skills & Opt-in Community Drafts:** Distill local suggestions from SQLite, save/rollback versioned skills synced to isolated vendor homes only, and preview redacted community drafts strictly opt-in.

---
name: Polyclave
description: Host page for coding agents from several vendors; black and silver, soft, with fluorescent colour reserved for work in motion.
colors:
  ground: "#000000"
  rail: "#09090a"
  raise: "#111113"
  raise-hi: "#18181b"
  raise-top: "#202024"
  line: "#1c1c1f"
  line-hi: "#2a2a2e"
  text: "#e8e8eb"
  text-2: "#a7a7ae"
  text-3: "#808087"
  silver: "#eceef2"
  silver-2: "#c3c6cc"
  silver-edge: "rgba(226, 230, 236, 0.46)"
  silver-metal-hi: "#f6f7f9"
  silver-metal-mid: "#c9ccd2"
  silver-metal-lo: "#8d9199"
  metal-ink: "#15161a"
  selection: "rgba(195, 198, 204, 0.28)"
  neon-green: "#2dff5c"
  neon-yellow: "#ffe414"
  neon-orange: "#ff6a00"
  green: "#3fb950"
  green-btn: "#238636"
  green-btn-hi: "#1f7a32"
  green-btn-ink: "#f2fff4"
  yellow: "#d4a72c"
  queued-edge: "rgba(212, 167, 44, 0.4)"
  orange: "#e8772e"
  orange-hi: "#f08a45"
  orange-ink: "#1c0a00"
  lock-tint: "rgba(232, 119, 46, 0.06)"
  lock-edge: "rgba(232, 119, 46, 0.45)"
typography:
  title:
    fontFamily: "-apple-system, BlinkMacSystemFont, \"SF Pro Text\", \"Helvetica Neue\", Arial, sans-serif"
    fontSize: "17px"
    fontWeight: 600
    lineHeight: 1.35
    letterSpacing: "-0.01em"
  brand:
    fontFamily: "-apple-system, BlinkMacSystemFont, \"SF Pro Text\", \"Helvetica Neue\", Arial, sans-serif"
    fontSize: "14.5px"
    fontWeight: 650
    lineHeight: 1.6
    letterSpacing: "-0.01em"
  body:
    fontFamily: "-apple-system, BlinkMacSystemFont, \"SF Pro Text\", \"Helvetica Neue\", Arial, sans-serif"
    fontSize: "14.5px"
    fontWeight: 400
    lineHeight: 1.6
    letterSpacing: "normal"
  answer:
    fontFamily: "-apple-system, BlinkMacSystemFont, \"SF Pro Text\", \"Helvetica Neue\", Arial, sans-serif"
    fontSize: "14px"
    fontWeight: 400
    lineHeight: 1.6
  label:
    fontFamily: "-apple-system, BlinkMacSystemFont, \"SF Pro Text\", \"Helvetica Neue\", Arial, sans-serif"
    fontSize: "13.5px"
    fontWeight: 400
    lineHeight: 1.6
  meta:
    fontFamily: "-apple-system, BlinkMacSystemFont, \"SF Pro Text\", \"Helvetica Neue\", Arial, sans-serif"
    fontSize: "13px"
    fontWeight: 400
    lineHeight: 1.6
  caption:
    fontFamily: "-apple-system, BlinkMacSystemFont, \"SF Pro Text\", \"Helvetica Neue\", Arial, sans-serif"
    fontSize: "12.5px"
    fontWeight: 400
    lineHeight: 1.6
  badge:
    fontFamily: "-apple-system, BlinkMacSystemFont, \"SF Pro Text\", \"Helvetica Neue\", Arial, sans-serif"
    fontSize: "11px"
    fontWeight: 700
    lineHeight: 1
  mono:
    fontFamily: "ui-monospace, \"SF Mono\", Menlo, monospace"
    fontSize: "12.5px"
    fontWeight: 400
    fontFeature: "\"tnum\""
  mono-time:
    fontFamily: "ui-monospace, \"SF Mono\", Menlo, monospace"
    fontSize: "11.5px"
    fontWeight: 400
    fontFeature: "\"tnum\""
  mono-title-id:
    fontFamily: "ui-monospace, \"SF Mono\", Menlo, monospace"
    fontSize: "15px"
    fontWeight: 500
    fontFeature: "\"tnum\""
rounded:
  code: "6px"
  avatar-sm: "7px"
  control: "8px"
  tab: "9px"
  row: "10px"
  card-sm: "12px"
  card: "16px"
  composer: "18px"
  pill: "999px"
  round: "50%"
spacing:
  hair: "2px"
  xs: "4px"
  sm: "6px"
  md: "8px"
  row: "10px"
  lg: "12px"
  xl: "14px"
  card: "16px"
  panel: "18px"
  stream-top: "22px"
  gutter: "32px"
components:
  button:
    backgroundColor: "{colors.raise-top}"
    textColor: "{colors.silver}"
    typography: "{typography.label}"
    rounded: "{rounded.row}"
    padding: "0 14px"
    height: "34px"
  button-warn:
    backgroundColor: "{colors.orange}"
    textColor: "{colors.orange-ink}"
    typography: "{typography.label}"
    rounded: "{rounded.row}"
    padding: "0 14px"
    height: "34px"
  button-warn-hover:
    backgroundColor: "{colors.orange-hi}"
    textColor: "{colors.orange-ink}"
  button-quiet:
    textColor: "{colors.text-2}"
    typography: "{typography.label}"
    rounded: "{rounded.row}"
    padding: "0 14px"
    height: "34px"
  button-quiet-hover:
    backgroundColor: "{colors.raise-hi}"
    textColor: "{colors.silver}"
  icon-button:
    textColor: "{colors.text-3}"
    rounded: "{rounded.control}"
    size: "28px"
  icon-button-hover:
    backgroundColor: "{colors.raise-hi}"
    textColor: "{colors.silver}"
  pill-answer:
    backgroundColor: "{colors.raise}"
    textColor: "{colors.silver}"
    typography: "{typography.answer}"
    rounded: "{rounded.pill}"
    padding: "0 16px"
    height: "38px"
  pill-answer-hover:
    backgroundColor: "{colors.raise-hi}"
  pill-answer-accept:
    backgroundColor: "{colors.green-btn}"
    textColor: "{colors.green-btn-ink}"
    typography: "{typography.answer}"
    rounded: "{rounded.pill}"
    padding: "0 16px"
    height: "38px"
  pill-answer-disabled:
    textColor: "{colors.text-3}"
  send-button:
    backgroundColor: "{colors.silver-metal-mid}"
    textColor: "{colors.metal-ink}"
    rounded: "{rounded.round}"
    size: "32px"
  logo-mark:
    backgroundColor: "{colors.silver-metal-mid}"
    textColor: "{colors.metal-ink}"
    rounded: "{rounded.control}"
    size: "24px"
  search-field:
    backgroundColor: "{colors.raise}"
    textColor: "{colors.text}"
    rounded: "{rounded.row}"
    padding: "0 10px"
    height: "36px"
  nav-item:
    textColor: "{colors.text-2}"
    rounded: "{rounded.row}"
    padding: "0 10px"
    height: "36px"
  nav-item-hover:
    backgroundColor: "{colors.raise}"
    textColor: "{colors.silver}"
  count-badge:
    backgroundColor: "{colors.orange}"
    textColor: "{colors.orange-ink}"
    typography: "{typography.badge}"
    rounded: "{rounded.tab}"
    padding: "0 5px"
    height: "18px"
  avatar:
    backgroundColor: "{colors.raise-top}"
    textColor: "{colors.silver}"
    rounded: "{rounded.row}"
    size: "32px"
  avatar-sm:
    backgroundColor: "{colors.raise-top}"
    textColor: "{colors.silver}"
    rounded: "{rounded.avatar-sm}"
    size: "22px"
  session-card:
    textColor: "{colors.silver}"
    rounded: "{rounded.card-sm}"
    padding: "9px 10px"
  session-card-hover:
    backgroundColor: "{colors.raise}"
  session-card-selected:
    backgroundColor: "{colors.raise-hi}"
  run-row:
    textColor: "{colors.text-2}"
    typography: "{typography.label}"
    rounded: "{rounded.row}"
    padding: "6px 10px 6px 30px"
  session-tab:
    textColor: "{colors.text-3}"
    typography: "{typography.label}"
    padding: "0 14px"
    height: "44px"
  session-tab-selected:
    textColor: "{colors.silver}"
  panel-tab:
    textColor: "{colors.text-3}"
    typography: "{typography.label}"
    rounded: "{rounded.tab}"
    padding: "0 12px"
    height: "32px"
  panel-tab-selected:
    backgroundColor: "{colors.raise-hi}"
    textColor: "{colors.silver}"
  chip:
    backgroundColor: "{colors.raise-hi}"
    textColor: "{colors.silver-2}"
    typography: "{typography.label}"
    rounded: "{rounded.control}"
    padding: "1px 9px 1px 7px"
  inline-code:
    backgroundColor: "{colors.raise-hi}"
    textColor: "{colors.silver}"
    typography: "{typography.mono}"
    rounded: "{rounded.code}"
    padding: "1px 6px"
  user-bubble:
    backgroundColor: "{colors.raise-hi}"
    textColor: "{colors.text}"
    typography: "{typography.body}"
    padding: "10px 15px"
  queued-bubble:
    textColor: "{colors.text-2}"
    typography: "{typography.body}"
    padding: "10px 15px"
  worked-steps:
    backgroundColor: "{colors.rail}"
    rounded: "{rounded.card-sm}"
    padding: "12px 14px"
  composer:
    backgroundColor: "{colors.raise}"
    textColor: "{colors.text}"
    typography: "{typography.body}"
    rounded: "{rounded.composer}"
    padding: "14px 12px 10px 16px"
  model-selector:
    textColor: "{colors.text-2}"
    typography: "{typography.meta}"
    rounded: "{rounded.tab}"
    padding: "0 10px"
    height: "30px"
  lock-card:
    backgroundColor: "{colors.lock-tint}"
    textColor: "{colors.text-2}"
    rounded: "{rounded.card}"
    padding: "14px 16px 16px"
  unlock-field:
    backgroundColor: "{colors.ground}"
    textColor: "{colors.text}"
    typography: "{typography.answer}"
    rounded: "{rounded.card-sm}"
    padding: "10px 12px"
  host-question:
    backgroundColor: "{colors.raise}"
    textColor: "{colors.silver}"
    rounded: "{rounded.card}"
    padding: "16px"
  status-bar:
    backgroundColor: "{colors.rail}"
    textColor: "{colors.text-3}"
    typography: "{typography.caption}"
    padding: "0 14px"
    height: "30px"
  phone-tab:
    textColor: "{colors.text-2}"
    typography: "{typography.label}"
    rounded: "{rounded.row}"
    height: "36px"
  phone-tab-selected:
    backgroundColor: "{colors.raise-hi}"
    textColor: "{colors.silver}"
---

# Design System: Polyclave

## Overview

**Creative North Star: "The Lit Switchboard"**

A pure black room where everything at rest is grey and silver, and the only light comes from work that is happening right now. Agents spin, wait and call for the user in fluorescent green, yellow and orange; the moment something stops moving it drops to an ordinary green, yellow or orange. The page reads calm at a glance and tells the user, by where the light is, what is alive and what is waiting on them.

The material is soft rather than industrial-hard: generous radii on every row, card and field (10 to 18px), pill-shaped answers, sentence-case system type at a 14.5px base, edges drawn as hairline inset strokes instead of shadows. Silver is the only "metal": bright silver marks names, titles and selection, and a brushed-silver gradient appears on exactly two objects, the logo mark and the send button.

The signature is the motion grammar of the status glyphs. A spinning arc, a slow dashed ring, a pinging dot, a check that draws itself only when a step has just finished, a pause ring and a lock. New items rise into the stream, a lock card settles into place like a tag hung on a hook. Every element is visible without animation, and reduced motion switches all of it off.

**Key Characteristics:**
- Pure black ground, near-black rails, three soft grey raise steps; no drop shadows anywhere.
- Silver for names, titles and selection; brushed-silver gradient only on the logo mark and send button.
- Fluorescent colour only on glyphs of work in motion; normal green, yellow and orange for finished and static states.
- Soft radii: 10px rows, 12px session cards, 16px cards, 18px composer, pill answers.
- Sentence-case system UI type; monospace only for shas, ids, paths and times.
- An animated 14px status-glyph set and a short motion grammar (rise, settle, draw) as the signature.

## Colors

A black-and-silver neutral world with two parallel sets of state colour: fluorescent for motion, normal for rest.

### Primary
- **Bright Silver** (`silver`): session names, the detail title, emphasised words in agent text, selected tab text, button labels, the composer caret. The brightest ink on the page and the mark of "this is a name or a choice".
- **Soft Silver** (`silver-2`): the active session-tab underline (2px), chip text, the global focus outline (2px, 2px offset), hover text on quiet controls.
- **Silver Edge** (`silver-edge`): the inset 1px outline of the selected session card, avatar outlines, the selected phone tab, hovered answer pills, and the focused composer.
- **Brushed Silver** (`silver-metal-hi` to `silver-metal-mid` to `silver-metal-lo`, as `linear-gradient(150deg, hi 0%, mid 45%, lo 100%)`): the logo mark tile and the round send button only, with **Metal Ink** (`metal-ink`) for the glyph on top. The host's identity in a host question is the logo mark on this tile.

### Secondary
- **Neon Green** (`neon-green`): the running arc, wherever a turn, run or step is executing.
- **Neon Yellow** (`neon-yellow`): the waiting ring, for queued messages and queued sessions.
- **Neon Orange** (`neon-orange`): the needs-you ping, on anything that is blocked on the user right now.

### Tertiary
- **Verified Green** (`green`): the finished check glyph. **Accept Green** (`green-btn`, hover `green-btn-hi`, ink `green-btn-ink`): the one primary/accept answer pill.
- **Interrupted Yellow** (`yellow`): the pause-ring glyph and the "Waiting · sent" line of a queued message; `queued-edge` outlines the queued bubble.
- **Lock Orange** (`orange`, hover `orange-hi`, ink `orange-ink`): the lock glyph, the lock card heading, the "Remove lock" and "Sign & remove lock" buttons, and every count badge. `lock-tint` fills the lock card and `lock-edge` outlines it and its note field.

### Neutral
- **Ground** (`ground`): the page itself and the stream column; also the unlock note field, sunk below the lock card.
- **Rail Black** (`rail`): sidebar, right panel, status bar, phone tab strip, the expanded step list, and the cut-out behind an avatar's status dot.
- **Raise** (`raise`), **Raise High** (`raise-hi`), **Raise Top** (`raise-top`): the three soft grey steps. Raise is hover and resting fields (search, composer, host question, answer pills); Raise High is selected or contained content (selected session, selected panel tab, chips, inline code, the user bubble); Raise Top is the solid button and avatar tiles.
- **Line** (`line`) and **Line High** (`line-hi`): 1px column dividers and section rules; Line High for inset control edges and the running arc's track.
- **Text** (`text`), **Text Secondary** (`text-2`), **Text Tertiary** (`text-3`): body copy; secondary rows, activity lines and quiet buttons; meta, times, placeholders, section labels, idle icons.
- **Selection** (`selection`): text selection highlight.

### Named Rules
**The Motion-Only Neon Rule.** Fluorescent colour lives only on a glyph whose state is in progress (running, waiting, needs you). When the state ends, the glyph is replaced by its normal-colour counterpart. No text, fill, border or button is ever neon.

**The Two Metal Objects Rule.** The brushed-silver gradient appears on the logo mark (including where it stands for the host) and the send button. Nothing else is metal.

**The Orange Means Locked Rule.** Static orange belongs to locks and to the counts of sessions that need the user. Composer send/stop failures may use standard orange text to name an action that needs attention. Orange is never decoration, and warn buttons are the only filled orange controls.

## Typography

**Display Font:** none; the page has no display tier.
**Body Font:** system UI (`-apple-system, BlinkMacSystemFont, "SF Pro Text", "Helvetica Neue", Arial, sans-serif`)
**Label/Mono Font:** system monospace (`ui-monospace, "SF Mono", Menlo, monospace`) with tabular numerals

**Character:** The operating system's own voice, set slightly large (14.5px) and loose (1.6) so a dense tool still reads like plain prose. Weight, not size, carries hierarchy: 600 for names and titles, 500 for controls, 400 for everything else.

### Hierarchy
- **Title** (600, 17px, 1.35, -0.01em, silver): the detail-view heading. Its run id leads in Mono Title Id (500, 15px, text-3).
- **Brand** (650, 14.5px, -0.01em, silver): the "Polyclave" wordmark beside the logo mark.
- **Body** (400, 14.5px, 1.6): agent text, user and queued bubbles, the composer. Stream items cap at 74ch.
- **Answer** (400, 14px): answer pills and the unlock note field.
- **Label** (400 or 500, 13.5px): tabs, panel tabs, run rows, steps, chips, key-values, timeline, breadcrumb, buttons (500), section heads like "Activity" (500, text-3).
- **Meta** (13px): session meta lines, the model selector, the lock card heading (600, orange).
- **Caption** (12.5px): sidebar section labels (500), status bar, under-composer line, fine print, the queued "Waiting" line.
- **Badge** (700, 11px): count badges.
- **Mono** (12.5px): inline code, commands and paths in steps. **Mono Time** (11.5px, text-3): times on run rows, timeline, lock card and host question; ids inside chips.

### Named Rules
**The Mono Is Data Rule.** Monospace is used for shas, ids, paths, commands, inline code and times — every sha and id, wherever it appears (chips, buttons, timeline, key-values, system lines) — and for nothing a person would read as words, including avatar initials and status words.

**The Sentence Case Rule.** Every label, tab, button and heading is sentence case. No uppercase labels, no tracking-out, no small caps.

## Layout

Desktop is a fixed three-column grid over a bottom status bar: sidebar 284px, stream `1fr`, right panel 392px, status bar 30px. Columns scroll on their own; the page does not. The stream column stacks a 44px session tab bar, the scrolling log (padding 22px 32px 28px, 16px between items, items capped at 74ch), and the composer (padding 10px 32px). The sidebar pads 10px with 2px between rows; the detail view pads 14px 18px 22px with 16px between blocks.

User bubbles and queued messages align right (`justify-self: end`); system lines, agent text, "Worked for" blocks and activity lines align left.

**Phone (max-width 860px):** a three-tab strip on Rail Black at the top (Sessions, Stream, Needs you with its count; 44px buttons, 4px gap, padding 8px 10px) replaces the side columns. One column shows at a time; the session tab bar, status bar and under-composer line hide; stream and composer gutters drop from 32px to 16px.

**Phone stream header:** the Stream area starts with one 44px line on Ground with a Line bottom rule: a 22px avatar, the session name in silver 600 (13.5px, ellipsised with its full text in `title`) and, at the right, the session's state glyph with its word — Neon Green arc "running", Neon Yellow ring "waiting", orange lock "locked" — and nothing when the session is idle. The line scrolls with neither the log nor the composer.

**Phone connection banner:** while the host is unreachable, a 36px Rail Black strip sits directly under the phone tab strip on every tab: the orange needs-you dot and "Host unreachable · reconnecting…" in Lock Orange 12.5px, with a Line bottom rule. It disappears the moment the stream reconnects, and the desktop status bar keeps its own copy of the same text.

**Phone first tab:** a phone load opens on "Needs you" when at least one session needs the user (a locked session, a `needs_review` run or an `interrupted` run), and on "Stream" otherwise. A deliberate tab tap is never overridden afterwards.

**Phone touch targets:** every control that is tapped at ≤860px is at least 44px tall, reached by padding or height and never by a larger font: the phone tabs, the send button (44px circle), answer pills, "Remove lock…", the unlock form buttons, the "Worked for" summary and the clickable run chip in the stream.

**Nothing is cut off:** every grid container that holds text (`.log`, `.detail`, the step list, the timeline, key-values) is a `minmax(0, 1fr)` single column whose children are `min-width: 0`, so a long command, branch name or path ellipsises inside its own row instead of widening the column; the full text sits in the element's `title`. A container may only report `scrollWidth > clientWidth` when its overflow is intentionally hidden.

Spacing steps observed: 2, 4, 6, 8, 10, 12, 14, 16, 18, 22, 32px. Icon-to-label gaps are 6 to 10px.

## Elevation & Depth

Flat. There are no drop shadows. Depth is tonal: black ground, near-black rails, then three grey raise steps, with 1px edges drawn as `box-shadow: inset 0 0 0 1px <colour>` so edges never change an element's size. The only "lift" is the brushed-silver gradient on the two metal objects.

### Shadow Vocabulary
- **Control edge** (`inset 0 0 0 1px` Line High): solid buttons, answer pills, composer, host question.
- **Selected edge** (`inset 0 0 0 1px` Silver Edge): selected session card, avatars, selected phone tab, hovered pill, focused composer.
- **Contained edge** (`inset 0 0 0 1px` Line): the expanded step list.
- **Lock edge** (`inset 0 0 0 1px` Lock Edge, full Orange on focus): lock card and its note field.
- **Queued edge** (`inset 0 0 0 1px` Queued Edge): the queued message bubble.

### Named Rules
**The Inset Edge Rule.** Edges are inset 1px strokes or tonal steps. If something looks like it needs a shadow, give it the next raise step instead.

## Shapes

Soft everywhere, with a clear size ladder: 6px inline code, 7px small avatar, 8px icon buttons, chips, logo tile and the focus outline, 9px panel tabs and the model selector, 10px rows (nav, search, run rows, buttons, avatars, phone tabs), 12px session cards, step list and note field, 16px lock card and host question, 18px composer, full pills for answers and count badges, circles for the send button and status dots. Chat bubbles use `16px 16px 6px 16px`, tucking the corner nearest the composer.

Glyphs are drawn on a 14px grid with round caps and joins; interface icons on a 16px grid at 1.6 stroke. Avatars are rounded squares, never circles.

## Components

### Buttons
- **Shape:** gently rounded (10px), 34px tall, 0 14px padding, 13.5px weight 500, icon gap 8px.
- **Solid:** Raise Top with the control edge, silver label; hover lightens one step.
- **Warn:** filled Lock Orange with Orange Ink, no edge; hover Orange High. Used only to remove a lock.
- **Quiet:** transparent, Text Secondary; hover Raise High with silver text. Used for "Keep locked".
- **Icon button:** 28px square, 8px radius, Text Tertiary 16px icon; hover Raise High and silver.
- **Send:** 32px circle in brushed silver with a 2px-stroke up arrow in Metal Ink.
- **Hover / Focus:** background and colour transition 0.15s; focus shows a 2px Soft Silver outline at 2px offset.

### Answer Pills
- **Shape:** full pill, 38px tall, 0 16px, 14px type, gap 8px, wrapping row with 8px gaps, indented 24px under the question.
- **Default:** Raise with control edge, silver label; hover Raise High with the selected edge.
- **Accept:** Accept Green with its light ink, no edge. Its label names the exact sha it accepts ("Accept 3e1a9f0"), and the fine print under the row repeats that the decision applies to that sha only.
- **Disabled:** transparent, no edge, Text Tertiary, `not-allowed` cursor, and the reason is always written under the row in Text Secondary 12.5px ("Allow retry is disabled: the branch is on the remote."), never only in a `title`.
- **Confirm row:** the first tap on Accept, Mark failed or Allow retry does not send anything, and it stores the sha the row showed. The pill row is replaced in place by one confirmation pill — "Confirm accept 3e1a9f0", "Confirm fail" or "Confirm retry" (the accept one keeps the accept-green fill and the exact sha) — beside a quiet "Back" pill. Only the confirmation pill sends, and it sends the sha it named; if the branch tip moves while the row is open, the row closes and one line under the status row says "The branch tip moved to <new sha> while you were confirming. Review it, then accept again.", with focus back on the Accept pill, which now names the new sha. "Back" or Escape restores the row and puts focus back on the pill that opened it. The note field stays visible above the row under a visible "Note" label; a note is required for Mark failed and Allow retry (an inline error appears if it is empty) and optional for Accept, and an Accept with no note stores the decision and the sha it saw without repeating the word.
- **Decision line:** for a run just decided, one line under its status row — a verified check plus "Accepted 3e1a9f0", "Marked failed" or "Retry allowed" — stays until another run is rendered.

### Chips
- **Style:** Raise High, Soft Silver text, 8px radius, `1px 9px 1px 7px`, 24px line, 13.5px, 6px gap; an optional leading 14px icon or status glyph, and an optional mono id in Text Tertiary at 11.5px.
- **Use:** branch, tip and base references in the detail view; linked runs in activity lines.

### Cards / Containers
- **Lock card:** 16px radius, Lock Tint fill, Lock Edge stroke, padding 14px 16px 16px, 10px gap. Heading row: lock glyph, "<session> locked" (13px 600 Orange) so the card names its session wherever it appears, time at right. Body text Text Secondary 13.5px. Then the **evidence block**: a key-value list in the detail style — 12.5px Text Tertiary labels over Text Secondary values, a 112px label column — with *Agent process* ("process group <pgid>, started <hh:mm:ss>", the host's UTC `ps lstart` string read in the page's local time — date added only when the start was not today, raw string if it does not parse — or "never started"), *Turn* ("Turn <id>, ended <time> (<reason in words>)") and *Working directory* (the run's worktree, else the session's cwd; one line, ellipsised, full text in `title`). Under it one Text Tertiary 12px line: "To check on the Mac: `ps -g <pgid>` shows nothing, and `git -C <path> status` shows no change you did not expect." — the `ps` clause is omitted when the host recorded no process group. A warn "Remove lock…" button expands **in place** into a form: a note field (min 60px, 12px radius, Ground fill, Lock Edge stroke, full Orange on focus, 14px type), then the **consequence** lines in Text Secondary 12.5px built from data — "Removing the lock lets <session> run again.", one "Run <id> will then wait for your decision." per `needs_review` run, and "<n> queued message(s) will be sent." when the queue is not empty — and a right-aligned row of "Keep locked" (quiet) and "Remove lock with this note" (warn). The form rises in (0.35s); the trigger button hides while it is open; Escape closes the form exactly like "Keep locked" and returns focus to "Remove lock…". The card is one `minmax(0, 1fr)` column, so nothing inside it can widen it: the heading wraps its time rather than clipping it, and the button row wraps — on a phone "Remove lock with this note" takes the full row under a right-aligned "Keep locked", both 44px tall.
- **Lock removed:** after a successful unlock the card is replaced, where it stood, by one line in a Raise 12px card: the verified check and "Lock removed on <session>." plus "Run <id> now waits for your decision." when a run does. It stays until another view renders.
- **Needs-you tag:** one 16px card per session that needs the user, Raise fill with the Line High edge, 12px padding, 8px gap. Its head is a 22px avatar, the session name in silver 600 13.5px with its full text in `title`, the word "locked" appended when the session is locked, and the lock glyph pushed right. Inside, in the order they must be handled: the lock card (reason, turn, time, "Remove lock…"), then that session's `needs_review` run cards, then its `interrupted` run cards. A session with nothing pending has no tag; the recent runs list follows all tags.
- **Host question:** 16px radius, Raise fill, control edge, 16px padding, 12px gap. Header: host avatar (the logo mark on a 22px brushed-silver tile), "Host" in silver 600, time at right. Question line silver 500 led by the needs-you ping. On a `needs_review` run, a **What changed** block (Rail Black with contained edge, 12px radius; also shown under the status line of a `done` run on Raise with control edge as **Accepted commit**) sits directly above the note field and pills: the commit subject in silver 500, up to 5 changed files as `path +added −deleted` in mono 12.5px ("binary" when binary) with middle-ellipsised paths and full paths in `title`, and "and N more files" when `filesTotal` is larger (or "The host could not read this commit from the remote." when `commit` is null). Then answer pills and fine print (12.5px Text Tertiary), both indented 24px. While the run's session is locked the pills, the note field and the fine print are replaced by one line in Text Secondary 13.5px indented 24px — "Remove the lock on **<session>** first. The lock means an agent process may still be running; check that none is, then remove it." — with the lock card directly above the block; the host refuses every resolve on a locked session with 409.
- **Worked for:** a `details` summary ("Worked for 48s", "Working for 2m 41s") in Text Tertiary with a trailing chevron that rotates 90deg when open (0.25s); hover Raise with Soft Silver. Expanded, a step list sits on Rail Black with the contained edge, 12px radius, padding 12px 14px, 8px gaps. Each step: status glyph, 16px tool icon, label in 13.5px Text Secondary, command or path in silver mono, optional diff count in dim mono.

### Inputs / Fields
- **Search:** 36px, 10px radius, Raise fill, no edge, leading search icon, placeholder Text Tertiary.
- **Composer:** 18px radius, Raise fill, control edge, padding 14px 12px 10px 16px, 10px gap; textarea min 40px with silver caret; action row of the "Stop turn" button while a turn runs (the only control called that; buttons that close a form keep "Cancel"), model selector pushed right (30px, 9px radius, 13px Text Secondary, trailing chevron, hover Raise High), then the send button. Focus-within swaps the edge to Silver Edge over 0.2s. Below, a caption line (host name, working path in mono) in Text Tertiary.
- **Unlock note:** see Lock card.
- **Fields during live updates:** a field in the side panel, the New task form or the composer is never disturbed by a live update. Each render records the focused field (by its `id`, or the unlock note by its form), its selection and its scroll, and restores all three afterwards, so typing through SSE traffic loses neither focus, caret nor characters; the unlock note's `id` is unique per session (`unlock-note-<session>`).

### Navigation
- **Sidebar:** Rail Black with a 1px Line right border. Brand row (24px logo tile with 14px mark, wordmark, new-session icon button). Search. Nav rows (36px, 10px radius, 16px icon, Text Secondary; hover Raise and silver) for Needs you (orange count badge), Runs (plain Text Tertiary number) and Activity. A section label "Sessions" (12.5px 500 Text Tertiary) with a filter icon button. Footer pinned to the bottom above a Line rule: settings and help icon buttons left, pause-dispatch right.
- **Session card:** grid of 32px avatar, two text lines (name 600 silver; meta 13px Text Tertiary — for a locked session "Locked · <reason>", ellipsised with the full reason in `title`) and a right cell (12.5px Text Tertiary, or a lock glyph). 12px radius, padding 9px 10px, 12px column gap; hover Raise; selected Raise High with the selected edge. The avatar is a 32px Raise Top rounded square (10px) with the selected edge, initials in silver, and a 13px Rail Black dot at its bottom-right corner (offset -3px) holding a 10px status glyph.
- **Run rows:** nested under a session card, indented 30px left; 14px glyph, label (13.5px Text Secondary, ellipsised), time in mono; 10px radius, hover Raise.
- **Session tab bar:** 44px with a Line bottom rule; tabs are a status glyph plus session name, 13.5px Text Tertiary, padding 0 14px; hover Text Secondary; selected silver with a 2px Soft Silver underline overlapping the rule. A trailing icon button opens a session.
- **Right panel tabs:** row with 8px padding over a Line rule; 32px tabs, 9px radius, 16px icon, Text Tertiary; hover Raise with Soft Silver; selected Raise High with silver.
- **Status bar:** 30px, Rail Black, Line top rule, 12.5px Text Tertiary, 18px gaps; live counts each led by its status glyph (running, queued, need you), host facts pushed right.
- **Phone tabs:** see Layout; selected tab is Raise High with silver text and the selected edge.
- **Phone stream header / connection banner:** see Layout. The header names the session the stream belongs to; the banner is the phone's only copy of the host-reachability fact, since the status bar is hidden under 860px.

### Stream Items
- **System line:** 13.5px Text Tertiary with a leading 16px icon; ids and times inside it are data.
- **User bubble:** right-aligned, Raise High, padding 10px 15px, radius `16px 16px 6px 16px`.
- **Agent text:** plain body on Ground, bold in silver 600, inline code as Raise High mono with 6px radius and 1px 6px padding.
- **Activity line:** Text Secondary with a Text Tertiary 16px icon, then a chip for the linked object; wraps.
- **Lock line:** a locked session's stream ends with one system line — the orange lock glyph and "Locked: <reason>. Messages wait until you remove the lock." in Text Secondary, so the reason is visible where the messages stop, not only in the panel — The glyph, the text and a quiet "Remove lock…" button (28px) stay in one row: the glyph is glued to the first line of the text, only the text wraps, and the button keeps its width on the right. The button opens that session's unlock form in the right panel and puts focus in the note; on a phone it switches to the "Needs you" tab first.
- **Queued message:** right-aligned, same bubble shape, no fill, Queued Edge stroke, Text Secondary; a header line in Interrupted Yellow 12.5px led by the neon waiting ring ("Waiting · sent 14:33" pattern).

### New task section
"New task" opens a 16px card above the "Needs you" list, and the list stays visible below it. It holds a 15px "New task" title, the session as text — "Session: <name>" in Caption with the name in silver 600, never an input — then a read-only target, the base branch field and the instructions field, with a right-aligned "Cancel" and "Start task". Opening it moves focus to Instructions; Cancel closes it and returns focus to the "New task" button.

### Right panel context
The panel and stream always show the same session: selecting another session in the sidebar or the tab bar while a run of a different session is open returns the panel to the "Needs you" list rather than showing a run that does not belong to the selected session, and opening a run from anywhere (Needs you, Recent runs, a sidebar run row, or a task chip) selects its session across the stream, session tab bar, sidebar and composer while leaving the active phone tab unchanged when opened from Needs you.

### Detail View
Order: breadcrumb reading "Viewing <session> · <run id>" (back icon, "Viewing", 22px avatar, the session name, a dim "·", the run id in mono; 13.5px Text Secondary — the session is the first named crumb, so a run is never shown without the session it belongs to), title with mono run id, status row (status button: glyph, silver 500 status name, chevron; then a Text Tertiary context line), the decision line for a run just decided, chips, lock card when the session is locked, the host question (decision block) directly under the chips with the lock card, if any, directly above it, key-values (96px label column in Text Tertiary, values in Text; 8px by 12px gaps; Line rule above, 14px top padding), activity timeline (heading, then rows of 18px icon column, text in Text Secondary with silver 500 emphasis and optional Text Tertiary sub-line, mono time at right; lock events use the orange lock glyph; a turn's end reason is written as words — `exit` "finished", `cancelled` "stopped", `host_lost` "host stopped", `never_started` "never started" — so a crash reads "interrupted (host stopped)").

### Status Glyphs (signature)
All glyphs are inline SVG, `viewBox="0 0 14 14"`, rendered at 14px (10px inside an avatar dot), `fill: none`, stroke-width 1.8, round caps, rotating or scaling around (7, 7). Every glyph is `role="img"` with an `aria-label` naming its state — "running", "waiting", "needs you", "done", "failed", "locked" — and the idle ring is `aria-hidden`. Session tabs carry the same word in their accessible name ("web, locked"), so the state never exists only as colour.

- **Running:** track `<circle cx="7" cy="7" r="5.5">` in Line High; arc `<path d="M7 1.5a5.5 5.5 0 0 1 5.5 5.5">` in Neon Green, rotating 360deg every 1s, linear, infinite.
- **Waiting:** `<circle cx="7" cy="7" r="5.5">` in Neon Yellow, `stroke-dasharray: 2.88 2.88`, rotating every 6s, linear, infinite.
- **Needs you:** core `<circle r="4">` filled Neon Orange; ring `<circle r="4.5">` stroke 1.4 Neon Orange, pinging every 1.9s on the standard ease: scale 0.55 at opacity 0.9, to scale 1.7 at opacity 0 by 80%, holding to 100%.
- **Done:** `<circle r="5.5">` plus check `<path d="M4.4 7.2l1.8 1.8 3.5-3.6">` in Verified Green. Only a newly finished item draws: `stroke-dasharray: 12; stroke-dashoffset: 12` animating to 0 over 0.5s after a 0.1s delay, standard ease, fill-mode forwards. Items already done on load render complete.
- **Interrupted:** `<circle r="5.5">` plus bars `<path d="M5.6 4.8v4.4M8.4 4.8v4.4">` in Interrupted Yellow, static.
- **Locked:** shackle `<path d="M4.5 6.5V4.8a2.5 2.5 0 0 1 5 0v1.7">` stroked and body `<rect x="2.5" y="6.5" width="9" height="6" rx="1.5">` filled, in Lock Orange, static.

### Motion Grammar
- **Standard ease:** `cubic-bezier(.16, 1, .3, 1)`, a fast-out settle used for every non-linear motion.
- **Rise:** a newly arrived stream item or step fades from opacity 0 and 6px below to rest in 0.45s; the unlock form uses the same rise in 0.35s.
- **Settle:** a lock card arriving drops in over 0.7s: from 8px above, rotated -1.2deg and transparent; at 60% opaque, 1px below and rotated 0.3deg; then at rest.
- **Hover:** background, colour and edge transitions 0.15s; composer edge 0.2s; chevron rotation 0.25s on the standard ease.
- **Reduced motion:** under `prefers-reduced-motion: reduce`, every animation and transition is off and the done check is drawn in full. Every element is visible in its final state by default; motion is only ever an entrance or a loop on top of a complete page.

### Named Rules
**The Small Avatar Rule.** The 22px avatar used in tags, crumbs and the phone header is 11px initials at weight 600 (never 9px), so its two letters stay legible at phone size.

**The Sessions, Not Items Rule.** Every count that says "need you" — the sidebar row, the panel heading, the phone tab and the status bar — counts sessions that need the user, not the runs and locks inside them; the tag then shows how many things that one session is holding.

**The Unlock First Rule.** A locked session's runs show no decision controls: the page shows the lock card and the "Remove the lock on <session> first" line, and the host refuses `resolve` with 409 until the lock is gone, so the page can never offer a decision the host will reject.

**The Draw Once Rule.** The check draws only for an item that finished while the user was watching. Nothing replays on load or on scroll.

**The Exact Sha Rule.** Any control that accepts a result names the sha in its own label, and the text beside it says the decision applies to that sha only.

**The Still Page Rule.** Remove all motion and the page must say exactly the same thing.

### Named Rules (decisions)
**The Second Tap Rule.** A decision that changes a run takes two taps: the pill opens a confirm row, the confirm row sends. Escape or "Back" closes it without a request, and focus returns to where it came from.

## Do's and Don'ts

### Do:
- **Do** keep the page on Ground (#000000) with Rail Black panels and the three raise steps; give contained or selected things the next raise step up.
- **Do** swap a neon glyph for its normal-colour counterpart the instant its state stops: running arc to verified check, waiting ring to pause ring or nothing, needs-you ping to lock or resolved.
- **Do** draw edges as 1px inset box-shadows (Line High for controls, Silver Edge for selection, Lock Edge for locks).
- **Do** use the radius ladder: 10px rows, 12px session cards, 16px cards, 18px composer, full pills for answers.
- **Do** put the exact sha in the Accept pill's label and repeat it in the fine print.
- **Do** open the unlock note in place inside the lock card, with "Keep locked" beside "Sign & remove lock".
- **Do** render every element in its final state first and treat rise, settle and draw as entrances only; honour `prefers-reduced-motion` by turning all of them off.
- **Do** use monospace with tabular numerals for shas, ids, paths, commands and times.

### Don't:
- **Don't** use fluorescent green, yellow or orange on text, fills, borders, buttons or anything at rest.
- **Don't** put the brushed-silver gradient on anything except the logo mark and the send button.
- **Don't** add drop shadows, glows or blur; depth is tonal.
- **Don't** use uppercase labels, letter-spaced eyebrows or small caps; everything is sentence case.
- **Don't** set names, labels or sentences in monospace.
- **Don't** use circular avatars; avatars are rounded squares with a status dot at the corner.
- **Don't** replay the check draw on load; only newly finished items animate.
- **Don't** fill a button orange unless it removes a lock.

## Corrections after review (2026-11-25)

- The selected-session outline, avatar outlines and active edges use silver at 46% (`silver-edge`), so they read as bright silver and clear 3:1 against their grounds.
- The accept button darkens on hover (`#1f7a32`) instead of lightening, keeping its white label above 4.5:1.
- Focus rings follow each element's own radius (no radius override); the search field shows the silver edge on focus.
- The waiting ring's dash is 2.88 + 2.88 so it repeats evenly round the circle with no seam.
- Faint edges (composer, pills, running-arc track) are decorative; every control that relies on its edge alone also changes fill on hover and shows the focus ring.

## v5 optical workspace (Phase 1B, Batch C redesign)

This section governs the Rust/Tauri client in `apps/polyclave-app/ui`. Everything above it describes the archived Node UI. The pinned OpenPencil document `docs/superpowers/design/polyclave-v5-desktop-ui.fig` (artboards Deep Obsidian, Daylight Crystal, Settings and Motion) is the visual source; every shipping style, shader, sprite and icon is an original implementation.

### Tokens (`ui/src/styles/tokens.css`)

| Role | Deep Obsidian (default) | Daylight Crystal (`data-theme="light"`) |
|---|---|---|
| Ground `--bg` | `#0D0D10` | `#F4F7FB` |
| Well `--well` (topology canvas, diff, code, inputs) | `#08080B` | `#E8EEF6` |
| Card `--card` (+1 px top highlight `--edge-hi`) | `#141418` | `#FFFFFF` |
| Line `--line` | `#23252F` | `#D6DEEA` |
| Ink / mute / faint | `#F0F1F4` / `#8F93A3` / `#5D6070` | `#0F172A` / `#5B6880` / `#93A0B4` |
| Inline code chip | `#24262E` bg, `#9EB4FA` text | `#E1E7F0` bg, `#B4610F` text |
| Running (rail / wash / pill edge) | `#58CF98` / `#10221A` / `#23523D` | `#15803D` / `#E7F6EE` / `#A7DCBD` |
| Needs review | `#F09B78` / `#241612` / `#593224` | `#C2410C` / `#FFF2EA` / `#F6C3A6` |
| Verified / archived | `#8AB4F8` / `#9B72CB` | `#2E64A9` / `#704497` |
| Primary capsule (`Accept <sha>`) | `#F2F3F7` on `#0E0F14`, emerald check | `#0F172A` on white |
| Vendors | Claude `#D97757`, Codex `#F2F2F5` (light `#0F172A`), Gemini `#8E84F5`, DeepSeek `#4F8BFF` | same |
| Prism spectrum | red `#E5534B`, orange `#F0A74E`, yellow `#F2C94C`, green `#3FC085`, cyan `#5CC6FC`, blue `#4F8BFF`, violet `#9B72CB` | same |

Type: system UI (`SF Pro Text`, `PingFang SC` fallbacks) at 13.5 px body, 15 px titles, 10.5–12 px meta; monospace only for ids, SHAs, paths, model names and counters. Radii: 5 / 7 / 10 / 14 px and pills. Status is shown by a 2.5 px left rail plus a low-alpha directional wash and a translucent pill, never by a full coloured border or a solid colour block.

### Layout

- Desktop: `276px | minmax(0,1fr) | 432px` columns over a shared `34px` status bar. Side columns are 70 % ground so the aurora shows faintly through them; the centre is transparent.
- Below 860 px: three 44 px tabs (Workspace, Hub, Inspect) replace the columns; every inspector function stays reachable. Every visible control is at least 44 px tall (`--touch`), and no element may scroll horizontally. Settings becomes a full-screen sheet with a horizontally scrolling section bar.
- `[hidden]` always wins (`display:none !important`), so component `display` rules never resurrect hidden controls.

### Components

- **Left navigation:** prism logo, theme toggle (sun/moon) and a single-icon register button; `Global Hub` (cross-project overview) and `Needs you` (count of runs waiting for review); a collapsible project tree grouped by repository, each project listing its Hub/Advisor sessions and its latest Worker runs with status dot and label; Settings at the foot. Collapsed projects persist per browser.
- **Conversation:** breadcrumb (project / session), active ticket chip (`Ticket <id> · used/limit runs`), and a custom listbox model selector. The DeepSeek API profile is listed first with `deepseek-flash (default)` before `deepseek-v4-pro`; this is display order only, not a provider switch. Messages: user bubbles with `Remember this rule` and `Create ticket`; Hub cards with a collapsible thought-process card, tool-call rows from `metadata.tool_calls` and inline code chips; host lines centred and muted; Worker run cards interleaved by time with vendor/model tags, attempt progress and a review entry. A live turn shows streaming reasoning and text; the stream itself stays one text node so `CSS.highlights` can fade it.
- **Composer:** glass card with 1.5 px prism rims top and bottom (brighter on focus), no resize handle (auto-grows to 200 px), model pill, `Auto` cut-crystal toggle, memory chip (verified count), 512K context vial with label, stop and send buttons with a conic prism ring, and the pixel prism with two sparks at the top-right corner. Enter sends; Shift+Enter adds a line; sending while a turn runs is allowed (queued).
- **Inspector:** four tabs with arrow-key navigation and a review badge.
  - *Topology:* the Hub node above its Worker nodes on a dotted well; SVG Bézier wires (first row direct, lower rows down a centre trunk), tinted by status (running dashed green, review orange, verified blue, failed red); node shows vendor tag, model, status and attempt progress. Tickets list with `Dispatch` per open ticket, a free `Dispatch Worker`, and a quick card for the run waiting on the user.
  - *Review:* run picker, status pill, `base` / `observed` / `result` SHA chips, host detail and resolution, the **approval evidence card** (below), `What changed` (commit list and per-file `+/−` from numstat or diffstat), decision note (required for `Mark failed` and `Allow retry`), a two-step `Accept <sha>` → `Confirm accept <sha>` that sends only the displayed SHA, `Retry` for failed or interrupted runs, `Cancel` for pending or running runs, and a `worker-chat` box while the run is open. A note on fail/allow-retry opens the `Remember this rule` draft for the run.
  - *Approval evidence card* (`views/evidence.ts`), directly under the run head, so a phone shows it first. It keeps three judgments apart for the SHA being approved (`observed_sha`, else `result_sha`):
    - **Summary line** (13.5 px 600 Ink): `<delivery> · <tests> · <decision>`, for example "Commit not verified · no test result for bbbbbbb · waiting for your decision" or "Commit verified · tests passed for ddddddd · not judged by you". It says tests passed only when a CI (or host-run) result was recorded for exactly the Run's verified `result_sha`; with no recorded result it says "no test result for <sha>", with only a Worker claim it says "worker claim only for <sha>", with results for an observed-but-unverified commit it says "tests recorded, not verified, for <sha>".
    - **Three rows**, each a 2.5 px left rail (tone colour, `--line` when neutral) with a 12 px 600 label, a state pill at right (wraps under the label on a phone), 12 px Ink-2 facts and a 10.5 px Mute source line naming where the evidence comes from:
      1. *Delivery verified (R1)* — `Verified` (verified blue) only for `done` with `result_source = agent`; `Needs review` (orange) when a writer may still reach the Attempt; otherwise `Not verified` (red) with the host's reason from Run classification, plus "Receipt: <receipt_sha> · mirror tip: <observed_sha>"; `Not checked yet` (neutral) while the Worker runs. A user-accepted run stays `Not verified` and says it was accepted by hand.
      2. *Tests for this exact SHA* — rows are read from `test_results` for exactly the evidence SHA (`test_sha`), never another commit. States: `No result recorded` (neutral, no row), `Worker’s claim` (orange, a `worker_claim` row only — never green, whatever it says), `Passed` (verified blue, CI/host-run rows for the verified `result_sha`), `Failed` (red, any such row failed), `Running` (orange, a check still pending) and `Reference only` (orange, checks passed for the observed SHA that the host has not verified). Each CI fact names the check and the SHA; when the Worker also filed a claim for the same SHA it is quoted after the checks and says "A CI result wins over a claim", so a claim can never mask a failure.
      3. *Request satisfied* — always `Your decision` (orange). Shows the Worker's latest `report_progress` text for the current Attempt as "Worker's claim: “…”" (clipped at 600 characters), then the optional `tests` / `tests_status` / `not_done` fields the Worker submitted with `submit_result` ("Tests, as the Worker submitted them (passed): “…”" and "Not done or unverified, per the Worker: “…”"), each labelled as the Worker's own statement. With no such field it says the Worker submitted no test summary.
    - Evidence comes from `GET /api/runs/{id}` → `evidence` (`RunEvidenceDto`), read for the current Attempt only. `test_results` holds one row per `(run, sha, source, name)` and is filtered to `test_sha` (the Run's verified `result_sha`, else its `observed_sha`), so a result recorded for an older commit can never appear as current evidence. The decision card below is titled "Your decision on <sha>" and its Accept pill still names that SHA.
  - *Files:* read-only viewer with line numbers, path input limited to the registered repository by the daemon.
  - *Memory:* verified/candidate records with kind, scope, status and tag pills, kind filters and Archive. New rules come only from a real user message or a run decision and are saved only on `Confirm and save` (draft, then commit).
- **Status bar:** connection dot and text, counts for running, thinking (streaming Hub sessions), to review and done; four quota marks. Claude, Codex and Gemini use a double ring (outer 5-hour, inner weekly); a field that is not reported is drawn as a dashed ring with `—`, never as a made-up value. DeepSeek has no 5-hour window and uses a solid dot with its balance when reported.
- **Settings · AgentMesh nodes:** one card per registered node: name, OS and status, endpoint and allowed repositories in mono (wrapping anywhere), and `Revoke`. Under a hairline (`--line-soft`), unless the node is revoked, a `Concurrent Runs` row: a hint with the node's Runs running on this host and the rule (over the limit is refused, not queued; default 3, up to 16), and a pill stepper on the well (`−`, the value in mono tabular figures, `+`), each button `--touch` square (32 px desktop, 44 px phone). Each step saves at once through `POST /api/mesh/nodes/{id}/concurrency`; the value is read back from the reply, and `−` / `+` disable at 1 and 16.
- **Settings · Usage & limits:** section nav; vendor cards (5-hour and weekly meters or `not reported`, DeepSeek balance, local 24 h tokens, cost and turns, high-water bars turn `#E8835C` at 80 %); the 512K vial with the largest recent prompt; a 24 h bar chart (hourly when the host reports a series, otherwise tokens per provider from the local ledger, labelled as such); motion intensity `off` / `subtle` / `standard` / `ultra`; theme; native keychain rows (`key_status`, `save_key` with the value trimmed and the field cleared, and `delete_key` when the desktop build provides it). Key values are never displayed, only `configured` and the masked tail.

### Motion

- One `FrameClock` drives everything: interactive bursts, about 30 fps while agents work (60 fps on `ultra`, slower on `subtle`) and sleep to `0fps` 1.2 s after idle or review. While asleep, `data-motion="0fps"` is set, the aurora canvas backing store is zeroed and a static CSS afterglow keeps the horizon visible.
- **Global horizon aurora:** full-width fixed canvas at the bottom of the viewport, beneath the workspace (`z-index` 0, `pointer-events: none`). WebGL2 shader with premultiplied alpha (WebKit composites it as light, not as a pale wash): a floor glow and three slow cobalt, cyan and iris curtains, faded at the side edges and the top, alpha at most ~0.64 in dark and ~0.4 in light. Rendered at half resolution. Canvas 2D radial gradients are the fallback.
- **Pixel partners:** four original 16 × 10 sprites, no black outlines, 1 × 2 vertical eyes, drawn at 3× (2× on phones) above the composer: **Deepy** (legless cobalt whale with a raised tail fluke and a cyan spout), the silver dragon, **Clawd** (terracotta, antenna and side arms) and the violet cat (ears and tail). States: idle (eyes closed, pixel “z”), thinking (staggered bob, thought dots, the cat hops onto Clawd every other beat), running (hops, alternating legs, spout), review (Clawd raises “!”, the others look at it). In light theme a hairline drop shadow keeps the silver dragon legible.
- Ink: five `CSS.highlights` tiers set foreground alpha 16 / 38 / 62 / 84 / 100 % on newly streamed ranges; all ranges are cleared when the stream ends. Optical caret follows the selection with a spring. SSE-driven re-renders go through `withFocusPreserved`.
- `prefers-reduced-motion: reduce` or intensity `off`: no animation, no canvas frames, plain streaming text, static pets and afterglow; all text stays readable.

### Boundaries

The renderer talks only to the authenticated loopback daemon (`Authorization: Bearer`, `#token=` cleared from the address bar, or Tauri `local_connection`). No external fonts, scripts, images, analytics or provider calls; the favicon is an inline data URL. All text is inserted with `textContent`. Contract types come from the generated `src/generated/proto.ts`, which is never edited by hand.
