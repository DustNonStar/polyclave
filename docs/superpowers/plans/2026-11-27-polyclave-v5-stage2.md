# Polyclave v5 Stage 2 Implementation Plan

> **Scope:** Multi-model blind-review `council`, GitHub `pr_gate` + background `SCM Observer` (direct Rust `reqwest` GitHub REST/GraphQL API), `agy` (Antigravity) & `dsh` (DeepSeek Harness) adapters, and Stage 2 desktop/mobile UI.
>
> **Spec references:**
> - `docs/superpowers/specs/2026-11-26-polyclave-v5-rust-design.md` (§1.2, §5)
> - `docs/superpowers/specs/2026-11-25-agent-console-design-v4.md` (§9, §10)
>
> **Owner decisions (2026-11-27):**
> 1. Execute in 3 batches: **2A** (`proto` + `store` + `agy`/`dsh` adapters) → **2B** (`council` + `pr_gate` + `SCM Observer` + daemon routes) → **2C** (Stage 2 UI, owned by Local Lead).
> 2. `pr_gate` and `SCM Observer` talk to GitHub directly via Rust `reqwest` REST/GraphQL API with separate Bot and Reviewer tokens stored in the OS Keychain (no `gh` CLI dependency; fully testable via a local `axum` mock server).
> 3. Keep `legacy-node/` intact until remaining Phase 1 real-CLI probes pass and deletion is explicitly approved.

---

## Standing Invariants

1. **R1 & Attempt isolation remain untouched:** Only R1-verified authoritative mirror branches (`refs/polyclave/runs/<runId>/final`) may be pushed to GitHub. Worker write roots remain strictly confined to `(worktree, staging)`.
2. **True blind council:** Vendor/model identity is stripped in Rust code before presenting proposals (`方案甲` / `方案乙` / `方案丙`) and revealed only after decision. `direction`-level councils never auto-select or auto-start `dev`.
3. **Heterogeneous disposable reviewer:** Every PR review runs in a fresh disposable worktree checked out at the exact `head_sha` using a different vendor/model than the author worker (verified from host launch records, never self-reported), and the worktree is deleted immediately after review.
4. **Crash-safe auto-merge & global Pause:** Always persist `auto_merge_armed` in SQLite *before* calling GitHub's enable-auto-merge mutation (`--match-head-commit` / `expectedHeadOid`). Global Pause revokes all armed auto-merges, refuses new merges, and blocks new dispatches.
5. **No self-trigger loops:** Host-submitted reviews carry `[agent-reviewer · <vendor>/<model> · <sha>]` and are ignored by `SCM Observer`. Observer events flow into the Hub inbox (`change_log` + triage queue), never directly back to a worker.
6. **Zero external UI requests:** The UI talks exclusively to loopback `polyclaved`.

---

## Batch 2A: Protocol Contracts, Store Schema, and `agy` / `dsh` Adapters

**Owned directories:**
- `crates/polyclave-proto/`
- `crates/polyclave-store/`
- `crates/polyclave-adapters/`
- `crates/polyclave-hub/src/cli_provider.rs`
- `apps/polyclave-app/ui/src/generated/proto.ts` (regenerated via `cargo test -p polyclave-proto`)

### Item 2A.1 — Extend `polyclave-proto` for Stage 2 & regenerate `proto.ts`
- **Commit:** `feat(proto): add Stage 2 vendors, council, pr_gate, and pause contracts`
- **Changes:**
  - Extend `Vendor` enum: `Codex`, `Claude`, `Agy`, `Dsh`.
  - Extend `ChangeTopic`: add `Council`, `PrGate`, `Pause`.
  - Add Stage 2 DTOs with `#[derive(TS)]` and export into `apps/polyclave-app/ui/src/generated/proto.ts`:
    - `CouncilLevel` (`Direction`, `Detail`), `PlanningPreference` (`Thorough`, `Balanced`, `Conservative`), `CouncilStatus` (`Running`, `AwaitingUser`, `Decided`, `Cancelled`, `Failed`).
    - `ProposalPayload` (`summary`, `approach`, `touched_paths: Vec<String>`, `public_interface_changes: bool`, `new_dependencies: Vec<String>`, `security_sensitive: bool`, `beyond_request: bool`, `open_questions: Vec<String>`, `pros: Vec<String>`, `cons: Vec<String>`).
    - `CouncilProposalDto` (blind view: `label` e.g. `"方案甲"`, `payload`, `vendor: Option<Vendor>`, `model: Option<String>` — `vendor`/`model` populated only when `status == Decided`).
    - `CouncilDto` (`id`, `ticket_id`, `repo_cwd`, `level`, `status`, `prompt`, `escalation_reasons: Vec<String>`, `chosen_label: Option<String>`, `decision_note: Option<String>`, `proposals: Vec<CouncilProposalDto>`, `created_at`, `decided_at`).
    - `PrGateStatus` (`Pending`, `ScopeViolation`, `Reviewing`, `ReviseRequested`, `Approved`, `AutoMergeArmed`, `Merged`, `Escalated`, `Paused`, `Failed`).
    - `ReviewVerdict` (`Approve`, `Revise`), `PrReviewDto` (`id`, `pr_gate_id`, `reviewer_vendor`, `reviewer_model`, `reviewed_sha`, `verdict`, `comments`, `github_review_id`, `created_at`).
    - `PrGateDto` (`id`, `run_id`, `ticket_id`, `repo_slug`, `branch`, `base_sha`, `head_sha`, `pr_number`, `pr_url`, `status`, `scope_violations: Vec<String>`, `revise_rounds`, `auto_merge_armed`, `reviews: Vec<PrReviewDto>`, `updated_at`).
    - `PauseStateDto` (`paused: bool`, `reason: Option<String>`, `updated_at: String`).
  - Keep `TurnDto` compile-fail doctest passing (no secret fields exposed).

