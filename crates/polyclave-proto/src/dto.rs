use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum SessionRole {
    Hub,
    Manager,
    Worker,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum Vendor {
    Codex,
    Claude,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum MessageSource {
    User,
    Agent,
    Host,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ClientKind {
    Desktop,
    Web,
    Ios,
    Android,
    Cli,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum TicketSource {
    ExplicitUser,
    InferredEvent,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum TicketStatus {
    Proposed,
    Active,
    BlockedOnUser,
    Done,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Pending,
    Running,
    Done,
    Interrupted,
    Failed,
    NeedsReview,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ResultSource {
    Agent,
    User,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum AttemptStatus {
    Prepared,
    Running,
    Completed,
    Cancelled,
    Superseded,
    Interrupted,
    Failed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum TurnMode {
    Hub,
    Manager,
    WorkerChat,
    Start,
    Correction,
    Retry,
    Consult,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum TurnStatus {
    Running,
    Completed,
    Cancelled,
    Interrupted,
    Failed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum TurnPhase {
    Prepared,
    Authorized,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    Exit,
    Cancel,
    HostLost,
    NeverStarted,
    DispatchClosed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum WriterCheck {
    Excluded,
    Possible,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScope {
    Global,
    Project,
    Run,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    Decision,
    Constraint,
    Progress,
    CodebaseFact,
    Pitfall,
    Preference,
    Receipt,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum MemoryStatus {
    Candidate,
    Verified,
    Superseded,
    Archived,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    OpenaiCompat,
    AnthropicCompat,
    CodexCli,
    ClaudeCli,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
pub struct SessionDto {
    pub id: String,
    pub name: String,
    pub role: SessionRole,
    pub vendor: Option<Vendor>,
    pub repo_cwd: String,
    pub profile_id: String,
    pub provider_model: String,
    pub rolling_summary: String,
    pub agent_thread_id: Option<String>,
    pub created_at: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
pub struct MessageDto {
    #[ts(type = "number")]
    pub id: i64,
    pub session_id: String,
    #[ts(type = "number | null")]
    pub turn_id: Option<i64>,
    pub source: MessageSource,
    pub client_kind: Option<ClientKind>,
    pub provider_label: Option<String>,
    pub reasoning: Option<String>,
    pub text: String,
    pub metadata_json: Option<Value>,
    pub queued: bool,
    pub created_at: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
pub struct TicketDto {
    pub id: String,
    pub hub_session_id: String,
    pub repo_cwd: String,
    pub title: String,
    pub mandate_json: Value,
    pub source_kind: TicketSource,
    #[ts(type = "number | null")]
    pub basis_msg_id: Option<i64>,
    pub max_runs: u32,
    pub runs_used: u32,
    pub status: TicketStatus,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
pub struct RunDto {
    pub id: String,
    pub ticket_id: String,
    pub hub_session_id: String,
    pub worker_session_id: String,
    pub request_id: String,
    pub repo_cwd: String,
    pub vendor: Vendor,
    pub model: Option<String>,
    pub instructions: String,
    pub context_refs_json: Option<Value>,
    pub base_branch: String,
    pub base_sha: String,
    pub status: RunStatus,
    pub current_attempt: u32,
    pub max_attempts: u32,
    #[ts(type = "number | null")]
    pub accepted_attempt_id: Option<i64>,
    pub detail: Option<String>,
    pub result_sha: Option<String>,
    pub observed_sha: Option<String>,
    pub result_source: Option<ResultSource>,
    pub resolution: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
pub struct AttemptDto {
    #[ts(type = "number")]
    pub id: i64,
    pub run_id: String,
    pub attempt_no: u32,
    pub branch: String,
    pub worktree_path: String,
    pub agent_thread_id: Option<String>,
    pub corrections: u32,
    pub status: AttemptStatus,
    pub created_at: String,
    pub ended_at: Option<String>,
}

/// Public projection. The host-only authorization secret is not part of this type.
///
/// ```compile_fail
/// use polyclave_proto::TurnDto;
/// fn forbidden(turn: TurnDto) { let _ = turn.mcp_token; }
/// ```
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
pub struct TurnDto {
    #[ts(type = "number")]
    pub id: i64,
    pub session_id: String,
    pub run_id: Option<String>,
    #[ts(type = "number | null")]
    pub attempt_id: Option<i64>,
    #[ts(type = "number | null")]
    pub parent_turn_id: Option<i64>,
    pub mode: TurnMode,
    pub status: TurnStatus,
    pub phase: TurnPhase,
    pub exit_code: Option<i32>,
    pub end_reason: Option<EndReason>,
    pub writer_check: Option<WriterCheck>,
    pub started_at: String,
    pub ended_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
pub struct MemoryItemDto {
    pub id: String,
    pub scope: MemoryScope,
    pub repo_cwd: Option<String>,
    pub kind: MemoryKind,
    pub title: String,
    pub content: String,
    pub tags: Vec<String>,
    pub evidence: Option<Value>,
    pub status: MemoryStatus,
    pub supersedes_id: Option<String>,
    pub created_by: String,
    pub source_run_id: Option<String>,
    #[ts(type = "number | null")]
    pub source_turn_id: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
pub struct ProviderProfileDto {
    pub id: String,
    pub display_name: String,
    pub kind: ProviderKind,
    pub models: Vec<String>,
    pub configured: bool,
    pub created_at: String,
}
