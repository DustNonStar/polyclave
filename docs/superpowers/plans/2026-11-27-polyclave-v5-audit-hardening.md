# Polyclave v5 Audit Hardening Plan (3 Batches)

**Baseline:** `main@3489145` (plus the Codex adversarial audit recorded in `HANDOFF.md`)
**Goal:** Close all 14 findings from the 2026-11-27 Codex adversarial audit before running real-environment end-to-end acceptance for the narrowed single-story milestone (`PLAN-ISSUES.md`, "Narrow the next milestone to one end-to-end user story").

## Recorded Owner Decisions (2026-11-27)

1. **Decision for Audit #1 (Codex macOS `.git` sandbox):** Add a restricted host-side MCP tool (`commit_and_stage`) so the host performs `git add` / `git commit` (with mandatory `Run-Id` trailer) inside the Attempt worktree and pushes to the Attempt staging repo. Sandboxed Workers (Codex `workspace-write`, Claude Seatbelt) never need write access to `.git`.
2. **Decision for Audit #4 (human approval before merge):** After heterogeneous Reviewer approval, the PR gate stops in an explicit `awaiting_human_approval` state. The phone/desktop approval card displays the exact SHA's full mirror diff, per-SHA test results, and open/not-done items; only an explicit human POST approving that exact SHA may trigger merge / `enableAutoMerge`.

## Execution Rules

- **3 separate batches, 3 separate branches**, **one commit per item** with the commit message listed below.
- Do **not** merge into `main` yourself and do **not** push `main` from a batch branch. After each batch passes all checks, stop and hand back to the merge reviewer (`HANDOFF.md` report + `handcheck/<job>/` if UI changed) so it can be reviewed and fast-forwarded into `main` before the next batch branches from updated `main`.
- Follow `AGENTS.md`: never weaken, skip, or delete an existing test; any required edit to an existing test must be recorded in `PLAN-ISSUES.md` with exact justification; never use fixed sleeps in tests; `bash scripts/privacy-scan.sh` must print `clean`.

---

## Batch 1 (`fix/hardening-batch1-runtime`) — Runtime Foundation & Sandbox Security

Covers Codex audit items **#12, #11, #7, #1**.

### Item 1.1: Drain stdout channel before joining reader on worker exit (Audit #12)
- **Commit:** `fix(core): drain stdout channel before joining reader on worker exit`
- **Target:** `crates/polyclave-core/src/runner.rs:356,413`
- **Change:** In `run_process`, continue draining `receiver` while waiting for `reader_thread` to exit (or use a non-blocking drain + bounded post-exit pipe drain timeout) so >256 stdout NDJSON lines emitted right before process exit can never deadlock between `events.send(...)` and `handle.join()`.
- **Test:** Add a barrier/regression test in `crates/polyclave-core/tests/worker.rs` where a fake worker emits >256 events and exits immediately; verify the Run settles cleanly, MCP token is revoked, and the execution slot is freed.

### Item 1.2: Persist Attempt reservation before preparing worktree and staging (Audit #11)
- **Commit:** `fix(core): persist attempt reservation before preparing worktree and staging`
- **Target:** `crates/polyclave-core/src/runs.rs:291`, `crates/polyclave-core/src/worktree.rs:22`, `crates/polyclave-core/src/recovery.rs:73`
- **Change:** Persist the Attempt row in SQLite before calling `worktree::prepare_attempt`, and teach startup recovery to recognize unreleased prepared Attempts and safely rebuild or skip residual directories without getting stuck on the same Attempt number.
- **Test:** Add a barrier test in `crates/polyclave-core/tests/recovery.rs` simulating host crash right after staging/worktree directory creation and before launch; verify recovery settles cleanly and retrying the Run succeeds without reusing an untrusted directory.

### Item 1.3: Tighten macOS Seatbelt read, temp-write and loopback boundaries (Audit #7)
- **Commit:** `fix(adapters): tighten macOS Seatbelt read, temp-write and loopback boundaries`
- **Target:** `crates/polyclave-adapters/src/claude.rs:42` (`MacosSeatbeltIsolation::build_profile`)
- **Change:**
  - Replace broad `/private/tmp` and `/tmp` write allowance with an Attempt-private temp directory (`TMPDIR` inside the Attempt boundary).
  - Explicitly deny reads to host state directories, GUI auth token files, keychain/credential files, and SQLite ledger paths.
  - Restrict loopback network outbound/inbound rules strictly to the turn's assigned MCP port instead of `localhost:*`.
