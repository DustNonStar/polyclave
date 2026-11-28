// Type contract exported by polyclave-proto.
export type JsonValue = string | number | boolean | null | JsonValue[] | { [key: string]: JsonValue };

export type SessionRole = "hub" | "manager" | "worker";

export type Vendor = "codex" | "claude" | "agy" | "dsh";

export type MessageSource = "user" | "agent" | "host";

export type ClientKind = "desktop" | "web" | "ios" | "android" | "cli";

export type TicketSource = "explicit_user" | "inferred_event";

export type TicketStatus = "proposed" | "active" | "blocked_on_user" | "done" | "cancelled";

export type RunStatus = "pending" | "running" | "done" | "interrupted" | "failed" | "needs_review";

export type ResultSource = "agent" | "user";

export type AttemptStatus = "prepared" | "running" | "completed" | "cancelled" | "superseded" | "interrupted" | "failed";

export type TurnMode = "hub" | "manager" | "worker_chat" | "start" | "correction" | "retry" | "consult";

export type TurnStatus = "running" | "completed" | "cancelled" | "interrupted" | "failed";

export type TurnPhase = "prepared" | "authorized";

export type EndReason = "exit" | "cancel" | "host_lost" | "never_started" | "dispatch_closed";

export type WriterCheck = "excluded" | "possible";

export type MemoryScope = "global" | "project" | "run";

export type MemoryKind = "decision" | "constraint" | "progress" | "codebase_fact" | "pitfall" | "preference" | "receipt";

export type MemoryStatus = "candidate" | "verified" | "superseded" | "archived";

export type ProviderKind = "openai_compat" | "anthropic_compat" | "codex_cli" | "claude_cli";

export type SessionDto = { id: string, name: string, role: SessionRole, vendor: Vendor | null, repo_cwd: string, profile_id: string, provider_model: string, rolling_summary: string, agent_thread_id: string | null, created_at: string, };

export type MessageDto = { id: number, session_id: string, turn_id: number | null, source: MessageSource, client_kind: ClientKind | null, provider_label: string | null, reasoning: string | null, text: string, metadata_json: JsonValue | null, queued: boolean, created_at: string, };

export type TicketDto = { id: string, hub_session_id: string, repo_cwd: string, title: string, mandate_json: JsonValue, source_kind: TicketSource, basis_msg_id: number | null, max_runs: number, runs_used: number, status: TicketStatus, created_at: string, updated_at: string, };

export type RunDto = { id: string, ticket_id: string, hub_session_id: string, worker_session_id: string, request_id: string, repo_cwd: string, vendor: Vendor, model: string | null, instructions: string, context_refs_json: JsonValue | null, base_branch: string, base_sha: string, status: RunStatus, current_attempt: number, max_attempts: number, accepted_attempt_id: number | null, detail: string | null, result_sha: string | null, observed_sha: string | null, result_source: ResultSource | null, resolution: string | null, created_at: string, updated_at: string, };

export type AttemptDto = { id: number, run_id: string, attempt_no: number, branch: string, worktree_path: string, agent_thread_id: string | null, corrections: number, status: AttemptStatus, created_at: string, ended_at: string | null, };

export type RunEvidenceDto = { attempt_no: number, receipt_sha: string | null, worker_claim: string | null, test_sha: string | null, receipt_tests: string | null, receipt_tests_status: TestStatus | null, receipt_not_done: string | null, test_results: Array<TestResultDto>, };

export type TestResultDto = { sha: string, source: TestResultSource, status: TestStatus, 
/**
 * Check-run name for `ci`; empty for a claim.
 */
name: string, summary: string, recorded_at: string, };

export type TestResultSource = "worker_claim" | "host_run" | "ci";

export type TestStatus = "passed" | "failed" | "pending";

export type TurnDto = { id: number, session_id: string, run_id: string | null, attempt_id: number | null, parent_turn_id: number | null, mode: TurnMode, status: TurnStatus, phase: TurnPhase, exit_code: number | null, end_reason: EndReason | null, writer_check: WriterCheck | null, started_at: string, ended_at: string | null, };

export type MemoryItemDto = { id: string, scope: MemoryScope, repo_cwd: string | null, kind: MemoryKind, title: string, content: string, tags: Array<string>, evidence: JsonValue | null, status: MemoryStatus, supersedes_id: string | null, created_by: string, source_run_id: string | null, source_turn_id: number | null, created_at: string, updated_at: string, };

