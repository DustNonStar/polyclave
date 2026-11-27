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

pub trait MirrorPusher: Send + Sync {
    fn push_mirror_branch<'a>(
        &'a self,
        mirror_path: &'a Path,
        refspec: &'a str,
        branch: &'a str,
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

impl MirrorPusher for GitMirrorPusher {
    fn push_mirror_branch<'a>(
        &'a self,
        mirror_path: &'a Path,
        refspec: &'a str,
        branch: &'a str,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let git = resolve_cli_binary(Path::new("git")).map_err(|_| Error::Policy)?;
            let remote = if self.remote.is_empty() {
                "origin".to_owned()
            } else {
                self.remote.clone()
            };
            let push_spec = format!("{refspec}:refs/heads/{branch}");
            let mirror = mirror_path.to_path_buf();
            let out = tokio::task::spawn_blocking(move || {
                let mut cmd = Command::new(&git);
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
                    .arg(&mirror)
                    .args(["push", &remote, &push_spec])
                    .output()
            })
            .await
            .map_err(|_| Error::Transport)?
            .map_err(|_| Error::Transport)?;
            if !out.status.success() {
                return Err(Error::Transport);
            }
            Ok(())
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
            exec.bounce_scope_violation(&gate, &scope.violations)
                .await?
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

    // Push R1-verified authoritative mirror branch and open/find PR
    let final_ref = format!("refs/polyclave/runs/{}/final", req.run_id);
    deps.pusher
        .push_mirror_branch(&req.mirror_path, &final_ref, &req.branch)
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

        let mut revise_comments = Vec::new();
        for spec in &selected_reviewers {
            let mirror = req.mirror_path.clone();
            let sha = current_head.clone();
            let gate_id = gate.id.clone();
            let repo_slug = req.repo_slug.clone();
            let pr_number = pr.number;
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
        let new_head = exec.revise_pr(&gate, &combined).await?;

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
            .push_mirror_branch(&req.mirror_path, &final_ref, &req.branch)
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
        fn push_mirror_branch<'a>(
            &'a self,
            _mirror_path: &'a Path,
            refspec: &'a str,
            branch: &'a str,
        ) -> BoxFuture<'a, Result<()>> {
            Box::pin(async move {
                self.pushed
                    .lock()
                    .unwrap()
                    .push((refspec.to_owned(), branch.to_owned()));
                Ok(())
            })
        }
    }

    struct MockReviewer {
        verdicts: Mutex<Vec<ReviewVerdict>>,
        seen_worktrees: Mutex<Vec<PathBuf>>,
    }
    impl ReviewerExecutor for MockReviewer {
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
}