- **Test:** Extend `crates/polyclave-adapters/tests/worker.rs` with boundary tests using sentinel credentials, a dummy main-API listener, and `/tmp`-placed host state verifying Claude Seatbelt blocks all three while still allowing worktree writes and the turn's MCP port.

### Item 1.4: Add host-side `commit_and_stage` MCP tool for sandboxed workers (Audit #1)
- **Commit:** `feat(adapters): add host-side commit_and_stage MCP tool for sandboxed workers`
- **Target:** `crates/polyclave-adapters/src/mcp.rs`, `crates/polyclave-core/src/runner.rs`, `crates/polyclave-adapters/src/codex.rs`
- **Change:**
  - Add a turn-authenticated host-side MCP tool `commit_and_stage` (taking commit message and optional path/trailer context) that runs `git add` / `git commit` inside the Attempt worktree on the trusted host side (enforcing `Run-Id: <run_id>` trailer and mandate path scope) and pushes `HEAD` to the Attempt staging repo.
  - Wire tool instructions/availability for sandboxed Workers (Codex `workspace-write`, Claude Seatbelt) so they do not need `.git` write access.
- **Test:** Add MCP + macOS Seatbelt/Codex sandbox tests verifying `commit_and_stage` succeeds when `.git` is read-only to the Worker, enforces `Run-Id` trailers, and never allows writes to the main repo or host mirror.

### Batch 1 Verification Gate
- `cargo fmt --all -- --check`
- `cargo clippy --locked --workspace --all-targets -- -D warnings`
- `cargo test --locked --workspace`
- `bash scripts/privacy-scan.sh` (`clean`)
- Append Batch 1 report to `HANDOFF.md` and stop for review/merge.

---

## Batch 2 (`fix/hardening-batch2-pr-gate`) — PR Gate Real Revision, Invariants & SCM Observer

Starts from updated `main` after Batch 1 is merged. Covers Codex audit items **#5, #6, #2, #9, #13, #14**.

### Item 2.1: Require R1 receipt provenance for auto-publish and green tests (Audit #5)
- **Commit:** `fix(hub): require R1 receipt provenance for auto-publish and green tests`
- **Target:** `crates/polyclave-core/src/runs.rs:471`, `bins/polyclaved/src/hub_api.rs:422`, `apps/polyclave-app/ui/src/views/evidence.ts:96`, `crates/polyclave-proto/src/dto.rs` (if provenance field is needed; sync `proto.ts`)
- **Change:** Preserve human accept (`result_source='user'`) for manual resolution, but require host R1 receipt provenance (accepted Attempt receipt + clean R1 check) for `verified_result_sha` in PR gate auto-publish and for turning the approval card's R1/tests rows green.
- **Test:** Add a negative test where a human accepts an un-receipted mirror tip (`result_source='user'`) via production path, then assert PR gate refuses it (no push/review/merge) and the UI card does not mark R1 or tests green.

### Item 2.2: Bind PR gate base and path scope strictly to Run and Ticket (Audit #6)
- **Commit:** `fix(hub): bind PR gate base and path scope strictly to Run and Ticket`
- **Target:** `bins/polyclaved/src/hub_api.rs:1007`, `bins/polyclaved/src/hub_api/tools.rs:113`, `crates/polyclave-hub/src/pr_gate.rs:656,790`
- **Change:** Remove caller-supplied overrides for `base_sha`, `base_branch`, and `touched_paths` in `run_pr_gate` tool / `prepare_pr_gate_run`; derive them strictly from persisted Run and Ticket mandate records, and verify the remote PR base branch matches the reviewed range before pushing/opening PR.
- **Test:** Add 3 negative tests (empty `base==head` range, mismatched remote base branch, caller attempting to widen `touched_paths`), all refused before any push.

