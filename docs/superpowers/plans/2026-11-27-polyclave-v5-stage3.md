# Polyclave v5 Stage 3 Implementation Plan

> **Scope:** Tauri v2 mobile entrypoint (iOS / Android), `google/agentmesh` (`polyclave-mesh`) cross-device zero-trust federation with deny-by-default attenuated capability tokens, 10-minute one-time QR pairing + dual-identity remote authentication (`Tailscale` / `AgentMesh`), and pure private self-hosted notifications (`ntfy` / UnifiedPush).
>
> **Spec references:**
> - `docs/superpowers/specs/2026-11-26-polyclave-v5-rust-design.md` (§1.2, §2, §5 阶段 3)
>
> **Owner decisions (2026-11-27):**
> 1. Execute in 3 batches: **3A** (`proto` + `store` pairing/devices/mesh/notifications) → **3B** (`polyclave-mesh` + `PrivateNotifier` + `polyclaved` dual-identity auth & routes) → **3C** (Tauri v2 `mobile_entry_point` target-gated lib + Stage 3 Pairing QR / Devices / Mesh / Private Notifications UI, owned by Local Lead).
> 2. Land Tauri v2 mobile entrypoint + 375px pairing/notification UI without requiring local Xcode/Android NDK, keeping macOS & Windows CI 100% green.

---

## Standing Invariants

1. **10-minute one-time pairing tickets:** Ticket secrets are hashed in SQLite (`sha256`), single-use, expire after 600s, and exchange for a long-lived device bearer token (also stored hashed only, never plaintext in SQLite, logs, or `change_log`).
2. **Dual-identity verification on non-loopback:** Remote requests arriving over `tailscale serve` or Agent Mesh require BOTH a valid non-revoked device bearer token AND a matching trusted node/peer identity header (`Tailscale-User-Login` or `X-AgentMesh-Node`), while loopback keeps existing capability auth.
3. **Deny-by-default cross-device federation (`polyclave-mesh`):** Attenuated capability tokens (scoped by `node_id`, `repo_cwd`, `actions`, `expires_at` with monotonic attenuation — child tokens can only narrow scope) enforced deny-by-default; R1 verification and the master SQLite ledger remain strictly on the primary `polyclaved` host.
4. **Pure private notifications (zero third-party commercial IM clouds):** `ntfy` / UnifiedPush HTTP targets are restricted to loopback, Tailscale (`100.64.0.0/10`, `*.ts.net`), private RFC1918, or configured mesh endpoints; payloads contain ONLY metadata (`topic`, `entity_id`, `counts`, short title — never conversation text, code, or secrets); mobile wakes and syncs via `Last-Event-ID`.
5. **Zero external UI requests & instant revocation:** Inline SVG QR rendering with zero external dependencies; revoking a device immediately invalidates its bearer token and disconnects its SSE stream.

---

## Batch 3A: Protocol Contracts & Store Schema

**Owned directories:**
- `crates/polyclave-proto/`
- `crates/polyclave-store/`
- `apps/polyclave-app/ui/src/generated/proto.ts`

### Item 3A.1 — Extend `polyclave-proto` for Stage 3 & regenerate `proto.ts`
- **Commit:** `feat(proto): add Stage 3 pairing, device, mesh, and private notification contracts`
- **Changes:**
  - Extend `ChangeTopic`: `Device`, `MeshNode`, `Notification`.
  - Add DTOs with `#[derive(TS)]` and export into `apps/polyclave-app/ui/src/generated/proto.ts`:
    - `PairingTicketDto` (`id`, `client_kind`, `device_label`, `expires_at`, `used_at`, `created_at` — never includes secret/hash)
    - `PairedDeviceDto` (`id`, `label`, `client_kind`, `peer_identity`, `last_seen_at`, `revoked_at`, `created_at` — never includes token/hash)
    - `MeshNodeStatus` (`Online`, `Offline`, `Revoked`)
    - `MeshNodeDto` (`id`, `name`, `endpoint`, `os_family`, `allowed_repos: Vec<String>`, `allowed_actions: Vec<String>`, `status: MeshNodeStatus`, `last_heartbeat_at`, `created_at`)
    - `NotificationChannelKind` (`Ntfy`, `UnifiedPush`)
    - `NotificationChannelDto` (`id`, `kind`, `label`, `endpoint`, `topic`, `enabled`, `created_at`)
  - Keep `TurnDto` compile-fail doctest passing.

### Item 3A.2 — Add Stage 3 durable tables & single-writer methods in `polyclave-store`
- **Commit:** `feat(store): add pairing_tickets, paired_devices, mesh_nodes, and notification_channels tables`
- **Changes:**
  - Additive SQLite schema migration v6 → v7 (`CREATE TABLE IF NOT EXISTS` in `crates/polyclave-store/src/stage3.rs`):
    - `pairing_tickets` (`id`, `ticket_hash`, `client_kind`, `device_label`, `expires_at`, `used_at`, `created_at`)
    - `paired_devices` (`id`, `label`, `client_kind`, `peer_identity`, `token_hash`, `last_seen_at`, `revoked_at`, `created_at`)
    - `mesh_nodes` (`id`, `name`, `endpoint`, `os_family`, `allowed_repos_json`, `allowed_actions_json`, `status`, `last_heartbeat_at`, `created_at`)
    - `notification_channels` (`id`, `kind`, `label`, `endpoint`, `topic`, `enabled`, `created_at`)
  - Single-writer methods:
    - `create_pairing_ticket`, `exchange_pairing_ticket` (atomic expiry + single-use check + `paired_devices` insert), `verify_device_token` (returns `PairedDeviceDto` if non-revoked & matching optional `peer_identity`), `list_paired_devices`, `revoke_paired_device` (appends `ChangeTopic::Device` to `change_log` with zero hashes/secrets).
    - `upsert_mesh_node`, `list_mesh_nodes`, `revoke_mesh_node`.
    - `upsert_notification_channel`, `list_notification_channels`, `delete_notification_channel`.
  - Unit tests in `crates/polyclave-store/tests/store.rs` covering ticket expiry, single-use replay rejection, instant device revocation, and zero token/hash leakage in `change_log`.

