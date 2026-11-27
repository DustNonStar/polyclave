# mesh-budget-ac hand check: measurements

Scratch daemon: `POLYCLAVE_STATE_DIR=/tmp/polyclave-hc/mesh-budget-ac/state POLYCLAVE_PORT=4417 target/debug/polyclaved`
(built from the branch). Two mesh nodes registered through `POST /api/mesh/nodes`; `node-linux` set to 5
through `POST /api/mesh/nodes/node-linux/concurrency`, `node-win` left at the default.

Screenshots taken with the UI's Playwright (system Chrome) at 1440 × 900 and 375 × 812 (mobile, touch),
Settings opened, AgentMesh nodes section in view. Measured with `getBoundingClientRect` in the page.

| Check | 1440 | 375 |
|---|---|---|
| `documentElement.scrollWidth` | 1440 (no horizontal overflow) | 375 (no horizontal overflow) |
| Elements in `.mesh-node` with `scrollWidth > clientWidth` and visible overflow | none | none |
| Node card width | 878 px | 351 px |
| Stepper `−` / `+` buttons | 32 × 32 px (desktop `--touch`) | 44 × 44 px |
| Values shown on load (`node-linux`, `node-win`) | 5, 3 | 5, 3 |
| After `+` on `node-win` (saved, read back in the DOM) | 5, 4 | 5, 4 |
| After `−` on `node-win` | 5, 3 | 5, 3 |
| Requests to hosts other than 127.0.0.1 | 0 | 0 |

After the run, `GET /api/mesh/limits` returned `node-win` = 3 again (the script's `−` restored it).

Files: `itemCUI-desktop-mesh.jpg`, `itemCUI-desktop-mesh-after-plus.jpg`, `itemCUI-phone-mesh.jpg`,
`itemCUI-phone-mesh-after-plus.jpg`. The nodes show `(offline)` because their private endpoints do not exist;
the heartbeat probe marked them offline. The setting is shown for every node that is not revoked.
