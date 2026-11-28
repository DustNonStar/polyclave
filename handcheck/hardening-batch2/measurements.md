# hardening-batch2 — browser measurements

`POLYCLAVE_BROWSER_CHANNEL=chrome node tests/approval-card.mjs`, 2026-11-27, head `1016382`.
Every reachable run (r1–r7), both widths, both themes. `wide` lists elements whose
`scrollWidth > clientWidth` with visible overflow; `card` is the evidence card's
left/right edge in CSS px.

| run | viewport | theme | body scrollWidth | viewport width | overflowing elements | card [left,right] |
|---|---|---|---|---|---|---|
| r1 | 1440x900 | dark | 1440 | 1440 | none | [1023, 1426] |
| r2 | 1440x900 | dark | 1440 | 1440 | none | [1023, 1426] |
| r3 | 1440x900 | dark | 1440 | 1440 | none | [1023, 1426] |
| r4 | 1440x900 | dark | 1440 | 1440 | none | [1023, 1426] |
| r5 | 1440x900 | dark | 1440 | 1440 | none | [1023, 1426] |
| r6 | 1440x900 | dark | 1440 | 1440 | none | [1023, 1426] |
| r7 | 1440x900 | dark | 1440 | 1440 | none | [1023, 1426] |
| r1 | 1440x900 | light | 1440 | 1440 | none | [1023, 1426] |
| r2 | 1440x900 | light | 1440 | 1440 | none | [1023, 1426] |
| r3 | 1440x900 | light | 1440 | 1440 | none | [1023, 1426] |
| r4 | 1440x900 | light | 1440 | 1440 | none | [1023, 1426] |
| r5 | 1440x900 | light | 1440 | 1440 | none | [1023, 1426] |
| r6 | 1440x900 | light | 1440 | 1440 | none | [1023, 1426] |
| r7 | 1440x900 | light | 1440 | 1440 | none | [1023, 1426] |
| r1 | 375x812 | dark | 375 | 375 | none | [10, 365] |
| r2 | 375x812 | dark | 375 | 375 | none | [10, 365] |
| r3 | 375x812 | dark | 375 | 375 | none | [10, 365] |
| r4 | 375x812 | dark | 375 | 375 | none | [10, 365] |
| r5 | 375x812 | dark | 375 | 375 | none | [10, 365] |
| r6 | 375x812 | dark | 375 | 375 | none | [10, 365] |
| r7 | 375x812 | dark | 375 | 375 | none | [10, 365] |
| r1 | 375x812 | light | 375 | 375 | none | [10, 365] |
| r2 | 375x812 | light | 375 | 375 | none | [10, 365] |
| r3 | 375x812 | light | 375 | 375 | none | [10, 365] |
| r4 | 375x812 | light | 375 | 375 | none | [10, 365] |
| r5 | 375x812 | light | 375 | 375 | none | [10, 365] |
| r6 | 375x812 | light | 375 | 375 | none | [10, 365] |
| r7 | 375x812 | light | 375 | 375 | none | [10, 365] |

Tallest card content is the r6 phone card: the two new rows (r6 hand accept,
r7 incomplete check-run list) add at most two fact lines each and no new
controls, so no element is under 44 px on the phone viewport (asserted in the run).