### Item 2.3: Support PR gate revision attempts from verified SHA (Audit #2)
- **Commit:** `feat(core): support PR gate revision attempts from verified SHA`
- **Target:** `crates/polyclave-core/src/runs.rs:215`, `crates/polyclave-core/src/runner.rs:274`, `crates/polyclave-store/src/schema.sql:153`, `bins/polyclaved/src/hub_api.rs:434`, `crates/polyclave-hub/src/pr_gate.rs:920`
- **Change:** Add a dedicated PR-gate revision transition that creates a new Attempt branched from the prior verified SHA, preserving the Run's immutable original baseline and requiring an independent accepted receipt for the new Attempt. Wire `HostWorkerReviseExecutor` (`revise_pr` and `bounce_scope_violation`) to use it instead of `TurnMode::WorkerChat` on a terminal Run.
- **Test:** Add an end-to-end test in `bins/polyclaved/src/hub_api/tests.rs` (or `crates/polyclave-hub/src/pr_gate.rs`) using real `HostWorkerReviseExecutor` + `TurnRunner` chaining initial commit A -> Reviewer requests changes -> revision commit B with new receipt; verify missing B receipt never reuses A's SHA or prior approval.

### Item 2.4: Serialize auto-merge arming with pause and persist remote intent (Audit #9)
- **Commit:** `fix(hub): serialize auto-merge arming with pause and persist remote intent`
- **Target:** `crates/polyclave-hub/src/pr_gate.rs:1023,1042`, `crates/polyclave-store/src/stage2.rs:706`
- **Change:** Serialize per-PR arm/disarm operations, persist unconfirmed remote auto-merge intent before calling GitHub, never silently disarm when the remote response is lost, and reconcile pending intent during `pause_all` and startup recovery.
- **Test:** Add a barrier test interleaving `enable_auto_merge` and `pause_all`, plus a lost-response recovery test verifying startup reconciliation disables or tracks the real remote state.

### Item 2.5: Paginate check-runs by stable id and emit `test_results` changes (Audit #13)
- **Commit:** `fix(hub): paginate check-runs by stable id and emit test_results changes`
- **Target:** `crates/polyclave-hub/src/scm.rs:698`, `crates/polyclave-store/src/test_results.rs:15,53`, `crates/polyclave-hub/src/scm_observer.rs:167,251,322`, `apps/polyclave-app/ui/src/views/evidence.ts:96`
- **Change:**
  - Paginate all pages of GitHub check-runs (`per_page=100` + `page` loop) and track completeness so partial/incomplete check-run fetches never turn the card green.
  - Key `test_results` by stable check identifier (live-schema additive upgrade, `user_version` stays 10) so same-named checks from different apps/suites do not overwrite each other.
  - Emit `ChangeTopic::PrGate` / test-update `change_log` events whenever `test_results` rows change, handle PRs already merged on first poll, and deduplicate CI notifications by `(check_id, state_transition)` instead of once per SHA.
- **Test:** Add regressions for page-2 failure, same-name distinct-source checks, same-SHA `queued -> failed` transition, already-merged PR on first poll, and `change_log` emission on check updates.

### Item 2.6: Route SCM events to owning Hub and kick idle sessions (Audit #14)
- **Commit:** `fix(hub): route SCM events to owning Hub and kick idle sessions`
- **Target:** `bins/polyclaved/src/hub_api.rs:684,1145`, `crates/polyclave-hub/src/scm_observer.rs:101,358`
- **Change:** Look up the Run's owning `hub_session_id` dynamically per gate instead of pinning the first Hub at daemon startup, and trigger `Hub::kick` after `hub_enqueue_external` succeeds so idle Hub sessions wake up.
- **Test:** Add a test starting the daemon on an empty DB, then registering two projects/Hubs and verifying SCM events route only to the owning Hub, wake it, and preserve `source='host'` + `scm_observation` metadata.

### Batch 2 Verification Gate
- `cargo fmt --all -- --check`
- `cargo clippy --locked --workspace --all-targets -- -D warnings`
- `cargo test --locked --workspace`
- `git diff --exit-code -- apps/polyclave-app/ui/src/generated/proto.ts` + UI `tsc --noEmit`, `npm run build`, `npm run test:browser` (if `proto.ts`/`evidence.ts` changed)
- `bash scripts/privacy-scan.sh` (`clean`)
- Append Batch 2 report to `HANDOFF.md` and stop for review/merge.

---

## Batch 3 (`fix/hardening-batch3-phone-approval`) — Phone Pairing, Exact-SHA Human Approval & Restart Reconnect

Starts from updated `main` after Batch 2 is merged. Covers Codex audit items **#8, #3, #4, #10**.