### Item 2A.2 — Add Stage 2 durable tables & single-writer methods in `polyclave-store`
- **Commit:** `feat(store): add council, pr_gate, scm_cursor, and global pause tables`
- **Changes:**
  - Add additive SQLite migrations (`CREATE TABLE IF NOT EXISTS` in `crates/polyclave-store/src/writer.rs` / new `stage2.rs`):
    - `councils` (`id`, `ticket_id`, `repo_cwd`, `level`, `status`, `prompt`, `preference`, `escalation_reasons_json`, `chosen_label`, `decision_note`, `created_at`, `decided_at`)
    - `council_proposals` (`id`, `council_id`, `label`, `vendor`, `model`, `payload_json`, `input_hash`, `created_at`, UNIQUE `(council_id, label)`)
    - `council_checkpoints` (`council_id`, `step_key`, `input_hash`, `output_json`, `created_at`, PRIMARY KEY `(council_id, step_key)`)
    - `pr_gates` (`id`, `run_id`, `ticket_id`, `repo_slug`, `branch`, `base_sha`, `head_sha`, `pr_number`, `pr_url`, `status`, `touched_paths_json`, `scope_violations_json`, `revise_rounds`, `auto_merge_armed`, `created_at`, `updated_at`)
    - `pr_reviews` (`id`, `pr_gate_id`, `reviewer_vendor`, `reviewer_model`, `reviewed_sha`, `verdict`, `comments`, `github_review_id`, `created_at`)
    - `scm_observer_cursors` (`repo_slug`, `pr_number`, `last_event_etag`, `last_check_sha`, `last_comment_id`, `updated_at`, PRIMARY KEY `(repo_slug, pr_number)`)
    - `host_pause_state` (`singleton INTEGER PRIMARY KEY CHECK(singleton = 1)`, `paused INTEGER NOT NULL`, `reason TEXT`, `updated_at TEXT NOT NULL`)
  - Implement atomic store operations that also append to `change_log` (preserving SSE `Last-Event-ID` ordering) and never leak credentials.
  - Add unit tests in `crates/polyclave-store/tests/store.rs`: council checkpoint idempotency, blind-vs-decided proposal projection, `pr_gate` in-flight auto-merge ledger + pause flag persistence.

### Item 2A.3 — Implement `agy` (Antigravity) & `dsh` (DeepSeek Harness) adapters
- **Commit:** `feat(adapters): add Antigravity (agy) and DeepSeek Harness (dsh) worker adapters`
- **Changes:**
  - Add `crates/polyclave-adapters/src/agy.rs`:
    - `agy_args`: `-p`, `--output-format`, `stream-json`, `--sandbox`, `--mode`, `auto`, repeatable `--add-dir <worktree>` / `--add-dir <staging>`, `--settings <tmp_mcp_config>`, optional `--resume <thread>`, optional `--model <model>`.
    - `agy_worker`: prepares isolated home under `<state_dir>/isolated-homes/agy`, writes per-turn MCP config under `<state_dir>/tmp/mcp-turn-<id>.json`, passes `POLYCLAVE_MCP_TOKEN` and `base_environment()`.
    - `parse_agy`: parses `stream-json` NDJSON into `AgentEvent` (`Session`, `TextDelta`, `ReasoningDelta`, `Finished`).
  - Add `crates/polyclave-adapters/src/dsh.rs`:
    - `dsh_args`: `--profile`, `headless`, `--json`, `--permission-mode`, `workspace-write`, `--approval-policy`, `never`, repeatable `--add-dir <worktree>` / `--add-dir <staging>`, optional `--session-id <thread>`, `-` (read task from stdin).
    - `dsh_worker`: prepares isolated `DSH_HOME` under `<state_dir>/isolated-homes/dsh` (linking credentials/skills only, never mutating user global `$DSH_HOME`), passes `POLYCLAVE_MCP_TOKEN` and `base_environment()`.
    - `parse_dsh`: parses `--json` NDJSON run events into `AgentEvent`.
  - Update `crates/polyclave-adapters/src/lib.rs` (`prepare_isolated_home_from` for `Vendor::Agy` and `Vendor::Dsh`) and `crates/polyclave-hub/src/cli_provider.rs` so `Agy`/`Dsh` can also be used cleanly where configured.
  - Add offline unit tests in `crates/polyclave-adapters/tests/worker.rs` covering args, write-root overlap checks, NDJSON parsing, and isolated home linking for both `agy` and `dsh`.

