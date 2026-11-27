//! PR gate orchestration: preflight, 3-layer scope check, disposable
//! heterogeneous review, crash-safe auto-merge, and global Pause revocation.
use crate::{
    scm::{BoxFuture, OpenPrRequest, ReviewEvent, ScmClient, ScmError, SubmitReviewRequest},
    Error, Result,
};
use polyclave_core::{scope::check_touched_paths, worktree::WorktreeManager, GitError};
use polyclave_os::resolve_cli_binary;
use polyclave_proto::{PauseStateDto, PrGateDto, PrGateStatus, ReviewVerdict, Vendor};
use polyclave_store::{NewPrGate, NewPrReview, Store};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
};

const MAX_REVISE_ROUNDS: u32 = 3;

/// Largest full-range review diff (stat, commit list and patch) a reviewer is
/// given. A larger change is never truncated for review: the gate escalates it
/// to a human, because an `approve` on a partial diff would approve code no
/// reviewer read, and a forced `revise` would only spend revise rounds on a
/// size problem the Worker cannot fix by addressing review comments.
pub const MAX_REVIEW_DIFF_BYTES: usize = 24_576;

/// The whole PR range `base..head` prepared for a reviewer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReviewDiff {
    Complete { text: String, commits: usize },
    TooLarge { bytes: usize },
}

/// Renders the full PR range `base..head` (commit list, stat and patch) from
/// `repo`, or reports that it exceeds [`MAX_REVIEW_DIFF_BYTES`].
pub fn review_diff(repo: &Path, base: &str, head: &str) -> Result<ReviewDiff> {
    if !is_full_oid(base) || !is_full_oid(head) {
        return Err(Error::Policy);
    }
    let git = resolve_cli_binary(Path::new("git")).map_err(|_| Error::Policy)?;
    let range = format!("{base}..{head}");
    let run = |args: &[&str]| -> Result<String> {
        let out = clean_git(&git, repo)
            .args(args)
            .output()
            .map_err(|_| Error::Transport)?;
        if !out.status.success() {
            return Err(Error::Policy);
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let log = run(&["log", "--no-decorate", "--format=%H %s", &range, "--"])?;
    let commits = log.lines().filter(|l| !l.trim().is_empty()).count();
    let patch = run(&[
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--stat",
        "--patch",
        base,
        head,
        "--",
    ])?;
    let text = format!("Commits ({commits}):\n{log}\n{patch}");
    if text.len() > MAX_REVIEW_DIFF_BYTES {
        return Ok(ReviewDiff::TooLarge { bytes: text.len() });
    }
    Ok(ReviewDiff::Complete { text, commits })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReviewerSpec {
    pub vendor: Vendor,
    pub model: String,
}

#[derive(Clone, Debug)]
pub struct ReviewWorktreeRequest {
    pub gate_id: String,
    pub repo_slug: String,
    pub pr_number: u64,
    /// The PR base; the reviewer sees the whole range `base_sha..head_sha`.
    pub base_sha: String,
    pub head_sha: String,
    pub worktree_path: PathBuf,
    pub vendor: Vendor,
    pub model: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReviewOutcome {
    pub verdict: ReviewVerdict,
    pub comments: String,
}

pub trait ReviewerExecutor: Send + Sync {
    /// Cheap local check that `spec` can resolve a provider (a CLI binary found
    /// or a configured API profile). The gate runs it for every selected
    /// reviewer before anything is pushed or a PR is opened.
    fn check_available(&self, spec: &ReviewerSpec) -> Result<()>;

    fn run_review<'a>(&'a self, req: ReviewWorktreeRequest)
        -> BoxFuture<'a, Result<ReviewOutcome>>;
}

pub trait WorkerReviseExecutor: Send + Sync {
    fn bounce_scope_violation<'a>(
        &'a self,
        gate: &'a PrGateDto,
        violations: &'a [String],
    ) -> BoxFuture<'a, Result<String>>;

    fn revise_pr<'a>(
        &'a self,
        gate: &'a PrGateDto,
        review_comments: &'a str,
    ) -> BoxFuture<'a, Result<String>>;
}

/// Publishes an R1-verified commit to the remote PR branch.
pub trait MirrorPusher: Send + Sync {
    /// Pushes exactly `sha` from the host mirror to `refs/heads/<branch>` on the
    /// remote, then confirms the remote branch tip equals `sha`.
    ///
    /// `expected_remote_tip` is the lease: `Some(tip)` requires the remote branch
    /// to be at `tip` before the push (a revise round leases on the SHA the gate
    /// pushed last); `None` leases on the tip read from the remote just before
    /// the push, or on the branch being absent.
    fn push_verified_sha<'a>(
        &'a self,
        mirror_path: &'a Path,
        sha: &'a str,
        branch: &'a str,
        expected_remote_tip: Option<&'a str>,
    ) -> BoxFuture<'a, Result<()>>;
}

#[derive(Clone, Default)]
pub struct GitMirrorPusher {
    pub remote: String,
}

impl GitMirrorPusher {
    pub fn new(remote: impl Into<String>) -> Self {
        Self {
            remote: remote.into(),
        }
    }
}

fn is_full_oid(sha: &str) -> bool {
    sha.len() == 40 && sha.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}

fn clean_git(git: &Path, mirror: &Path) -> Command {
    let mut cmd = Command::new(git);
    for (key, _) in std::env::vars_os() {
        if key
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("GIT_")
        {
            cmd.env_remove(key);
        }
    }
    cmd.env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .arg("-C")
        .arg(mirror);
    cmd
}

/// Reads the remote tip of `dest` (`None` when the branch is absent).
fn remote_tip(git: &Path, mirror: &Path, remote: &str, dest: &str) -> Result<Option<String>> {
    let out = clean_git(git, mirror)
        .args(["ls-remote", "--refs", remote, dest])
        .output()
        .map_err(|_| Error::Transport)?;
    if !out.status.success() {
        return Err(Error::Transport);
    }
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        let mut parts = line.split('\t');
        if let (Some(oid), Some(name)) = (parts.next(), parts.next()) {
            if name == dest {
                if !is_full_oid(oid) {
                    return Err(Error::Protocol);
                }
                return Ok(Some(oid.to_owned()));
            }
        }
    }
    Ok(None)
}

impl MirrorPusher for GitMirrorPusher {
    fn push_verified_sha<'a>(
        &'a self,
        mirror_path: &'a Path,
        sha: &'a str,
        branch: &'a str,
        expected_remote_tip: Option<&'a str>,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            if !is_full_oid(sha) || expected_remote_tip.is_some_and(|t| !is_full_oid(t)) {
                return Err(Error::Policy);
            }
            let git = resolve_cli_binary(Path::new("git")).map_err(|_| Error::Policy)?;
            let remote = if self.remote.is_empty() {
                "origin".to_owned()
            } else {
                self.remote.clone()
            };
            let dest = format!("refs/heads/{branch}");
            let mirror = mirror_path.to_path_buf();
            let sha = sha.to_owned();
            let expected = expected_remote_tip.map(str::to_owned);
            tokio::task::spawn_blocking(move || {
                let fmt = clean_git(&git, &mirror)
                    .args(["check-ref-format", &dest])
                    .output()
                    .map_err(|_| Error::Transport)?;
                if !fmt.status.success() {
                    return Err(Error::Policy);
                }
                // The SHA must name a commit the host mirror already holds.
                let commit = clean_git(&git, &mirror)
                    .args(["cat-file", "-e", &format!("{sha}^{{commit}}")])
                    .output()
                    .map_err(|_| Error::Transport)?;
                if !commit.status.success() {
                    return Err(Error::Policy);
                }
                let observed = remote_tip(&git, &mirror, &remote, &dest)?;
                if let Some(expected) = &expected {
                    if observed.as_deref() != Some(expected.as_str())
                        && observed.as_deref() != Some(sha.as_str())
                    {
                        return Err(Error::Policy);
                    }
                }
                if observed.as_deref() != Some(sha.as_str()) {
                    let lease = format!(
                        "--force-with-lease={dest}:{}",
                        observed.as_deref().unwrap_or("")
                    );
                    let out = clean_git(&git, &mirror)
                        .args([
                            "push",
                            "--porcelain",
                            &lease,
                            &remote,
                            &format!("{sha}:{dest}"),
                        ])
                        .output()
                        .map_err(|_| Error::Transport)?;
                    if !out.status.success() {
                        return Err(Error::Transport);
                    }
                }
                // Confirm what the remote now serves is exactly the verified SHA.
                if remote_tip(&git, &mirror, &remote, &dest)?.as_deref() != Some(sha.as_str()) {
                    return Err(Error::Policy);
                }
                Ok(())
            })
            .await
            .map_err(|_| Error::Transport)?
        })
    }
}

