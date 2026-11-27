// Type contract exported by polyclave-proto.
export type JsonValue = string | number | boolean | null | JsonValue[] | { [key: string]: JsonValue };

export type SessionRole = "hub" | "manager" | "worker";

export type Vendor = "codex" | "claude";

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

export type TurnDto = { id: number, session_id: string, run_id: string | null, attempt_id: number | null, parent_turn_id: number | null, mode: TurnMode, status: TurnStatus, phase: TurnPhase, exit_code: number | null, end_reason: EndReason | null, writer_check: WriterCheck | null, started_at: string, ended_at: string | null, };

export type MemoryItemDto = { id: string, scope: MemoryScope, repo_cwd: string | null, kind: MemoryKind, title: string, content: string, tags: Array<string>, evidence: JsonValue | null, status: MemoryStatus, supersedes_id: string | null, created_by: string, source_run_id: string | null, source_turn_id: number | null, created_at: string, updated_at: string, };

export type ProviderProfileDto = { id: string, display_name: string, kind: ProviderKind, models: Array<string>, configured: boolean, created_at: string, };

export type ChangeTopic = "message" | "turn" | "ticket" | "run" | "attempt" | "memory" | "hub_receipt" | "inbox";

export type SseEnvelope = { seq: number, topic: ChangeTopic, entity_id: string, payload: JsonValue, created_at: string, };

export type AgentEvent = { "kind": "session", id: string, } | { "kind": "text_delta", text: string, } | { "kind": "reasoning_delta", text: string, } | { "kind": "finished", success: boolean, };

export type ApiError = { code: string, message: string, };