export type ProviderProfileDto = { id: string, display_name: string, kind: ProviderKind, models: Array<string>, configured: boolean, created_at: string, };

export type CouncilLevel = "direction" | "detail";

export type PlanningPreference = "thorough" | "balanced" | "conservative";

export type CouncilStatus = "running" | "awaiting_user" | "decided" | "cancelled" | "failed";

export type ProposalPayload = { summary: string, approach: string, touched_paths: Array<string>, public_interface_changes: boolean, new_dependencies: Array<string>, security_sensitive: boolean, beyond_request: boolean, open_questions: Array<string>, pros: Array<string>, cons: Array<string>, };

export type CouncilProposalDto = { label: string, payload: ProposalPayload, vendor: Vendor | null, model: string | null, };

export type CouncilDto = { id: string, ticket_id: string | null, repo_cwd: string, level: CouncilLevel, status: CouncilStatus, prompt: string, escalation_reasons: Array<string>, chosen_label: string | null, decision_note: string | null, proposals: Array<CouncilProposalDto>, created_at: string, decided_at: string | null, };

export type PrGateStatus = "pending" | "scope_violation" | "reviewing" | "revise_requested" | "approved" | "auto_merge_armed" | "merged" | "escalated" | "paused" | "failed";

export type ReviewVerdict = "approve" | "revise";

export type PrReviewDto = { id: string, pr_gate_id: string, reviewer_vendor: Vendor, reviewer_model: string, reviewed_sha: string, verdict: ReviewVerdict, comments: string, github_review_id: number | null, created_at: string, };

export type PrGateDto = { id: string, run_id: string, ticket_id: string, repo_slug: string, branch: string, base_sha: string, head_sha: string, pr_number: number | null, pr_url: string | null, status: PrGateStatus, scope_violations: Array<string>, revise_rounds: number, auto_merge_armed: boolean, reviews: Array<PrReviewDto>, updated_at: string, };

export type PauseStateDto = { paused: boolean, reason: string | null, updated_at: string, };

export type PairingTicketDto = { id: string, client_kind: ClientKind, device_label: string, expires_at: string, used_at: string | null, created_at: string, };

export type PairedDeviceDto = { id: string, label: string, client_kind: ClientKind, peer_identity: string | null, last_seen_at: string | null, revoked_at: string | null, created_at: string, };

export type MeshNodeStatus = "online" | "offline" | "revoked";

export type MeshNodeDto = { id: string, name: string, endpoint: string, os_family: string, allowed_repos: Array<string>, allowed_actions: Array<string>, status: MeshNodeStatus, last_heartbeat_at: string | null, created_at: string, };

export type MeshNodeLimitDto = { node_id: string, max_concurrent_runs: number, active_runs: number, };

export type NotificationChannelKind = "ntfy" | "unified_push";

export type NotificationChannelDto = { id: string, kind: NotificationChannelKind, label: string, endpoint: string, topic: string, enabled: boolean, created_at: string, };

export type CuaLockDto = { locked: boolean, holder_run_id: string | null, target_window: string | null, idle_threshold_secs: number, acquired_at: string | null, reclaimed_at: string | null, };

export type EvolutionKind = "routing" | "memory_rule" | "role_brief";

export type EvolutionStatus = "proposed" | "applied" | "rolled_back" | "rejected";

export type EvolutionSuggestionDto = { id: string, kind: EvolutionKind, title: string, rationale: string, payload_json: JsonValue, status: EvolutionStatus, supersedes_id: string | null, created_at: string, updated_at: string, };

export type LocalSkillDto = { id: string, name: string, version: number, supersedes_id: string | null, description: string, body_md: string, enabled: boolean, synced_vendors: Array<Vendor>, created_at: string, };

export type CommunityDraftDto = { opt_in: boolean, title: string, redacted_markdown: string, stripped_items: number, generated_at: string, };

export type ChangeTopic = "message" | "turn" | "ticket" | "run" | "attempt" | "memory" | "hub_receipt" | "inbox" | "council" | "pr_gate" | "pause" | "device" | "mesh_node" | "notification" | "cua" | "evolution" | "skill";

export type SseEnvelope = { seq: number, topic: ChangeTopic, entity_id: string, payload: JsonValue, created_at: string, };

export type AgentEvent = { "kind": "session", id: string, } | { "kind": "text_delta", text: string, } | { "kind": "reasoning_delta", text: string, } | { "kind": "finished", success: boolean, };

export type ApiError = { code: string, message: string, };
