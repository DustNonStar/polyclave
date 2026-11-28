# hardening-batch3 — browser measurements

Head `43d98cc`, 2026-11-28. All four items are on the branch. Every number comes
from `POLYCLAVE_BROWSER_CHANNEL=chrome npm run test:browser` (offline; each page is
served by the test itself and nothing leaves the machine).

| test | viewport | body scrollWidth | viewport width | overflowing elements | claim checked |
|---|---|---|---|---|---|
| pairing flow · paired | 1440x900 | 1440 | 1440 | none | scan → exchange → secret stripped from the URL → device token authorizes API + SSE |
| pairing flow · paired | 375x812 | 375 | 375 | none | same, phone width |
| pairing flow · revoked | 1440x900 | 1440 | 1440 | none | revoked token never shows "Local daemon connected"; reconnect dialog present |
| pairing flow · revoked | 375x812 | 375 | 375 | none | same, phone width |
| pr gate approval · awaiting | 1440x900 | 1440 | 1440 | none | gate shows `awaiting human approval`, the exact SHA, the diff after one fetch; `state.enabled === false` |
| pr gate approval · awaiting | 375x812 | 375 | 375 | none | same, phone width; diff panel capped and scrollable |
| pr gate approval · approved | both | — | — | — | the POST carried exactly `bbbbbbb…` and only then `enabled === true` |
| restart reconnect · rotated | 1440x900 | 1440 | 1440 | none | after the token rotates the page stops claiming to be connected and no stale delta from the old stream is shown |
| restart reconnect · rotated | 375x812 | 375 | 375 | none | same, phone width |
| restart reconnect · recovered | both | — | — | — | a fresh token snapshots the same run without the old delta |

Screenshots (`JPEG q70`, each < 60 KB): `item2-phone-paired.jpg`,
`item2-phone-revoked.jpg`, `item3-phone-awaiting.jpg`,
`item3-desktop-awaiting.jpg`, `item3-phone-approved.jpg`,
`item4-phone-rotated.jpg`.
