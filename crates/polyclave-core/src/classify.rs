use polyclave_proto::{RunStatus, WriterCheck};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BranchCheck {
    Absent,
    Present {
        tip: String,
        problem: Option<String>,
    },
    Unreadable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Verdict {
    pub status: RunStatus,
    pub observed_sha: Option<String>,
    pub result_sha: Option<String>,
    pub detail: Option<String>,
}

pub fn classify_run(writer: WriterCheck, accepted: Option<&str>, branch: BranchCheck) -> Verdict {
    let mut v = Verdict {
        status: RunStatus::NeedsReview,
        observed_sha: None,
        result_sha: None,
        detail: None,
    };
    if branch == BranchCheck::Unreadable {
        v.status = RunStatus::Interrupted;
        v.detail = Some("mirror check failed".into());
        return v;
    }
    if let BranchCheck::Present { tip, .. } = &branch {
        v.observed_sha = Some(tip.clone());
    }
    if writer == WriterCheck::Possible {
        v.detail = Some("a writer may still have access to the old Attempt".into());
        return v;
    }
    match (accepted, branch) {
        (None, BranchCheck::Absent) => {
            v.status = RunStatus::Interrupted;
            v.detail = Some("retryable: no receipt and no mirror branch".into());
        }
        (Some(receipt), BranchCheck::Present { tip, problem: None }) if receipt == tip => {
            v.status = RunStatus::Done;
            v.result_sha = Some(tip);
        }
        (
            _,
            BranchCheck::Present {
                problem: Some(problem),
                ..
            },
        ) => v.detail = Some(problem),
        (None, BranchCheck::Present { .. }) => {
            v.detail = Some("branch published without an accepted receipt".into())
        }
        (Some(_), BranchCheck::Present { .. }) => {
            v.detail = Some("mirror tip differs from the receipt".into())
        }
        (Some(_), BranchCheck::Absent) => {
            v.detail = Some("receipt exists but the mirror branch is missing".into())
        }
        (_, BranchCheck::Unreadable) => unreachable!("handled before evaluation"),
    }
    v
}