#[derive(Clone)]
pub struct RunPrGateRequest {
    pub run_id: String,
    pub hub_session_id: Option<String>,
    pub mirror_path: PathBuf,
    pub repo_slug: String,
    pub branch: String,
    pub base_branch: String,
    pub base_sha: String,
    pub head_sha: String,
    pub pr_title: String,
    pub pr_body: String,
    pub touched_paths: Vec<String>,
    pub is_high_risk: bool,
    pub reviewers: Vec<ReviewerSpec>,
}

pub struct PrGateDeps {
    pub store: Store,
    pub scm: Arc<dyn ScmClient>,
    pub reviewer: Arc<dyn ReviewerExecutor>,
    pub worker_revise: Option<Arc<dyn WorkerReviseExecutor>>,
    pub pusher: Arc<dyn MirrorPusher>,
}

fn map_store_err(err: polyclave_store::Error) -> Error {
    match err {
        polyclave_store::Error::ConflictError
        | polyclave_store::Error::BudgetExceededError
        | polyclave_store::Error::Invalid(_)
        | polyclave_store::Error::NotFound => Error::Policy,
        _ => Error::Store,
    }
}

fn map_scm_err(err: ScmError) -> Error {
    match err {
        ScmError::SameBotAndReviewerAccount { .. }
        | ScmError::InvalidRepoSlug(_)
        | ScmError::EmptyToken(_) => Error::Policy,
        ScmError::Http {
            status, message, ..
        } => Error::Http {
            status,
            detail: message,
        },
        ScmError::GraphQl { message, .. } => Error::Http {
            status: 422,
            detail: message,
        },
        ScmError::Transport { .. } => Error::Transport,
        ScmError::InvalidResponse { .. } => Error::Protocol,
    }
}

fn map_git_err(_err: GitError) -> Error {
    Error::Policy
}

fn vendor_slug(vendor: Vendor) -> &'static str {
    match vendor {
        Vendor::Codex => "codex",
        Vendor::Claude => "claude",
        Vendor::Agy => "agy",
        Vendor::Dsh => "dsh",
    }
}

fn parse_vendor(raw: &str) -> Result<Vendor> {
    match raw {
        "codex" => Ok(Vendor::Codex),
        "claude" => Ok(Vendor::Claude),
        "agy" => Ok(Vendor::Agy),
        "dsh" => Ok(Vendor::Dsh),
        _ => Err(Error::Store),
    }
}

fn lookup_author_launch(store: &Store, run_id: &str) -> Result<(Vendor, Option<String>)> {
    let run_id = run_id.to_owned();
    let row = store
        .read(|db| {
            let mut stmt = db.prepare("SELECT vendor, model FROM runs WHERE id=?1")?;
            let mut rows = stmt.query([run_id])?;
            if let Some(r) = rows.next()? {
                Ok(Some((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                )))
            } else {
                Ok(None)
            }
        })
        .map_err(map_store_err)?;
    let Some((vendor_str, model)) = row else {
        return Err(Error::Policy);
    };
    Ok((parse_vendor(&vendor_str)?, model))
}

fn is_heterogeneous(
    author_vendor: Vendor,
    author_model: Option<&str>,
    reviewer: &ReviewerSpec,
) -> bool {
    if reviewer.vendor != author_vendor {
        return true;
    }
    match author_model {
        Some(m) if !m.is_empty() => reviewer.model != m,
        _ => false,
    }
}

fn enqueue_hub_notice(store: &Store, hub_session_id: Option<&str>, intent: &str, text: &str) {
    if let Some(hub_id) = hub_session_id {
        let _ = store.hub_enqueue(hub_id, intent, text);
    }
}

fn escalate(
    deps: &PrGateDeps,
    gate_id: &str,
    hub_session_id: Option<&str>,
    reason: &str,
    detail: &str,
) -> Result<PrGateDto> {
    let escalated = deps
        .store
        .set_pr_gate_status(gate_id, PrGateStatus::Escalated)
        .map_err(map_store_err)?;
    enqueue_hub_notice(
        &deps.store,
        hub_session_id,
        &format!("pr_gate:{}:{reason}", escalated.id),
        &format!("PR gate {} escalated: {detail}", escalated.id),
    );
    Ok(escalated)
}

