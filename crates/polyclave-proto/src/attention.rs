//! Single exhaustive truth source for "needs human attention" states.
//!
//! Every `match` below intentionally omits `_` wildcard arms so adding a new
//! status variant fails compilation until its attention semantics are decided.
use crate::{CouncilStatus, PrGateStatus, RunStatus};

pub fn run_needs_attention(status: RunStatus) -> Option<&'static str> {
    match status {
        RunStatus::NeedsReview => Some("Run needs review"),
        RunStatus::Failed => Some("Run failed"),
        RunStatus::Pending | RunStatus::Running | RunStatus::Done | RunStatus::Interrupted => None,
    }
}

pub fn pr_gate_needs_attention(status: PrGateStatus) -> Option<&'static str> {
    match status {
        PrGateStatus::AwaitingHumanApproval => Some("PR gate awaiting your approval"),
        PrGateStatus::ScopeViolation => Some("PR gate has scope violations"),
        PrGateStatus::Escalated => Some("PR gate escalated for human review"),
        PrGateStatus::Failed => Some("PR gate failed"),
        PrGateStatus::Pending
        | PrGateStatus::Reviewing
        | PrGateStatus::ReviseRequested
        | PrGateStatus::Approved
        | PrGateStatus::AutoMergeArmed
        | PrGateStatus::Merged
        | PrGateStatus::Paused => None,
    }
}

pub fn council_needs_attention(status: CouncilStatus) -> Option<&'static str> {
    match status {
        CouncilStatus::AwaitingUser => Some("Council awaiting decision"),
        CouncilStatus::Running
        | CouncilStatus::Decided
        | CouncilStatus::Cancelled
        | CouncilStatus::Failed => None,
    }
}