**Batch 2A Verification:**
- `cargo fmt --all -- --check`
- `cargo clippy --locked --workspace --all-targets -- -D warnings`
- `cargo test --locked --workspace`
- `git diff --exit-code -- apps/polyclave-app/ui/src/generated/proto.ts`

---

## Batch 2B: `council`, `pr_gate`, `SCM Observer`, and Daemon API

**Owned directories:**
- `crates/polyclave-core/` (scope check & disposable reviewer worktree helpers)
- `crates/polyclave-hub/` (`council.rs`, `pr_gate.rs`, `scm.rs`, `scm_observer.rs`)
- `bins/polyclaved/` (routes, Hub tools, observer background task, pause enforcement)

### Item 2B.1 — Multi-model blind-review `council` engine
- **Commit:** `feat(hub): implement blind multi-model council with checkpoint replay and escalation`
- **Changes:**
  - Add `crates/polyclave-hub/src/council.rs`:
    - `start_council`: validates `level` (`direction` default), checks `council_checkpoints` by input hash, fans out in parallel to enabled `manager` providers/sessions (strictly isolated — no peer visibility).
    - Parses structured `ProposalPayload` with up to 2 automatic schema-correction retries; fails/escalates cleanly on 3rd failure.
    - Host-side anonymization: assigns `方案甲` / `方案乙` / `方案丙` (shuffled deterministically by council ID), strips vendor/model strings from display summary.
    - Escalation detector (`evaluate_escalations`): fires on `>3x` `touched_paths` count delta across proposals, `security_sensitive` or `.github/` in `touched_paths`, `public_interface_changes`, non-empty `open_questions`, `beyond_request`, or `level == Direction`.
    - `detail` without escalations selects per `PlanningPreference`; `direction` or any escalation moves to `AwaitingUser` and posts summary to Hub inbox.
    - `resolve_council`: records user's chosen label + note, transitions to `Decided`, reveals vendor/model attribution.
  - Offline unit tests with mock providers covering blind stripping, checkpoint replay across restart, escalation rules, and `direction` never auto-starting `dev`.

### Item 2B.2 — 3-layer `touched_paths` scope check & disposable reviewer worktree
- **Commit:** `feat(core): add touched_paths scope check and disposable reviewer worktree`
- **Changes:**
  - Add `crates/polyclave-core/src/scope.rs`: computes `git diff --name-only <base_sha>..<head_sha>` against authoritative mirror and compares against glob/prefix `touched_paths` contract; returns structured `ScopeCheckResult { allowed, violations }`.
  - Add `WorktreeManager::with_disposable_review_worktree(mirror, head_sha, f)` in `crates/polyclave-core/src/worktree.rs`: creates a detached worktree at exact `head_sha`, guarantees removal on drop/exit (even on error or panic), so accidental reviewer writes can never pollute the PR branch.
  - Unit tests in `crates/polyclave-core/tests/git.rs`.

