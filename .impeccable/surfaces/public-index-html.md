---
version: 1
slug: "public-index-html"
primary_target: "public/index.html"
related_targets: []
---

## Scope

The Polyclave host page (phase 1 plan, Task 10): `public/index.html` + `public/app.js`. Mode: Operate. Users: the owner on a Mac and on a phone equally; must stay readable for a newcomer.

## Direction contract

THESIS: The page is a lockout station, worn lightly. Everything that waits on the user hangs as an orange lockout tag that can only be removed with a signed note; everything else reads as quiet stamped nameplates. It refuses the category default: a gray three-pane chat app with one neon accent and status pills.

OWN-WORLD (revised 2026-11-25, third round of user feedback): pure black ground with three soft grey steps, bright silver for titles, the selected session outline, the active tab underline, and a brushed-silver gradient on the logo mark and the send button; generous softness (10–12px rows, 16px cards, 18px composer, pill-shaped answer buttons), sentence-case system UI type. Richer Orca-like structure: sidebar with search, nav (Needs you / Runs / Activity), session cards with avatars and nested run rows; main area with session tabs, collapsible "Worked for" blocks, activity lines with chips, a composer with model selector; right panel with tabs and a detail view (breadcrumb, title, status, chips, key-values, activity timeline, a host question with pill answers); a bottom status bar. Fluorescent colour only for what is in motion (neon green running arc, neon yellow waiting ring, neon orange needs-you ping); finished and static states in normal green / yellow / orange.

STORY: The user opens the page, sees at once which sessions are locked out and which runs need a decision, removes a lock or decides a run with the exact sha in front of them, and otherwise watches agents work in a calm stream.

FIRST VIEWPORT: Top strip: host nameplate (instance, uptime, dispatch open). Left rail: session nameplates with energized/queued/locked indicators. Centre: the selected session's stream with turn separators and collapsible agent event lines, composer at the bottom. Right column, titled "Needs you": lockout tags first, then work orders needing review, then interrupted runs. Primary action sits on the tag itself.

FORM: lockout-tagout tags and equipment nameplates, candidate 5 of 7; seed: none (unavailable)

FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance

Signature move: the lockout tag. A frozen session's tag shows reason, turn, time, and a "Remove lock" control that expands in place into a note field and a signed confirm; a run needing review is a work order whose decision row repeats the sha being accepted.
