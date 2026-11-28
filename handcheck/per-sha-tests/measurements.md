# Per-SHA test results: hand check

Offline browser run of `tests/approval-card.mjs` at 1440×900 and 375×812, dark and light
(`POLYCLAVE_HANDCHECK_DIR` set). All five fixtures pass at both sizes and both themes.

## What the screenshots show

| File | What it shows |
|---|---|
| `r3-desktop-light.jpg` / `r3-desktop-dark.jpg` | A verified Run with CI passed for its checked-in SHA: summary "Commit verified · tests passed for dddddd · not judged by you", tests row state `Passed` with facts "CI: ci/rust-test — passed at dddddd." and the Worker's claim quoted after it. An older commit's failed row (`ci/old`) is not shown. |
| `r3-phone-light.jpg` / `r3-phone-dark.jpg` | The same card at 375: summary wraps to two lines, the tests row stacks the pill under the label, no horizontal overflow. |
| `r4-desktop-dark-card.jpg` | CI failed while the Worker claimed a pass: state `Failed`, tone red, the claim is still quoted and the fact says a CI result wins. |
| `r5-phone-dark-card.jpg` | Only a `worker_claim` row: state "Worker’s claim", tone orange, never green; summary stays "Commit not verified · worker claim only for ffffff · waiting for your decision". |
| `r1-phone-light-card.jpg` | The unchanged no-record case still reads "No test result recorded for bbbbbbb." |

## Measurements (from the run's JSON log)

- No element inside `#inspect` has `scrollWidth > clientWidth + 1` with visible overflow in any of the five fixtures, at 1440 and at 375 (`wide: []` for every case).
- `document.documentElement.scrollWidth` equals the viewport width in every case (1440 / 375).
- The evidence card stays inside the viewport: at 375 its bounds are 10 → 365 px.
- Touch targets under 44 px inside `#inspect` at 375: none (`under44: []`).
- Tests row tone per fixture, read from the rendered pill: r1 `none`, r2 `none`, r3 `done`, r4 `fail`, r5 `review`.

## Rust side

`cargo test --locked --workspace` covers the same rules below the UI: rows are keyed by
`(run, sha, source, name)`, a result for another SHA is never returned, an older check-run
conclusion is never written over a newer SHA, and a resend of `submit_result` that only
rewords its notes keeps the first accepted receipt.
