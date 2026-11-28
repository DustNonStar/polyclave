# hardening-batch3 — browser measurements

Head `1da3de1` + assertion update, 2026-11-28. Only Items 3.1–3.2 exist on this
branch; Items 3.3–3.4 are not done (see `PLAN-ISSUES.md`).

`POLYCLAVE_BROWSER_CHANNEL=chrome node tests/pairing-flow.mjs`, both widths:

| step | viewport | body scrollWidth | viewport width | overflowing elements |
|---|---|---|---|---|
| paired | 1440x900 | 1440 | 1440 | none |
| paired | 375x812 | 375 | 375 | none |
| revoked | 1440x900 | 1440 | 1440 | none |
| revoked | 375x812 | 375 | 375 | none |

- The pairing page is reached over `https` (Tailscale origin shape) in the test;
  every request is served by the test, so no network leaves the machine.
- Device token authorizes `/api/*` and the SSE stream (`state.sse >= 1`); a
  replayed link is refused by the host (ticket spent once) and the device keeps
  the token it had; a revoked token never shows "Local daemon connected".
- At 375 the whole pairing + revoked flow fits the viewport with no horizontal
  overflow (`documentElement.scrollWidth == innerWidth`).

Screenshots (`JPEG q70`, each < 100 KB): `item2-phone-paired.jpg`,
`item2-phone-revoked.jpg`.