### Item 2B.3 — `ScmClient` (Rust `reqwest` GitHub REST/GraphQL), `pr_gate`, Pause, and `SCM Observer`
- **Commit:** `feat(hub): add reqwest GitHub ScmClient, pr_gate, crash-safe auto-merge, and SCM Observer`
- **Changes:**
  - Add `crates/polyclave-hub/src/scm.rs`:
    - `ScmClient` trait + `GitHubScmClient` built on `reqwest` (configured base URL so tests point at local `axum` mock server; separate `bot_token` and `reviewer_token` resolved from OS Keychain, never logged).
    - Methods: `preflight_repo`, `open_or_find_pr`, `submit_review(commit_id, body, event)`, `enable_auto_merge(pr_node_id, expected_head_oid)`, `disable_auto_merge(pr_node_id)`, `get_pr_state`, `list_pr_checks_and_comments`.
    - Enforces bot account != reviewer account during `preflight_repo`.
  - Add `crates/polyclave-hub/src/pr_gate.rs`:
    - Runs preflight → 3-layer `touched_paths` check (1 automatic worker bounce, escalate on 2nd violation) → pushes R1-verified mirror branch → opens PR → spawns heterogeneous reviewer(s) in disposable worktree at `head_sha` (1 normal, 2 high-risk; enforces `reviewer_vendor != author_vendor || reviewer_model != author_model` from host launch records) → submits GitHub review with explicit `commit_id = reviewed_sha` and `[agent-reviewer · <vendor>/<model> · <sha>]` marker → handles up to 3 `Revise` rounds → writes `auto_merge_armed = true` to SQLite *before* calling `enable_auto_merge(expected_head_oid)`.
    - Global Pause (`pause_all` / `resume_all`): sets `host_pause_state.paused = true`, iterates all `auto_merge_armed` PRs in SQLite and calls `disable_auto_merge`, blocks new dispatches and merges.
  - Add `crates/polyclave-hub/src/scm_observer.rs`:
    - Background Tokio task polling active `pr_gates` on a configurable interval, ignoring reviews carrying `[agent-reviewer · …]`, updating `scm_observer_cursors`, and pushing CI failure / review comment / merge conflict / merged events into the Hub inbox (`change_log` + triage queue).
  - Offline integration tests using a local `axum` fake GitHub REST+GraphQL server verifying: preflight refusal when bot == reviewer, `commit_id` pinning, crash-safe ledger-before-auto-merge ordering, Pause revoking armed auto-merges, and `[agent-reviewer · …]` self-loop filtering.

### Item 2B.4 — Wire Stage 2 routes, Hub tools, and observer lifecycle in `polyclaved`
- **Commit:** `feat(daemon): expose council, pr_gate, pause endpoints and start SCM Observer`
- **Changes:**
  - Expose authenticated loopback endpoints in `bins/polyclaved/src/routes.rs` & `bins/polyclaved/src/hub_api/`:
    - `GET /api/councils`, `POST /api/councils`, `POST /api/councils/:id/resolve`
    - `GET /api/pr-gates`, `POST /api/pr-gates/:id/run`, `GET /api/pr-gates/:id`
    - `GET /api/pause`, `POST /api/pause`, `POST /api/resume`
  - Register Hub tools (`start_council`, `run_pr_gate`) and respect global Pause in dispatch/run admission (`409 Conflict` when paused).
  - Start/stop `SCM Observer` cleanly with `polyclaved` shutdown.
  - Add HTTP/SSE route tests in `bins/polyclaved/src/hub_api/tests.rs`.

**Batch 2B Verification:**
- `cargo fmt --all -- --check`
- `cargo clippy --locked --workspace --all-targets -- -D warnings`
- `cargo test --locked --workspace`

---

## Batch 2C: Stage 2 Desktop & Mobile UI (Owned by Local Lead)

**Owned directories:**
- `apps/polyclave-app/ui/`
- `DESIGN.md`

### Item 2C.1 — Council Blind-Review UI, PR Gate & Global Pause, and `agy`/`dsh` Picker
- **Commit:** `feat(ui): add Council blind-review cards, PR Gate panel, global Pause, and agy/dsh pickers`
- **Changes:**
  - **Council view**: side-by-side anonymous proposal cards (`方案甲 / 乙 / 丙`), risk & escalation pills (`touched_paths` count, `security_sensitive`, `public_interface_changes`, `new_dependencies`, `open_questions`), one-click decision form for `direction` / escalated councils, post-decision vendor/model badge reveal.
  - **PR Gate & SCM Observer view**: preflight checklist, 3-layer `touched_paths` diff summary, heterogeneous reviewer verdict & pinned `head_sha`, in-flight auto-merge badge, and prominent global **Pause / Resume** control in the top status bar (accessible on both 1440px desktop and 375px mobile, ≥44px touch targets).
  - **Provider / Worker pickers & Settings**: add `Antigravity (agy)` and `DeepSeek Harness (dsh)` options, plus GitHub Bot / Reviewer token Keychain configuration rows (masked status only, never plaintext).
  - Keep zero `backdrop-filter` GPU thrash, zero external requests, and update `DESIGN.md` in the same commit.

**Batch 2C Verification:**
- `npm run typecheck`
- `npm run build`
- `POLYCLAVE_BROWSER_CHANNEL=chrome npm run test:browser` (`1440×900` & `375×812`, zero overflow, zero external requests)
- `bash scripts/privacy-scan.sh` (`clean`)
