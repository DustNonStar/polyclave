# Approval card evidence — measurements

Command: `POLYCLAVE_BROWSER_CHANNEL=chrome POLYCLAVE_HANDCHECK_DIR=/tmp/polyclave-hc/approval-card-evidence/png node tests/approval-card.mjs` (system Chrome, reduced motion, fixture server on 127.0.0.1, no external requests).

Fixtures: `r1` needs_review, receipt `aaaaaaa` ≠ mirror tip `bbbbbbb`, detail "mirror tip differs from the receipt", Worker progress claim present. `r2` done, `result_source = agent`, receipt = tip `ccccccc`, no claim. Neither has a test record (the host stores none).

| Run | Viewport | Theme | `documentElement.scrollWidth` | Card left–right (px) | `#inspect` elements with visible horizontal overflow | `#inspect` buttons/selects under 44 px |
|---|---|---|---|---|---|---|
| r1 | 1440×900 | dark | 1440 | 1023–1426 | none | (desktop, not checked) |
| r1 | 1440×900 | light | 1440 | 1023–1426 | none | (desktop, not checked) |
| r1 | 375×812 | dark | 375 | 10–365 | none | none |
| r1 | 375×812 | light | 375 | 10–365 | none | none |
| r2 | 1440×900 | dark / light | 1440 | 1023–1426 | none | — |
| r2 | 375×812 | dark / light | 375 | 10–365 | none | — |

Summary lines read from the page:

- r1: "Commit not verified · no test result for bbbbbbb · waiting for your decision"
- r2: "Commit verified · no test result for ccccccc · not judged by you"

Both summaries and both tests rows fail the "claims a pass" pattern `/(tests?|checks?|ci)\b[^·]*\b(pass(ed|ing)?|green|succeeded|ok)\b|✓|✔/i`. The tests-row pill has no tone (not green) even when delivery is verified (r2). The Accept pill reads "Accept bbbbbbb" for r1.

Screenshots: `r1-desktop-{dark,light}.jpg`, `r1-phone-{dark,light}.jpg` (viewport), `r1-desktop-dark-card.jpg`, `r1-phone-light-card.jpg`, `r2-{desktop,phone}-dark-card.jpg` (card only), `r1-phone-{dark,light}-accept.jpg` (decision card with "Your decision on bbbbbbb" and "Accept bbbbbbb").