/// Runs the full PR gate lifecycle for a completed run.
pub async fn run_pr_gate(deps: &PrGateDeps, req: RunPrGateRequest) -> Result<PrGateDto> {
    if deps.store.pause_state().map_err(map_store_err)?.paused {
        return Err(Error::Policy);
    }

    let (author_vendor, author_model) = lookup_author_launch(&deps.store, &req.run_id)?;
    let required_reviewers = if req.is_high_risk { 2 } else { 1 };
    let selected_reviewers: Vec<ReviewerSpec> = req
        .reviewers
        .iter()
        .filter(|r| is_heterogeneous(author_vendor, author_model.as_deref(), r))
        .take(required_reviewers)
        .cloned()
        .collect();
    if selected_reviewers.len() < required_reviewers {
        return Err(Error::Policy);
    }

    let mut gate = deps
        .store
        .create_pr_gate(NewPrGate {
            run_id: req.run_id.clone(),
            repo_slug: req.repo_slug.clone(),
            branch: req.branch.clone(),
            base_sha: req.base_sha.clone(),
            head_sha: req.head_sha.clone(),
            touched_paths: req.touched_paths.clone(),
        })
        .map_err(map_store_err)?;

    // Reviewer preflight: never push or open a PR that no reviewer can review.
    let unavailable: Vec<String> = selected_reviewers
        .iter()
        .filter(|spec| deps.reviewer.check_available(spec).is_err())
        .map(|spec| format!("{}/{}", vendor_slug(spec.vendor), spec.model))
        .collect();
    if !unavailable.is_empty() {
        return escalate(
            deps,
            &gate.id,
            req.hub_session_id.as_deref(),
            "reviewer_unavailable",
            &format!(
                "no provider resolves for reviewer(s) {}; nothing was pushed",
                unavailable.join(", ")
            ),
        );
    }

    let preflight = match deps
        .scm
        .preflight_repo(&req.repo_slug, &req.base_branch)
        .await
    {
        Ok(report) => report,
        Err(err) => {
            let _ = deps
                .store
                .set_pr_gate_status(&gate.id, PrGateStatus::Failed);
            return Err(map_scm_err(err));
        }
    };
    if !preflight.passed {
        let _ = deps
            .store
            .set_pr_gate_status(&gate.id, PrGateStatus::Failed);
        return Err(Error::Policy);
    }

    // 3-layer touched_paths scope check (1 worker bounce, escalate on 2nd violation)
    let mut current_head = req.head_sha.clone();
    let scope = check_touched_paths(
        &req.mirror_path,
        &req.base_sha,
        &current_head,
        &req.touched_paths,
    )
    .map_err(map_git_err)?;
    if !scope.allowed {
        gate = deps
            .store
            .record_scope_violations(&gate.id, scope.violations.clone())
            .map_err(map_store_err)?;

        let bounced = if let Some(ref exec) = deps.worker_revise {
            match exec.bounce_scope_violation(&gate, &scope.violations).await {
                Ok(sha) => sha,
                Err(_) => {
                    return escalate(
                        deps,
                        &gate.id,
                        req.hub_session_id.as_deref(),
                        "bounce_unverified",
                        "the scope-bounce Worker turn produced no R1-verified result",
                    );
                }
            }
        } else {
            let escalated = deps
                .store
                .set_pr_gate_status(&gate.id, PrGateStatus::Escalated)
                .map_err(map_store_err)?;
            enqueue_hub_notice(
                &deps.store,
                req.hub_session_id.as_deref(),
                &format!("pr_gate:{}:scope_escalated", escalated.id),
                &format!(
                    "PR gate {} escalated due to touched_paths violation: {}",
                    escalated.id,
                    scope.violations.join(", ")
                ),
            );
            return Ok(escalated);
        };

        current_head = bounced;
        gate = deps
            .store
            .set_pr_head(&gate.id, &current_head)
            .map_err(map_store_err)?;
        let scope2 = check_touched_paths(
            &req.mirror_path,
            &req.base_sha,
            &current_head,
            &req.touched_paths,
        )
        .map_err(map_git_err)?;
        if !scope2.allowed {
            let _ = deps
                .store
                .record_scope_violations(&gate.id, scope2.violations.clone())
                .map_err(map_store_err)?;
            let escalated = deps
                .store
                .set_pr_gate_status(&gate.id, PrGateStatus::Escalated)
                .map_err(map_store_err)?;
            enqueue_hub_notice(
                &deps.store,
                req.hub_session_id.as_deref(),
                &format!("pr_gate:{}:scope_escalated_2nd", escalated.id),
                &format!(
                    "PR gate {} escalated on 2nd touched_paths violation: {}",
                    escalated.id,
                    scope2.violations.join(", ")
                ),
            );
            return Ok(escalated);
        }
        gate = deps
            .store
            .record_scope_violations(&gate.id, Vec::new())
            .map_err(map_store_err)?;
    }

    // Push exactly the R1-verified SHA from the authoritative mirror, then
    // open/find the PR. The pusher confirms the remote tip equals that SHA.
    deps.pusher
        .push_verified_sha(&req.mirror_path, &current_head, &req.branch, None)
        .await?;

    let pr = deps
        .scm
        .open_or_find_pr(OpenPrRequest {
            repo_slug: req.repo_slug.clone(),
            head_branch: req.branch.clone(),
            base_branch: req.base_branch.clone(),
            title: req.pr_title.clone(),
            body: req.pr_body.clone(),
        })
        .await
        .map_err(map_scm_err)?;

    gate = deps
        .store
        .record_pr_opened(&gate.id, pr.number as u32, &pr.html_url)
        .map_err(map_store_err)?;

    // Heterogeneous disposable worktree review loop (up to MAX_REVISE_ROUNDS)
    loop {
        if deps.store.pause_state().map_err(map_store_err)?.paused {
            return deps
                .store
                .set_pr_gate_status(&gate.id, PrGateStatus::Paused)
                .map_err(map_store_err);
        }

        gate = deps
            .store
            .set_pr_gate_status(&gate.id, PrGateStatus::Reviewing)
            .map_err(map_store_err)?;

        // Reviewers see the whole PR range; never review a truncated diff.
        if let ReviewDiff::TooLarge { bytes } =
            review_diff(&req.mirror_path, &req.base_sha, &current_head)?
        {
            return escalate(
                deps,
                &gate.id,
                req.hub_session_id.as_deref(),
                "diff_too_large",
                &format!(
                    "the PR diff {}..{} is {bytes} bytes, over the {MAX_REVIEW_DIFF_BYTES}-byte review limit; a human must review it",
                    req.base_sha, current_head
                ),
            );
        }

        let mut revise_comments = Vec::new();
        for spec in &selected_reviewers {
            let mirror = req.mirror_path.clone();
            let sha = current_head.clone();
            let gate_id = gate.id.clone();
            let repo_slug = req.repo_slug.clone();
            let pr_number = pr.number;
            let base_sha = req.base_sha.clone();
            let reviewer = deps.reviewer.clone();
            let spec_clone = spec.clone();

            let outcome = WorktreeManager::with_disposable_review_worktree_async(
                &mirror,
                &sha.clone(),
                move |wt_path| async move {
                    reviewer
                        .run_review(ReviewWorktreeRequest {
                            gate_id,
                            repo_slug,
                            pr_number,
                            base_sha,
                            head_sha: sha,
                            worktree_path: wt_path,
                            vendor: spec_clone.vendor,
                            model: spec_clone.model,
                        })
                        .await
                        .map_err(|_| GitError::Command("heterogeneous review failed"))
                },
            )
            .await
            .map_err(map_git_err)?;

            let event = match outcome.verdict {
                ReviewVerdict::Approve => ReviewEvent::Approve,
                ReviewVerdict::Revise => ReviewEvent::RequestChanges,
            };
            let submitted = deps
                .scm
                .submit_review(SubmitReviewRequest {
                    repo_slug: req.repo_slug.clone(),
                    pr_number: pr.number,
                    reviewed_sha: current_head.clone(),
                    vendor: vendor_slug(spec.vendor).to_owned(),
                    model: spec.model.clone(),
                    event,
                    body: outcome.comments.clone(),
                })
                .await
                .map_err(map_scm_err)?;

            gate = deps
                .store
                .record_pr_review(
                    &gate.id,
                    NewPrReview {
                        reviewer_vendor: spec.vendor,
                        reviewer_model: spec.model.clone(),
                        reviewed_sha: current_head.clone(),
                        verdict: outcome.verdict,
                        comments: outcome.comments.clone(),
                        github_review_id: Some(submitted.id as i64),
                    },
                )
                .map_err(map_store_err)?;

            if outcome.verdict == ReviewVerdict::Revise {
                revise_comments.push(outcome.comments);
            }
        }

        if revise_comments.is_empty() {
            break;
        }

        if gate.revise_rounds >= MAX_REVISE_ROUNDS {
            let escalated = deps
                .store
                .set_pr_gate_status(&gate.id, PrGateStatus::Escalated)
                .map_err(map_store_err)?;
            enqueue_hub_notice(
                &deps.store,
                req.hub_session_id.as_deref(),
                &format!("pr_gate:{}:max_revise", escalated.id),
                &format!(
                    "PR gate {} escalated after {} revise rounds",
                    escalated.id, escalated.revise_rounds
                ),
            );
            return Ok(escalated);
        }

        let Some(ref exec) = deps.worker_revise else {
            return deps
                .store
                .set_pr_gate_status(&gate.id, PrGateStatus::ReviseRequested)
                .map_err(map_store_err);
        };

        gate = deps
            .store
            .begin_revise_round(&gate.id)
            .map_err(map_store_err)?;
        let combined = revise_comments.join("\n---\n");
        // Only an R1-verified result may reach review, push and auto-merge; a
        // revise turn without one escalates to a human.
        let new_head = match exec.revise_pr(&gate, &combined).await {
            Ok(sha) => sha,
            Err(_) => {
                return escalate(
                    deps,
                    &gate.id,
                    req.hub_session_id.as_deref(),
                    "revise_unverified",
                    "the revise Worker turn produced no R1-verified result",
                );
            }
        };

        let revise_scope = check_touched_paths(
            &req.mirror_path,
            &req.base_sha,
            &new_head,
            &req.touched_paths,
        )
        .map_err(map_git_err)?;
        if !revise_scope.allowed {
            let _ = deps
                .store
                .record_scope_violations(&gate.id, revise_scope.violations)
                .map_err(map_store_err)?;
            return deps
                .store
                .set_pr_gate_status(&gate.id, PrGateStatus::Escalated)
                .map_err(map_store_err);
        }

        deps.pusher
            .push_verified_sha(
                &req.mirror_path,
                &new_head,
                &req.branch,
                Some(current_head.as_str()),
            )
            .await?;
        current_head = new_head;
        gate = deps
            .store
            .set_pr_head(&gate.id, &current_head)
            .map_err(map_store_err)?;
    }

    gate = deps
        .store
        .set_pr_gate_status(&gate.id, PrGateStatus::Approved)
        .map_err(map_store_err)?;

    if deps.store.pause_state().map_err(map_store_err)?.paused {
        return deps
            .store
            .set_pr_gate_status(&gate.id, PrGateStatus::Paused)
            .map_err(map_store_err);
    }

    // Crash-safe ordering: persist auto_merge_armed = 1 in SQLite BEFORE
    // calling GitHub enableAutoMerge(expectedHeadOid).
    let armed = deps
        .store
        .arm_auto_merge(&gate.id, &current_head)
        .map_err(map_store_err)?;

    if let Err(err) = deps.scm.enable_auto_merge(&pr.node_id, &current_head).await {
        let _ = deps
            .store
            .disarm_auto_merge(&gate.id, PrGateStatus::Approved);
        return Err(map_scm_err(err));
    }

    Ok(armed)
}