---

## Batch 3B: `polyclave-mesh`, Private Notifier, and Daemon Remote Auth

**Owned directories:**
- `Cargo.toml` (add `crates/polyclave-mesh` member)
- `crates/polyclave-mesh/` (new crate)
- `crates/polyclave-hub/src/{notify.rs,lib.rs}`
- `bins/polyclaved/`

### Item 3B.1 — `polyclave-mesh` deny-by-default federation & attenuated tokens
- **Commit:** `feat(mesh): add deny-by-default polyclave-mesh federation and attenuated token engine`
- **Changes:**
  - Add workspace crate `crates/polyclave-mesh/` (`MeshPolicy`, `AttenuatedToken`, `MeshClient` using `reqwest` + `hmac`/`sha2` or `sha2`-based keyed MAC with `zeroize`).
  - Enforce deny-by-default authorization (`authorize(token, node_id, repo_cwd, action, now)`): rejects expired, wrong node, wrong repo, or unpermitted action; supports monotonic `attenuate(child_scope)` that can only shrink `allowed_repos`, `allowed_actions`, or `expires_at`.
  - Offline unit tests including local `axum` mock remote node health/dispatch checks.

### Item 3B.2 — Pure private self-hosted `ntfy` / UnifiedPush notifier
- **Commit:** `feat(hub): add private self-hosted ntfy and UnifiedPush notifier`
- **Changes:**
  - Add `crates/polyclave-hub/src/notify.rs` (`PrivateNotifier` trait + `HttpPrivateNotifier` using `reqwest` with redirects disabled).
  - URL guard (`validate_private_endpoint`): allows ONLY `127.0.0.1`/`::1`/`localhost`, RFC1918 (`10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`), Tailscale CGNAT (`100.64.0.0/10`), `.ts.net`, or `.local` hosts; rejects public internet/third-party IM clouds.
  - Metadata-only payload builder (`NotificationMetadata { topic, entity_id, title, counts }`): enforces size cap and strips any sensitive fields.
  - Offline tests against local `axum` mock `ntfy` / UnifiedPush receiver.

### Item 3B.3 — `polyclaved` pairing, dual-identity remote auth, mesh & notification routes
- **Commit:** `feat(daemon): add pairing exchange, dual-identity remote auth, mesh, and notification routes`
- **Changes:**
  - Expose routes in `bins/polyclaved/`:
    - `GET /api/pairing/tickets`, `POST /api/pairing/tickets` (auth required; returns one-time ticket secret + QR payload once)
    - `POST /api/pairing/exchange` (unauthenticated pre-pairing exchange consuming valid 10-min ticket + peer identity, returning device bearer token once)
    - `GET /api/devices`, `POST /api/devices/{id}/revoke`
    - `GET /api/mesh/nodes`, `POST /api/mesh/nodes`, `POST /api/mesh/nodes/{id}/revoke`
    - `GET /api/notifications/channels`, `POST /api/notifications/channels`, `DELETE /api/notifications/channels/{id}`, `POST /api/notifications/test`
  - Extend auth middleware: loopback keeps capability token; non-loopback requests require valid non-revoked device bearer token AND matching `Tailscale-User-Login` or `X-AgentMesh-Node` header.
  - Dispatch metadata-only private notifications automatically when high-attention `change_log` events occur (`needs_review`, `awaiting_user`, `scope_violation`, `paused`).
  - Offline tests in `bins/polyclaved/src/hub_api/tests.rs` & `routes.rs`.

---

## Batch 3C: Tauri v2 Mobile Entrypoint & Stage 3 UI (Owned by Local Lead)

**Owned directories:**
- `apps/polyclave-app/src-tauri/`
- `apps/polyclave-app/ui/`
- `DESIGN.md`

### Item 3C.1 — Tauri v2 mobile entrypoint + Pairing QR, Devices, Mesh & Notification UI
- **Commit:** `feat(app): add Tauri v2 mobile entrypoint and Stage 3 pairing, devices, mesh, and notifications UI`
- **Changes:**
  - Add `apps/polyclave-app/src-tauri/src/lib.rs` with `#[cfg_attr(mobile, tauri::mobile_entry_point)] pub fn run()` (cleanly target-gated so macOS/Windows desktop builds and CI stay green without local Xcode/Android NDK).
  - Add UI **Devices & Remote Access** section (accessible from Settings and a compact header/nav trigger):
    - **10-min Pairing Ticket + Inline SVG QR** (zero external requests), copyable pairing payload, expiry countdown.
    - **Paired Devices** list with `client_kind`, `peer_identity`, `last_seen_at`, and one-click **Revoke**.
    - **Mesh Nodes** panel showing `Online`/`Offline`/`Revoked` nodes, allowed repos/actions, and **Revoke** button.
    - **Private Notifications (`ntfy` / UnifiedPush)** setup form + **Send test ping** button (explicitly showing metadata-only privacy guarantee).
  - Update `DESIGN.md` and `apps/polyclave-app/ui/tests/offline-browser.mjs` (1440×900 & 375×812, ≥44px touch targets, zero overflow, zero external requests).
