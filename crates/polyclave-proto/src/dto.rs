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
    Agy,
    Dsh,
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

/// Where a recorded test result came from.
///
/// Only `ci` and `host_run` are host-observed. `worker_claim` is the Worker's own
/// statement and is always labelled as such.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum TestResultSource {
    WorkerClaim,
    HostRun,
    Ci,
}

impl TestResultSource {
    /// The spelling stored in the ledger.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WorkerClaim => "worker_claim",
            Self::HostRun => "host_run",
            Self::Ci => "ci",
        }
    }
    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw {
            "worker_claim" => Self::WorkerClaim,
            "host_run" => Self::HostRun,
            "ci" => Self::Ci,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum TestStatus {
    Passed,
    Failed,
    Pending,
}

impl TestStatus {
    /// The spelling stored in the ledger.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Pending => "pending",
        }
    }
    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw {
            "passed" => Self::Passed,
            "failed" => Self::Failed,
            "pending" => Self::Pending,
            _ => return None,
        })
    }
}

/// One recorded result for one SHA.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
pub struct TestResultDto {
    pub sha: String,
    pub source: TestResultSource,
    pub status: TestStatus,
    /// Check-run name for `ci`; empty for a claim.
    pub name: String,
    pub summary: String,
    pub recorded_at: String,
}

/// Evidence the approval card shows for a Run's current Attempt.
///
/// Every field is read for that Attempt only, so a receipt or a claim from an
/// older Attempt (and so an older SHA) is never shown as current evidence, and
/// `test_results` holds rows for exactly one SHA — the one in `test_sha`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
pub struct RunEvidenceDto {
    // The Attempt this evidence belongs to.
    pub attempt_no: u32,
    // SHA the Worker submitted with `submit_result` and the host accepted for
    // this Attempt. R1 compares it with the authoritative mirror tip.
    pub receipt_sha: Option<String>,
    // Latest `report_progress` text from this Attempt's turns: the Worker's own
    // claim, never a host check.
    pub worker_claim: Option<String>,
    // The SHA the `test_results` rows belong to: the Run's R1-verified
    // `result_sha`, or its `observed_sha` while nothing is verified yet.
    pub test_sha: Option<String>,
    // The Worker's own test summary from the accepted receipt, and its claimed
    // status. Never a host check.
    pub receipt_tests: Option<String>,
    pub receipt_tests_status: Option<TestStatus>,
    // What the Worker said it did not finish.
    pub receipt_not_done: Option<String>,
    // One row per recorded result for exactly `test_sha`; never another SHA.
    pub test_results: Vec<TestResultDto>,
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

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum CouncilLevel {
    Direction,
    Detail,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum PlanningPreference {
    Thorough,
    Balanced,
    Conservative,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum CouncilStatus {
    Running,
    AwaitingUser,
    Decided,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize, TS)]
pub struct ProposalPayload {
    pub summary: String,
    pub approach: String,
    pub touched_paths: Vec<String>,
    pub public_interface_changes: bool,
    pub new_dependencies: Vec<String>,
    pub security_sensitive: bool,
    pub beyond_request: bool,
    pub open_questions: Vec<String>,
    pub pros: Vec<String>,
    pub cons: Vec<String>,
}

/// Blind projection of one council proposal. `vendor` and `model` are only
/// populated once the owning council is `Decided`.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
pub struct CouncilProposalDto {
    pub label: String,
    pub payload: ProposalPayload,
    pub vendor: Option<Vendor>,
    pub model: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
pub struct CouncilDto {
    pub id: String,
    pub ticket_id: Option<String>,
    pub repo_cwd: String,
    pub level: CouncilLevel,
    pub status: CouncilStatus,
    pub prompt: String,
    pub escalation_reasons: Vec<String>,
    pub chosen_label: Option<String>,
    pub decision_note: Option<String>,
    pub proposals: Vec<CouncilProposalDto>,
    pub created_at: String,
    pub decided_at: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum PrGateStatus {
    Pending,
    ScopeViolation,
    Reviewing,
    ReviseRequested,
    Approved,
    AutoMergeArmed,
    Merged,
    Escalated,
    Paused,
    Failed,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ReviewVerdict {
    Approve,
    Revise,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
pub struct PrReviewDto {
    pub id: String,
    pub pr_gate_id: String,
    pub reviewer_vendor: Vendor,
    pub reviewer_model: String,
    pub reviewed_sha: String,
    pub verdict: ReviewVerdict,
    pub comments: String,
    #[ts(type = "number | null")]
    pub github_review_id: Option<i64>,
    pub created_at: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
pub struct PrGateDto {
    pub id: String,
    pub run_id: String,
    pub ticket_id: String,
    pub repo_slug: String,
    pub branch: String,
    pub base_sha: String,
    pub head_sha: String,
    pub pr_number: Option<u32>,
    pub pr_url: Option<String>,
    pub status: PrGateStatus,
    pub scope_violations: Vec<String>,
    pub revise_rounds: u32,
    pub auto_merge_armed: bool,
    pub reviews: Vec<PrReviewDto>,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
pub struct PauseStateDto {
    pub paused: bool,
    pub reason: Option<String>,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
pub struct PairingTicketDto {
    pub id: String,
    pub client_kind: ClientKind,
    pub device_label: String,
    pub expires_at: String,
    pub used_at: Option<String>,
    pub created_at: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
pub struct PairedDeviceDto {
    pub id: String,
    pub label: String,
    pub client_kind: ClientKind,
    pub peer_identity: Option<String>,
    pub last_seen_at: Option<String>,
    pub revoked_at: Option<String>,
    pub created_at: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum MeshNodeStatus {
    Online,
    Offline,
    Revoked,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
pub struct MeshNodeDto {
    pub id: String,
    pub name: String,
    pub endpoint: String,
    pub os_family: String,
    pub allowed_repos: Vec<String>,
    pub allowed_actions: Vec<String>,
    pub status: MeshNodeStatus,
    pub last_heartbeat_at: Option<String>,
    pub created_at: String,
}

/// Receiver-side concurrency cap for Runs one mesh node may have active here.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
pub struct MeshNodeLimitDto {
    pub node_id: String,
    pub max_concurrent_runs: u32,
    pub active_runs: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum NotificationChannelKind {
    Ntfy,
    UnifiedPush,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
pub struct NotificationChannelDto {
    pub id: String,
    pub kind: NotificationChannelKind,
    pub label: String,
    pub endpoint: String,
    pub topic: String,
    pub enabled: bool,
    pub created_at: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
pub struct CuaLockDto {
    pub locked: bool,
    pub holder_run_id: Option<String>,
    pub target_window: Option<String>,
    pub idle_threshold_secs: u32,
    pub acquired_at: Option<String>,
    pub reclaimed_at: Option<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum EvolutionKind {
    Routing,
    MemoryRule,
    RoleBrief,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum EvolutionStatus {
    Proposed,
    Applied,
    RolledBack,
    Rejected,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
pub struct EvolutionSuggestionDto {
    pub id: String,
    pub kind: EvolutionKind,
    pub title: String,
    pub rationale: String,
    pub payload_json: Value,
    pub status: EvolutionStatus,
    pub supersedes_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
pub struct LocalSkillDto {
    pub id: String,
    pub name: String,
    pub version: u32,
    pub supersedes_id: Option<String>,
    pub description: String,
    pub body_md: String,
    pub enabled: bool,
    pub synced_vendors: Vec<Vendor>,
    pub created_at: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
pub struct CommunityDraftDto {
    pub opt_in: bool,
    pub title: String,
    pub redacted_markdown: String,
    pub stripped_items: u32,
    pub generated_at: String,
}