/// Persists global Pause, revokes auto-merge on GitHub for all armed PRs, and
/// disarms each gate in SQLite once GitHub confirms revocation.
pub async fn pause_all(
    store: &Store,
    scm: &dyn ScmClient,
    reason: Option<String>,
) -> Result<PauseStateDto> {
    let state = store.set_pause(true, reason).map_err(map_store_err)?;
    let armed_gates = store.armed_pr_gates().map_err(map_store_err)?;
    for gate in armed_gates {
        let Some(pr_number) = gate.pr_number else {
            continue;
        };
        let pr_state = scm
            .get_pr_state(&gate.repo_slug, pr_number as u64)
            .await
            .map_err(map_scm_err)?;
        scm.disable_auto_merge(&pr_state.node_id)
            .await
            .map_err(map_scm_err)?;
        store
            .disarm_auto_merge(&gate.id, PrGateStatus::Paused)
            .map_err(map_store_err)?;
    }
    Ok(state)
}

/// Clears the global Pause flag.
pub fn resume_all(store: &Store) -> Result<PauseStateDto> {
    store.set_pause(false, None).map_err(map_store_err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scm::{GitHubConfig, GitHubScmClient, SecretToken};
    use axum::{
        extract::State,
        routing::{get, post},
        Json, Router,
    };
    use serde_json::{json, Value};
    use std::process::Command as StdCommand;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    use tokio::net::TcpListener;

    #[derive(Default)]
    struct FakeGhState {
        enable_auto_merge_calls: Mutex<Vec<(String, String)>>,
        disable_auto_merge_calls: Mutex<Vec<String>>,
        submitted_reviews: Mutex<Vec<Value>>,
        store_armed_before_enable: Mutex<Option<bool>>,
    }

    struct AppState {
        gh: Arc<FakeGhState>,
        store: Store,
        gate_id: Mutex<Option<String>>,
    }

    async fn spawn_fake_github(
        store: Store,
    ) -> (
        String,
        Arc<FakeGhState>,
        Arc<AppState>,
        tokio::task::JoinHandle<()>,
    ) {
        let gh = Arc::new(FakeGhState::default());
        let app_state = Arc::new(AppState {
            gh: gh.clone(),
            store,
            gate_id: Mutex::new(None),
        });

        let router = Router::new()
            .route(
                "/user",
                get(|headers: axum::http::HeaderMap| async move {
                    let auth = headers
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("");
                    if auth.contains("bot-tok") {
                        Json(json!({"login": "polyclave-bot"}))
                    } else {
                        Json(json!({"login": "polyclave-reviewer"}))
                    }
                }),
            )
            .route(
                "/repos/{owner}/{repo}",
                get(|| async {
                    Json(json!({
                        "default_branch": "main",
                        "allow_squash_merge": true,
                        "allow_auto_merge": true
                    }))
                }),
            )
            .route(
                "/repos/{owner}/{repo}/branches/{branch}/protection",
                get(|| async {
                    Json(json!({
                        "required_pull_request_reviews": {
                            "required_approving_review_count": 1
                        }
                    }))
                }),
            )
            .route(
                "/repos/{owner}/{repo}/pulls",
                get(|| async { Json(json!([])) }).post(|| async {
                    (
                        axum::http::StatusCode::CREATED,
                        Json(json!({
                            "number": 42,
                            "node_id": "PR_node_42",
                            "state": "open",
                            "html_url": "https://github.com/org/repo/pull/42",
                            "head": { "sha": "head_sha_placeholder" }
                        })),
                    )
                }),
            )
            .route(
                "/repos/{owner}/{repo}/pulls/{num}",
                get(|| async {
                    Json(json!({
                        "number": 42,
                        "node_id": "PR_node_42",
                        "state": "open",
                        "merged": false,
                        "mergeable": true,
                        "mergeable_state": "clean",
                        "html_url": "https://github.com/org/repo/pull/42",
                        "head": { "sha": "head_sha_placeholder" },
                        "auto_merge": null
                    }))
                }),
            )
            .route(
                "/repos/{owner}/{repo}/pulls/{num}/reviews",
                post(
                    |State(st): State<Arc<AppState>>, Json(body): Json<Value>| async move {
                        st.gh.submitted_reviews.lock().unwrap().push(body.clone());
                        Json(json!({
                            "id": 9001,
                            "commit_id": body["commit_id"],
                            "state": body["event"],
                            "html_url": "https://github.com/org/repo/pull/42#pullrequestreview-9001"
                        }))
                    },
                ),
            )
            .route(
                "/graphql",
                post(
                    |State(st): State<Arc<AppState>>, Json(body): Json<Value>| async move {
                        let query = body["query"].as_str().unwrap_or("");
                        if query.contains("enablePullRequestAutoMerge") {
                            let pr_id = body["variables"]["pullRequestId"]
                                .as_str()
                                .unwrap_or("")
                                .to_owned();
                            let expected = body["variables"]["expectedHeadOid"]
                                .as_str()
                                .unwrap_or("")
                                .to_owned();
                            // Verify SQLite already persisted auto_merge_armed = true BEFORE this call!
                            let armed = st
                                .gate_id
                                .lock()
                                .unwrap()
                                .as_ref()
                                .and_then(|gid| st.store.pr_gate(gid).ok())
                                .map(|g| g.auto_merge_armed);
                            *st.gh.store_armed_before_enable.lock().unwrap() = armed;
                            st.gh
                                .enable_auto_merge_calls
                                .lock()
                                .unwrap()
                                .push((pr_id, expected));
                            Json(json!({"data": {"enablePullRequestAutoMerge": {"pullRequest": {"id": "PR_node_42"}}}}))
                        } else if query.contains("disablePullRequestAutoMerge") {
                            let pr_id = body["variables"]["pullRequestId"]
                                .as_str()
                                .unwrap_or("")
                                .to_owned();
                            st.gh.disable_auto_merge_calls.lock().unwrap().push(pr_id);
                            Json(json!({"data": {"disablePullRequestAutoMerge": {"pullRequest": {"id": "PR_node_42"}}}}))
                        } else {
                            Json(json!({"data": {}}))
                        }
                    },
                ),
            )
            .with_state(app_state.clone());

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        (format!("http://{addr}"), gh, app_state, handle)
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = StdCommand::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    struct GitFixture {
        _dir: tempfile::TempDir,
        mirror: PathBuf,
        base_sha: String,
        good_head_sha: String,
        bad_head_sha_1: String,
        bad_head_sha_2: String,
        store: Store,
    }

    fn setup_git_and_store() -> GitFixture {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("work");
        std::fs::create_dir_all(work.join("src")).unwrap();
        git(&work, &["init", "-b", "main", "--template="]);
        git(&work, &["config", "user.name", "Test"]);
        git(&work, &["config", "user.email", "test@example.invalid"]);
        std::fs::write(work.join("src/lib.rs"), "pub fn base() {}\n").unwrap();
        git(&work, &["add", "."]);
        git(&work, &["commit", "-m", "base"]);
        let base_sha = git(&work, &["rev-parse", "HEAD"]);

        // Good head touching src/lib.rs
        std::fs::write(work.join("src/lib.rs"), "pub fn base() -> i32 { 1 }\n").unwrap();
        git(&work, &["add", "."]);
        git(&work, &["commit", "-m", "good"]);
        let good_head_sha = git(&work, &["rev-parse", "HEAD"]);

        // Bad head 1 touching README.md
        git(&work, &["checkout", "-b", "bad1", &base_sha]);
        std::fs::write(work.join("README.md"), "oops\n").unwrap();
        git(&work, &["add", "."]);
        git(&work, &["commit", "-m", "bad1"]);
        let bad_head_sha_1 = git(&work, &["rev-parse", "HEAD"]);

        // Bad head 2 still touching README.md
        std::fs::write(work.join("README.md"), "still oops\n").unwrap();
        git(&work, &["add", "."]);
        git(&work, &["commit", "-m", "bad2"]);
        let bad_head_sha_2 = git(&work, &["rev-parse", "HEAD"]);

        let mirror = dir.path().join("mirror.git");
        git(
            dir.path(),
            &[
                "clone",
                "--bare",
                work.to_str().unwrap(),
                mirror.to_str().unwrap(),
            ],
        );
        let mirror = polyclave_os::canonical_real_path(&mirror).unwrap();

        let store = Store::open(dir.path().join("ledger")).unwrap();
        store
            .write(|db| {
                db.execute_batch(
                    "INSERT INTO provider_profiles(id,display_name,kind,models_json,created_at)
                     VALUES('p','test','codex_cli','[]','now');
                     INSERT INTO sessions(id,name,role,repo_cwd,profile_id,provider_model,created_at)
                     VALUES('hub','hub','hub','/tmp/repo','p','test','now');
                     INSERT INTO sessions(id,name,role,repo_cwd,profile_id,provider_model,created_at)
                     VALUES('w1','w1','worker','/tmp/repo','p','gpt-5.3-codex','now');
                     INSERT INTO tickets(id,hub_session_id,repo_cwd,title,mandate_json,source_kind,max_runs,runs_used,status,created_at,updated_at)
                     VALUES('t1','hub','/tmp/repo','t1','{}','explicit_user',4,1,'active','now','now');
                     INSERT INTO runs(id,ticket_id,hub_session_id,worker_session_id,request_id,repo_cwd,vendor,model,instructions,mirror_remote,base_branch,base_sha,status,created_at,updated_at)
                     VALUES('run1','t1','hub','w1','req1','/tmp/repo','codex','gpt-5.3-codex','do it','origin','main','base','done','now','now');",
                )?;
                Ok(())
            })
            .unwrap();

        GitFixture {
            _dir: dir,
            mirror,
            base_sha,
            good_head_sha,
            bad_head_sha_1,
            bad_head_sha_2,
            store,
        }
    }

    struct MockPusher {
        pushed: Mutex<Vec<(String, String)>>,
    }
    impl MirrorPusher for MockPusher {
        fn push_verified_sha<'a>(
            &'a self,
            _mirror_path: &'a Path,
            sha: &'a str,
            branch: &'a str,
            _expected_remote_tip: Option<&'a str>,
        ) -> BoxFuture<'a, Result<()>> {
            Box::pin(async move {
                self.pushed
                    .lock()
                    .unwrap()
                    .push((sha.to_owned(), branch.to_owned()));
                Ok(())
            })
        }
    }

    struct MockReviewer {
        verdicts: Mutex<Vec<ReviewVerdict>>,
        seen_worktrees: Mutex<Vec<PathBuf>>,
    }
    impl ReviewerExecutor for MockReviewer {
        fn check_available(&self, _spec: &ReviewerSpec) -> Result<()> {
            Ok(())
        }
        fn run_review<'a>(
            &'a self,
            req: ReviewWorktreeRequest,
        ) -> BoxFuture<'a, Result<ReviewOutcome>> {
            Box::pin(async move {
                assert!(req.worktree_path.exists());
                self.seen_worktrees.lock().unwrap().push(req.worktree_path);
                let verdict = self.verdicts.lock().unwrap().remove(0);
                Ok(ReviewOutcome {
                    verdict,
                    comments: "LGTM".to_owned(),
                })
            })
        }
    }

    struct MockRevise {
        next_sha: String,
        bounces: AtomicUsize,
    }
    impl WorkerReviseExecutor for MockRevise {
        fn bounce_scope_violation<'a>(
            &'a self,
            _gate: &'a PrGateDto,
            _violations: &'a [String],
        ) -> BoxFuture<'a, Result<String>> {
            Box::pin(async move {
                self.bounces.fetch_add(1, Ordering::SeqCst);
                Ok(self.next_sha.clone())
            })
        }
        fn revise_pr<'a>(
            &'a self,
            _gate: &'a PrGateDto,
            _review_comments: &'a str,
        ) -> BoxFuture<'a, Result<String>> {
            Box::pin(async move { Ok(self.next_sha.clone()) })
        }
    }

    #[tokio::test]
    async fn full_pr_gate_disposable_worktree_crash_safe_automerge_and_pause_revocation() {
        let fix = setup_git_and_store();
        let (base_url, gh, app_state, server) = spawn_fake_github(fix.store.clone()).await;
        let scm = Arc::new(
            GitHubScmClient::new(
                GitHubConfig::new(SecretToken::new("bot-tok"), SecretToken::new("rev-tok"))
                    .with_base_urls(&base_url, format!("{base_url}/graphql")),
            )
            .unwrap(),
        );
        let reviewer = Arc::new(MockReviewer {
            verdicts: Mutex::new(vec![ReviewVerdict::Approve]),
            seen_worktrees: Mutex::new(Vec::new()),
        });
        let pusher = Arc::new(MockPusher {
            pushed: Mutex::new(Vec::new()),
        });
        let deps = PrGateDeps {
            store: fix.store.clone(),
            scm: scm.clone(),
            reviewer: reviewer.clone(),
            worker_revise: None,
            pusher: pusher.clone(),
        };

        // 1. Same vendor+model reviewer must be refused
        let err = run_pr_gate(
            &deps,
            RunPrGateRequest {
                run_id: "run1".into(),
                hub_session_id: Some("hub".into()),
                mirror_path: fix.mirror.clone(),
                repo_slug: "org/repo".into(),
                branch: "feat/x".into(),
                base_branch: "main".into(),
                base_sha: fix.base_sha.clone(),
                head_sha: fix.good_head_sha.clone(),
                pr_title: "feat: x".into(),
                pr_body: "body".into(),
                touched_paths: vec!["src/**".into()],
                is_high_risk: false,
                reviewers: vec![ReviewerSpec {
                    vendor: Vendor::Codex,
                    model: "gpt-5.3-codex".into(),
                }],
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, Error::Policy));

        // 2. Heterogeneous reviewer succeeds, arms auto-merge AFTER persisting in SQLite
        // Pre-populate gate_id observer hook via list_pr_gates after creation isn't needed if we
        // check store_armed_before_enable: let's set gate_id by wrapping or checking armed_pr_gates.
        let gate = run_pr_gate(
            &deps,
            RunPrGateRequest {
                run_id: "run1".into(),
                hub_session_id: Some("hub".into()),
                mirror_path: fix.mirror.clone(),
                repo_slug: "org/repo".into(),
                branch: "feat/x".into(),
                base_branch: "main".into(),
                base_sha: fix.base_sha.clone(),
                head_sha: fix.good_head_sha.clone(),
                pr_title: "feat: x".into(),
                pr_body: "body".into(),
                touched_paths: vec!["src/**".into()],
                is_high_risk: false,
                reviewers: vec![ReviewerSpec {
                    vendor: Vendor::Claude,
                    model: "claude-sonnet-4-6".into(),
                }],
            },
        )
        .await
        .unwrap();

        assert_eq!(gate.status, PrGateStatus::AutoMergeArmed);
        assert!(gate.auto_merge_armed);
        assert_eq!(gate.pr_number, Some(42));
        assert_eq!(pusher.pushed.lock().unwrap().len(), 1);
        // Disposable worktree must have been cleaned up after review
        {
            let wt_paths = reviewer.seen_worktrees.lock().unwrap();
            assert_eq!(wt_paths.len(), 1);
            assert!(!wt_paths[0].exists());
        }

        // Verify review commit_id pinning and marker
        {
            let reviews = gh.submitted_reviews.lock().unwrap();
            assert_eq!(reviews.len(), 1);
            assert_eq!(reviews[0]["commit_id"], fix.good_head_sha);
            assert!(reviews[0]["body"]
                .as_str()
                .unwrap()
                .contains("[agent-reviewer · claude/claude-sonnet-4-6 · "));
        }

        // Verify enableAutoMerge received expectedHeadOid
        {
            let enables = gh.enable_auto_merge_calls.lock().unwrap();
            assert_eq!(enables.len(), 1);
            assert_eq!(enables[0].1, fix.good_head_sha);
        }

        // 3. Global Pause revokes armed auto-merges on GitHub and blocks new dispatches
        let pause_state = pause_all(&fix.store, scm.as_ref(), Some("incident".into()))
            .await
            .unwrap();
        assert!(pause_state.paused);
        assert_eq!(gh.disable_auto_merge_calls.lock().unwrap().len(), 1);
        let paused_gate = fix.store.pr_gate(&gate.id).unwrap();
        assert!(!paused_gate.auto_merge_armed);
        assert_eq!(paused_gate.status, PrGateStatus::Paused);

        // New dispatch refused while paused
        let blocked = run_pr_gate(
            &deps,
            RunPrGateRequest {
                run_id: "run1".into(),
                hub_session_id: None,
                mirror_path: fix.mirror.clone(),
                repo_slug: "org/repo".into(),
                branch: "feat/y".into(),
                base_branch: "main".into(),
                base_sha: fix.base_sha.clone(),
                head_sha: fix.good_head_sha.clone(),
                pr_title: "feat: y".into(),
                pr_body: "body".into(),
                touched_paths: vec!["src/**".into()],
                is_high_risk: false,
                reviewers: vec![ReviewerSpec {
                    vendor: Vendor::Claude,
                    model: "claude-sonnet-4-6".into(),
                }],
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(blocked, Error::Policy));

        let resumed = resume_all(&fix.store).unwrap();
        assert!(!resumed.paused);

        let _ = app_state;
        server.abort();
    }

    #[tokio::test]
    async fn scope_violation_bounces_once_then_escalates_on_second_violation() {
        let fix = setup_git_and_store();
        let (base_url, _gh, _app_state, server) = spawn_fake_github(fix.store.clone()).await;
        let scm = Arc::new(
            GitHubScmClient::new(
                GitHubConfig::new(SecretToken::new("bot-tok"), SecretToken::new("rev-tok"))
                    .with_base_urls(&base_url, format!("{base_url}/graphql")),
            )
            .unwrap(),
        );
        let revise = Arc::new(MockRevise {
            next_sha: fix.bad_head_sha_2.clone(),
            bounces: AtomicUsize::new(0),
        });
        let deps = PrGateDeps {
            store: fix.store.clone(),
            scm,
            reviewer: Arc::new(MockReviewer {
                verdicts: Mutex::new(vec![]),
                seen_worktrees: Mutex::new(Vec::new()),
            }),
            worker_revise: Some(revise.clone()),
            pusher: Arc::new(MockPusher {
                pushed: Mutex::new(Vec::new()),
            }),
        };

        let gate = run_pr_gate(
            &deps,
            RunPrGateRequest {
                run_id: "run1".into(),
                hub_session_id: Some("hub".into()),
                mirror_path: fix.mirror.clone(),
                repo_slug: "org/repo".into(),
                branch: "feat/scope".into(),
                base_branch: "main".into(),
                base_sha: fix.base_sha.clone(),
                head_sha: fix.bad_head_sha_1.clone(),
                pr_title: "feat: scope".into(),
                pr_body: "body".into(),
                touched_paths: vec!["src/**".into()],
                is_high_risk: false,
                reviewers: vec![ReviewerSpec {
                    vendor: Vendor::Claude,
                    model: "claude-sonnet-4-6".into(),
                }],
            },
        )
        .await
        .unwrap();

        assert_eq!(revise.bounces.load(Ordering::SeqCst), 1);
        assert_eq!(gate.status, PrGateStatus::Escalated);
        assert_eq!(gate.scope_violations, vec!["README.md".to_owned()]);
        server.abort();
    }

    #[tokio::test]
    async fn git_pusher_pushes_exact_sha_with_lease_and_confirms_remote_tip() {
        let fix = setup_git_and_store();
        let remote = fix._dir.path().join("remote.git");
        git(
            fix._dir.path(),
            &["init", "--bare", "--template=", remote.to_str().unwrap()],
        );
        let pusher = GitMirrorPusher::new(remote.to_str().unwrap());
        let tip = |branch: &str| git(&remote, &["rev-parse", &format!("refs/heads/{branch}")]);

        // First push: the branch is absent, so the lease is "must not exist".
        pusher
            .push_verified_sha(&fix.mirror, &fix.good_head_sha, "feat/x", None)
            .await
            .unwrap();
        assert_eq!(tip("feat/x"), fix.good_head_sha);
        // Nothing else was published from the mirror.
        let refs = git(&remote, &["for-each-ref", "--format=%(refname)"]);
        assert_eq!(refs, "refs/heads/feat/x");

        // Re-pushing the same SHA is a no-op that still confirms the tip.
        pusher
            .push_verified_sha(&fix.mirror, &fix.good_head_sha, "feat/x", None)
            .await
            .unwrap();

        // A revise round leases on the SHA it pushed last; a stale lease is refused
        // and leaves the remote untouched.
        let stale = pusher
            .push_verified_sha(
                &fix.mirror,
                &fix.bad_head_sha_2,
                "feat/x",
                Some(fix.bad_head_sha_1.as_str()),
            )
            .await;
        assert!(stale.is_err());
        assert_eq!(tip("feat/x"), fix.good_head_sha);

        // With the right lease a non-fast-forward revise head is published exactly.
        pusher
            .push_verified_sha(
                &fix.mirror,
                &fix.bad_head_sha_2,
                "feat/x",
                Some(fix.good_head_sha.as_str()),
            )
            .await
            .unwrap();
        assert_eq!(tip("feat/x"), fix.bad_head_sha_2);

        // Abbreviated SHAs, unknown commits and bad branch names are refused.
        assert!(pusher
            .push_verified_sha(&fix.mirror, &fix.good_head_sha[..12], "feat/y", None)
            .await
            .is_err());
        assert!(pusher
            .push_verified_sha(&fix.mirror, &"a".repeat(40), "feat/y", None)
            .await
            .is_err());
        assert!(pusher
            .push_verified_sha(&fix.mirror, &fix.good_head_sha, "bad..name", None)
            .await
            .is_err());
        assert!(git(&remote, &["for-each-ref", "--format=%(refname)"]) == "refs/heads/feat/x");
    }

    #[tokio::test]
    async fn gate_pushes_the_verified_head_sha_not_a_side_ref() {
        let fix = setup_git_and_store();
        let (base_url, gh, _app_state, server) = spawn_fake_github(fix.store.clone()).await;
        let scm = Arc::new(
            GitHubScmClient::new(
                GitHubConfig::new(SecretToken::new("bot-tok"), SecretToken::new("rev-tok"))
                    .with_base_urls(&base_url, format!("{base_url}/graphql")),
            )
            .unwrap(),
        );
        let pusher = Arc::new(MockPusher {
            pushed: Mutex::new(Vec::new()),
        });
        let deps = PrGateDeps {
            store: fix.store.clone(),
            scm,
            reviewer: Arc::new(MockReviewer {
                verdicts: Mutex::new(vec![ReviewVerdict::Approve]),
                seen_worktrees: Mutex::new(Vec::new()),
            }),
            worker_revise: None,
            pusher: pusher.clone(),
        };
        let gate = run_pr_gate(
            &deps,
            RunPrGateRequest {
                run_id: "run1".into(),
                hub_session_id: None,
                mirror_path: fix.mirror.clone(),
                repo_slug: "org/repo".into(),
                branch: "feat/sha".into(),
                base_branch: "main".into(),
                base_sha: fix.base_sha.clone(),
                head_sha: fix.good_head_sha.clone(),
                pr_title: "feat: sha".into(),
                pr_body: "body".into(),
                touched_paths: vec!["src/**".into()],
                is_high_risk: false,
                reviewers: vec![ReviewerSpec {
                    vendor: Vendor::Claude,
                    model: "claude-sonnet-4-6".into(),
                }],
            },
        )
        .await
        .unwrap();
        assert_eq!(gate.status, PrGateStatus::AutoMergeArmed);
        assert_eq!(
            *pusher.pushed.lock().unwrap(),
            vec![(fix.good_head_sha.clone(), "feat/sha".to_owned())]
        );
        assert_eq!(
            gh.enable_auto_merge_calls.lock().unwrap()[0].1,
            fix.good_head_sha
        );
        server.abort();
    }

    /// A revise turn that ends with only an observed (unverified) tip: the host
    /// executor reports no R1-verified result.
    struct UnverifiedRevise {
        revisions: AtomicUsize,
    }
    impl WorkerReviseExecutor for UnverifiedRevise {
        fn bounce_scope_violation<'a>(
            &'a self,
            _gate: &'a PrGateDto,
            _violations: &'a [String],
        ) -> BoxFuture<'a, Result<String>> {
            Box::pin(async { Err(Error::Policy) })
        }
        fn revise_pr<'a>(
            &'a self,
            _gate: &'a PrGateDto,
            _review_comments: &'a str,
        ) -> BoxFuture<'a, Result<String>> {
            Box::pin(async move {
                self.revisions.fetch_add(1, Ordering::SeqCst);
                Err(Error::Policy)
            })
        }
    }

    #[tokio::test]
    async fn unverified_revise_result_escalates_instead_of_reaching_review() {
        let fix = setup_git_and_store();
        let (base_url, gh, _app_state, server) = spawn_fake_github(fix.store.clone()).await;
        let scm = Arc::new(
            GitHubScmClient::new(
                GitHubConfig::new(SecretToken::new("bot-tok"), SecretToken::new("rev-tok"))
                    .with_base_urls(&base_url, format!("{base_url}/graphql")),
            )
            .unwrap(),
        );
        // Only one verdict: a second review would panic on the empty queue.
        let reviewer = Arc::new(MockReviewer {
            verdicts: Mutex::new(vec![ReviewVerdict::Revise]),
            seen_worktrees: Mutex::new(Vec::new()),
        });
        let pusher = Arc::new(MockPusher {
            pushed: Mutex::new(Vec::new()),
        });
        let revise = Arc::new(UnverifiedRevise {
            revisions: AtomicUsize::new(0),
        });
        let deps = PrGateDeps {
            store: fix.store.clone(),
            scm,
            reviewer: reviewer.clone(),
            worker_revise: Some(revise.clone()),
            pusher: pusher.clone(),
        };
        let gate = run_pr_gate(
            &deps,
            RunPrGateRequest {
                run_id: "run1".into(),
                hub_session_id: Some("hub".into()),
                mirror_path: fix.mirror.clone(),
                repo_slug: "org/repo".into(),
                branch: "feat/revise".into(),
                base_branch: "main".into(),
                base_sha: fix.base_sha.clone(),
                head_sha: fix.good_head_sha.clone(),
                pr_title: "feat: revise".into(),
                pr_body: "body".into(),
                touched_paths: vec!["src/**".into()],
                is_high_risk: false,
                reviewers: vec![ReviewerSpec {
                    vendor: Vendor::Claude,
                    model: "claude-sonnet-4-6".into(),
                }],
            },
        )
        .await
        .unwrap();

        assert_eq!(gate.status, PrGateStatus::Escalated);
        assert!(!gate.auto_merge_armed);
        assert_eq!(gate.head_sha, fix.good_head_sha);
        assert_eq!(revise.revisions.load(Ordering::SeqCst), 1);
        // Reviewed once (the verified head), never the revise output.
        assert_eq!(reviewer.seen_worktrees.lock().unwrap().len(), 1);
        assert_eq!(
            *pusher.pushed.lock().unwrap(),
            vec![(fix.good_head_sha.clone(), "feat/revise".to_owned())]
        );
        assert!(gh.enable_auto_merge_calls.lock().unwrap().is_empty());
        server.abort();
    }

    #[tokio::test]
    async fn unverified_scope_bounce_result_escalates_without_push() {
        let fix = setup_git_and_store();
        let (base_url, _gh, _app_state, server) = spawn_fake_github(fix.store.clone()).await;
        let scm = Arc::new(
            GitHubScmClient::new(
                GitHubConfig::new(SecretToken::new("bot-tok"), SecretToken::new("rev-tok"))
                    .with_base_urls(&base_url, format!("{base_url}/graphql")),
            )
            .unwrap(),
        );
        let pusher = Arc::new(MockPusher {
            pushed: Mutex::new(Vec::new()),
        });
        let deps = PrGateDeps {
            store: fix.store.clone(),
            scm,
            reviewer: Arc::new(MockReviewer {
                verdicts: Mutex::new(vec![]),
                seen_worktrees: Mutex::new(Vec::new()),
            }),
            worker_revise: Some(Arc::new(UnverifiedRevise {
                revisions: AtomicUsize::new(0),
            })),
            pusher: pusher.clone(),
        };
        let gate = run_pr_gate(
            &deps,
            RunPrGateRequest {
                run_id: "run1".into(),
                hub_session_id: None,
                mirror_path: fix.mirror.clone(),
                repo_slug: "org/repo".into(),
                branch: "feat/bounce".into(),
                base_branch: "main".into(),
                base_sha: fix.base_sha.clone(),
                head_sha: fix.bad_head_sha_1.clone(),
                pr_title: "feat: bounce".into(),
                pr_body: "body".into(),
                touched_paths: vec!["src/**".into()],
                is_high_risk: false,
                reviewers: vec![ReviewerSpec {
                    vendor: Vendor::Claude,
                    model: "claude-sonnet-4-6".into(),
                }],
            },
        )
        .await
        .unwrap();
        assert_eq!(gate.status, PrGateStatus::Escalated);
        assert!(pusher.pushed.lock().unwrap().is_empty());
        server.abort();
    }

    /// Adds one commit on top of the good head (writing `file`) and publishes it
    /// to the mirror's `main`, so `base..new` spans two commits.
    fn extend_good_head(fix: &GitFixture, file: &str, contents: &str) -> String {
        let work = fix._dir.path().join("work");
        git(&work, &["checkout", "main"]);
        std::fs::write(work.join(file), contents).unwrap();
        git(&work, &["add", "."]);
        git(&work, &["commit", "-m", "second"]);
        let sha = git(&work, &["rev-parse", "HEAD"]);
        git(
            &work,
            &["push", fix.mirror.to_str().unwrap(), "HEAD:refs/heads/main"],
        );
        sha
    }

    struct RangeReviewer {
        seen: Mutex<Vec<(ReviewWorktreeRequest, ReviewDiff)>>,
    }
    impl ReviewerExecutor for RangeReviewer {
        fn check_available(&self, _spec: &ReviewerSpec) -> Result<()> {
            Ok(())
        }
        fn run_review<'a>(
            &'a self,
            req: ReviewWorktreeRequest,
        ) -> BoxFuture<'a, Result<ReviewOutcome>> {
            Box::pin(async move {
                let diff = review_diff(&req.worktree_path, &req.base_sha, &req.head_sha)?;
                self.seen.lock().unwrap().push((req, diff));
                Ok(ReviewOutcome {
                    verdict: ReviewVerdict::Approve,
                    comments: "LGTM".to_owned(),
                })
            })
        }
    }

    fn range_request(fix: &GitFixture, head: &str, branch: &str) -> RunPrGateRequest {
        RunPrGateRequest {
            run_id: "run1".into(),
            hub_session_id: None,
            mirror_path: fix.mirror.clone(),
            repo_slug: "org/repo".into(),
            branch: branch.into(),
            base_branch: "main".into(),
            base_sha: fix.base_sha.clone(),
            head_sha: head.to_owned(),
            pr_title: "feat: range".into(),
            pr_body: "body".into(),
            touched_paths: vec!["src/**".into()],
            is_high_risk: false,
            reviewers: vec![ReviewerSpec {
                vendor: Vendor::Claude,
                model: "claude-sonnet-4-6".into(),
            }],
        }
    }

    #[tokio::test]
    async fn reviewer_sees_the_whole_two_commit_pr_range() {
        let fix = setup_git_and_store();
        let head = extend_good_head(&fix, "src/two.rs", "pub fn two() {}\n");

        // The helper covers both commits; `git show <head>` would miss src/lib.rs.
        let ReviewDiff::Complete { text, commits } =
            review_diff(&fix.mirror, &fix.base_sha, &head).unwrap()
        else {
            panic!("small range must be complete");
        };
        assert_eq!(commits, 2);
        assert!(text.contains(&fix.good_head_sha) && text.contains(&head));
        assert!(text.contains("src/lib.rs") && text.contains("src/two.rs"));
        assert!(text.contains("pub fn base() -> i32 { 1 }") && text.contains("pub fn two()"));
        assert!(review_diff(&fix.mirror, &fix.base_sha[..12], &head).is_err());

        let (base_url, gh, _app_state, server) = spawn_fake_github(fix.store.clone()).await;
        let scm = Arc::new(
            GitHubScmClient::new(
                GitHubConfig::new(SecretToken::new("bot-tok"), SecretToken::new("rev-tok"))
                    .with_base_urls(&base_url, format!("{base_url}/graphql")),
            )
            .unwrap(),
        );
        let reviewer = Arc::new(RangeReviewer {
            seen: Mutex::new(Vec::new()),
        });
        let deps = PrGateDeps {
            store: fix.store.clone(),
            scm,
            reviewer: reviewer.clone(),
            worker_revise: None,
            pusher: Arc::new(MockPusher {
                pushed: Mutex::new(Vec::new()),
            }),
        };
        let gate = run_pr_gate(&deps, range_request(&fix, &head, "feat/range"))
            .await
            .unwrap();
        assert_eq!(gate.status, PrGateStatus::AutoMergeArmed);
        let seen = reviewer.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0.base_sha, fix.base_sha);
        assert_eq!(seen[0].0.head_sha, head);
        // Inside the disposable worktree the reviewer got the same full range.
        assert_eq!(
            seen[0].1,
            ReviewDiff::Complete {
                text: text.clone(),
                commits: 2
            }
        );
        assert_eq!(gh.enable_auto_merge_calls.lock().unwrap()[0].1, head);
        server.abort();
    }

    #[tokio::test]
    async fn oversized_pr_range_escalates_instead_of_a_truncated_review() {
        let fix = setup_git_and_store();
        let big: String = (0..MAX_REVIEW_DIFF_BYTES / 16)
            .map(|i| format!("// line {i:08}\n"))
            .collect();
        let head = extend_good_head(&fix, "src/big.rs", &big);
        assert!(matches!(
            review_diff(&fix.mirror, &fix.base_sha, &head).unwrap(),
            ReviewDiff::TooLarge { bytes } if bytes > MAX_REVIEW_DIFF_BYTES
        ));

        let (base_url, gh, _app_state, server) = spawn_fake_github(fix.store.clone()).await;
        let scm = Arc::new(
            GitHubScmClient::new(
                GitHubConfig::new(SecretToken::new("bot-tok"), SecretToken::new("rev-tok"))
                    .with_base_urls(&base_url, format!("{base_url}/graphql")),
            )
            .unwrap(),
        );
        let reviewer = Arc::new(RangeReviewer {
            seen: Mutex::new(Vec::new()),
        });
        let deps = PrGateDeps {
            store: fix.store.clone(),
            scm,
            reviewer: reviewer.clone(),
            worker_revise: None,
            pusher: Arc::new(MockPusher {
                pushed: Mutex::new(Vec::new()),
            }),
        };
        let gate = run_pr_gate(&deps, range_request(&fix, &head, "feat/big"))
            .await
            .unwrap();
        assert_eq!(gate.status, PrGateStatus::Escalated);
        assert!(!gate.auto_merge_armed);
        assert!(reviewer.seen.lock().unwrap().is_empty());
        assert!(gh.submitted_reviews.lock().unwrap().is_empty());
        assert!(gh.enable_auto_merge_calls.lock().unwrap().is_empty());
        server.abort();
    }

    /// Only `available` vendors resolve a provider; every review is recorded.
    struct PartlyAvailableReviewer {
        available: Vec<Vendor>,
        checked: Mutex<Vec<Vendor>>,
        reviews: AtomicUsize,
    }
    impl ReviewerExecutor for PartlyAvailableReviewer {
        fn check_available(&self, spec: &ReviewerSpec) -> Result<()> {
            self.checked.lock().unwrap().push(spec.vendor);
            if self.available.contains(&spec.vendor) {
                Ok(())
            } else {
                Err(Error::Credential)
            }
        }
        fn run_review<'a>(
            &'a self,
            _req: ReviewWorktreeRequest,
        ) -> BoxFuture<'a, Result<ReviewOutcome>> {
            Box::pin(async move {
                self.reviews.fetch_add(1, Ordering::SeqCst);
                Ok(ReviewOutcome {
                    verdict: ReviewVerdict::Approve,
                    comments: "LGTM".to_owned(),
                })
            })
        }
    }

    #[tokio::test]
    async fn unavailable_reviewer_escalates_before_any_push_or_pr() {
        let fix = setup_git_and_store();
        let (base_url, gh, _app_state, server) = spawn_fake_github(fix.store.clone()).await;
        let scm = Arc::new(
            GitHubScmClient::new(
                GitHubConfig::new(SecretToken::new("bot-tok"), SecretToken::new("rev-tok"))
                    .with_base_urls(&base_url, format!("{base_url}/graphql")),
            )
            .unwrap(),
        );
        // High risk needs two reviewers: Claude resolves, Agy does not.
        let reviewer = Arc::new(PartlyAvailableReviewer {
            available: vec![Vendor::Claude],
            checked: Mutex::new(Vec::new()),
            reviews: AtomicUsize::new(0),
        });
        let pusher = Arc::new(MockPusher {
            pushed: Mutex::new(Vec::new()),
        });
        let deps = PrGateDeps {
            store: fix.store.clone(),
            scm,
            reviewer: reviewer.clone(),
            worker_revise: None,
            pusher: pusher.clone(),
        };
        let mut request = range_request(&fix, &fix.good_head_sha, "feat/preflight");
        request.hub_session_id = Some("hub".into());
        request.is_high_risk = true;
        request.reviewers = vec![
            ReviewerSpec {
                vendor: Vendor::Claude,
                model: "claude-sonnet-4-6".into(),
            },
            ReviewerSpec {
                vendor: Vendor::Agy,
                model: "gemini-3-pro".into(),
            },
        ];
        let gate = run_pr_gate(&deps, request).await.unwrap();

        assert_eq!(gate.status, PrGateStatus::Escalated);
        assert_eq!(gate.pr_number, None);
        assert_eq!(
            *reviewer.checked.lock().unwrap(),
            vec![Vendor::Claude, Vendor::Agy]
        );
        assert_eq!(reviewer.reviews.load(Ordering::SeqCst), 0);
        assert!(pusher.pushed.lock().unwrap().is_empty());
        assert!(gh.submitted_reviews.lock().unwrap().is_empty());
        assert!(gh.enable_auto_merge_calls.lock().unwrap().is_empty());
        server.abort();
    }
}