### Item 3.1: Require Tailscale identity and persisted binding for remote device auth (Audit #8)
- **Commit:** `fix(auth): require Tailscale identity and persisted binding for remote device auth`
- **Target:** `bins/polyclaved/src/routes.rs:308`, `bins/polyclaved/src/auth.rs:68`, `crates/polyclave-store/src/stage3.rs:295`
- **Change:** Remove `x-agentmesh-node` fallback from phone pairing and HTTP device auth; require a non-null matching persisted `peer_identity` on remote/non-loopback calls so locally-paired unbound tokens cannot pass remote Tailscale checks.
- **Test:** Add negative tests for missing Tailscale identity, `x-agentmesh-node`-only header, and locally-paired unbound token sent through remote origin.

### Item 3.2: Pair over configured Tailscale HTTPS origin and exchange ticket in page (Audit #3)
- **Commit:** `feat(ui): pair over configured Tailscale HTTPS origin and exchange ticket in page`
- **Target:** `bins/polyclaved/src/hub_api/http.rs:706`, `apps/polyclave-app/ui/src/api/sse.ts:4,16`, `apps/polyclave-app/ui/src/views/flows.ts:142` (and Tauri/pairing config if needed)
- **Change:** Generate pairing QR URLs pointing at the configured Tailscale HTTPS origin (`https://<host>/#pair=<secret>` or equivalent query/fragment), allow same-origin `https://` in `sse.ts` (`bootstrap` / `request` / `follow`), implement automatic in-page `/api/pairing/exchange` on load + immediate URL secret stripping (`history.replaceState`) + device token storage.
- **Test:** Add a 375 px browser regression starting from the scanned pairing URL through exchange, replay rejection, device API/SSE access, revocation, and reconnect.

### Item 3.3: Gate merge on explicit human approval of exact verified SHA with full diff (Audit #4)
- **Commit:** `feat(hub): gate merge on explicit human approval of exact verified SHA with full diff`
- **Target:** `crates/polyclave-hub/src/pr_gate.rs:1011`, `bins/polyclaved/src/routes.rs:157`, `crates/polyclave-core/src/mirror.rs:400`, `crates/polyclave-proto/src/dto.rs`, `apps/polyclave-app/ui/src/views/{inspector.ts,evidence.ts}`, `DESIGN.md`
- **Change:**
  - Add an explicit `awaiting_human_approval` gate state after heterogeneous Reviewer approval so `run_pr_gate` never enables auto-merge before human sign-off.
  - Add an exact-SHA human approval endpoint (`seen_sha` / `expected_sha` checked atomically against the gate's current verified `head_sha`) and expose the full mirror diff for that exact SHA (`base_sha..head_sha`, bounded/paginated safely without hiding truncation).
  - Update the approval card UI (`inspector.ts`, `evidence.ts`, `DESIGN.md`, `proto.ts`) to display the exact SHA's diff, per-SHA CI/test status, and open/not-done items, with an Accept/Approve action bound to that SHA.
- **Test & Hand Check:** Add backend + 375 px browser tests that POST approval for B, reject stale SHA A when head moved to B, and verify zero merge/`enableAutoMerge` side-effects occur before B is approved. Capture `handcheck/hardening-batch3/` screenshots (1440×900 & 375×812) and `measurements.md`.

### Item 3.4: Recover auth token and clear live stream state after daemon restart (Audit #10)
- **Commit:** `fix(ui): recover auth token and clear live stream state after daemon restart`
- **Target:** `bins/polyclaved/src/main.rs:102`, `apps/polyclave-app/ui/src/app.ts:121,228`, `apps/polyclave-app/ui/src/api/sse.ts:30`
- **Change:** When SSE/API returns `401` after daemon restart token rotation, re-fetch `local_connection` in the Tauri shell or surface a clear reconnect prompt in browser mode; on `resync_required` / reconnect recovery, re-fetch the full snapshot and clear stale `S.live` transient stream state.
- **Test:** Add a browser regression simulating stream disconnect -> daemon token rotation -> recovery, verifying the page re-snapshots the exact Run without stale `S.live` artifacts.

### Batch 3 Verification Gate
- `cargo fmt --all -- --check`
- `cargo clippy --locked --workspace --all-targets -- -D warnings`
- `cargo test --locked --workspace`
- `git diff --exit-code -- apps/polyclave-app/ui/src/generated/proto.ts`
- `cd apps/polyclave-app/ui && npx tsc --noEmit && npm run build && POLYCLAVE_BROWSER_CHANNEL=chrome npm run test:browser`
- `bash scripts/privacy-scan.sh` (`clean`)
- Append Batch 3 report + `handcheck/hardening-batch3/` screenshots + `measurements.md` to `HANDOFF.md` and stop for final review/merge.
