use super::*;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use polyclave_core::{mirror::MirrorManager, runner::RunnerConfig};
use polyclave_hub::provider::{FunctionCall, HubReply, ToolCall};
use std::sync::atomic::AtomicUsize;
use tower::ServiceExt;

async fn test_hub() -> (
    tempfile::TempDir,
    Hub,
    broadcast::Receiver<(String, StreamEvent)>,
) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(repo.join("hello.txt"), "line1\nline2\nline3\n").unwrap();
    let store = Store::open(dir.path().join("hub.db")).unwrap();
    let repo_str = repo.to_str().unwrap().to_owned();
    store
        .write(move |db| {
            db.execute_batch(
                "INSERT INTO provider_profiles(id,display_name,kind,models_json,created_at)
                 VALUES('api','API','openai_compat','[\"m1\"]','now');",
            )?;
            db.execute(
                "INSERT INTO sessions(id,name,role,repo_cwd,profile_id,provider_model,created_at)
                 VALUES('hub','Hub','hub',?1,'api','m1','now')",
                [&repo_str],
            )?;
            Ok(())
        })
        .unwrap();
    let runs = RunManager::new(
        store.clone(),
        Arc::new(MirrorManager::new(dir.path()).unwrap()),
    );
    let runner = TurnRunner::new(
        runs.clone(),
        dir.path(),
        "fixture-host".into(),
        RunnerConfig {
            supervisor: "offline".into(),
            mcp_url: "http://127.0.0.1:1/mcp".into(),
            codex: "offline".into(),
            claude: "offline".into(),
            claude_isolation: None,
        },
        |_, _| {},
    )
    .unwrap();
    let (stream, rx) = broadcast::channel(64);
    let hub = Hub::new(
        store,
        runs,
        runner,
        "fixture-host".into(),
        dir.path().to_path_buf(),
        stream,
    )
    .await
    .unwrap();
    (dir, hub, rx)
}

struct MultiStepProvider {
    step: AtomicUsize,
}
impl HubModelProvider for MultiStepProvider {
    fn complete(
        &self,
        request: HubRequest,
        events: broadcast::Sender<ProviderEvent>,
    ) -> polyclave_hub::provider::ProviderFuture<'_> {
        let idx = self.step.fetch_add(1, Ordering::AcqRel);
        Box::pin(async move {
            match idx {
                0 => {
                    let _ = events.send(ProviderEvent::ToolDelta {
                        index: 0,
                        arguments: "{\"rel_path\":\"hello.txt\"".into(),
                    });
                    let _ = events.send(ProviderEvent::Finished);
                    Ok(HubReply {
                        calls: vec![
                            ToolCall {
                                id: "bad_json".into(),
                                kind: "function".into(),
                                function: FunctionCall {
                                    name: "read_repo_file".into(),
                                    arguments: "{not-json".into(),
                                },
                            },
                            ToolCall {
                                id: "good_read".into(),
                                kind: "function".into(),
                                function: FunctionCall {
                                    name: "read_repo_file".into(),
                                    arguments:
                                        r#"{"rel_path":"hello.txt","start_line":2,"end_line":2}"#
                                            .into(),
                                },
                            },
                        ],
                        ..Default::default()
                    })
                }
                _ => {
                    assert!(request
                        .messages
                        .iter()
                        .any(|m| m.role == Role::Tool && m.content.contains("valid JSON object")));
                    assert!(request
                        .messages
                        .iter()
                        .any(|m| m.role == Role::Tool && m.content.contains("line2")));
                    let _ = events.send(ProviderEvent::Text("done".into()));
                    let _ = events.send(ProviderEvent::Finished);
                    Ok(HubReply {
                        text: "done".into(),
                        usage: Some(
                            json!({"total_tokens": 12, "prompt_tokens": 8, "completion_tokens": 4}),
                        ),
                        ..Default::default()
                    })
                }
            }
        })
    }
}

#[tokio::test]
async fn api_turn_recovers_from_invalid_tool_json_forwards_tool_deltas_and_finishes_once() {
    let (_dir, hub, mut rx) = test_hub().await;
    hub.store
        .hub_enqueue("hub", "req-1", "read line 2")
        .unwrap();
    let claim = hub
        .store
        .hub_claim("hub", "fixture-host", &hub.boot)
        .unwrap()
        .unwrap();
    hub.store.hub_authorize(claim.id, None).unwrap();
    let messages = vec![
        ChatMessage::text(Role::System, SYSTEM),
        ChatMessage::text(Role::User, "read line 2"),
    ];
    let (ev_tx, ev_rx) = broadcast::channel(32);
    let relay = tokio::spawn(forward_events(hub.stream.clone(), "hub".into(), ev_rx));
    let stop = Stop::default();
    let provider = MultiStepProvider {
        step: AtomicUsize::new(0),
    };
    hub.api_turn(&provider, false, claim, "hub", messages, &stop, ev_tx)
        .await
        .unwrap();
    let _ = relay.await;
    let mut forwarded = Vec::new();
    while let Ok((_, ev)) = rx.try_recv() {
        forwarded.push(ev);
    }
    assert!(forwarded
        .iter()
        .any(|e| matches!(e, StreamEvent::Tool(d) if d.kind == "tool_delta")));
    assert!(!forwarded
        .iter()
        .any(|e| matches!(e, StreamEvent::Agent(AgentEvent::Finished { .. }))));
    hub.close().await;
}

#[tokio::test]
async fn ticket_tool_creates_and_updates_and_schemas_include_expected_fields() {
    let (_dir, hub, _rx) = test_hub().await;
    let tools = tool_list(true);
    let read_tool = tools
        .iter()
        .find(|t| t["name"] == "read_repo_file")
        .unwrap();
    assert!(read_tool["inputSchema"]["properties"]["start_line"].is_object());
    assert!(read_tool["inputSchema"]["properties"]["end_line"].is_object());
    assert!(tools.iter().any(|t| t["name"] == "memory_propose"));

    hub.store
        .hub_enqueue("hub", "req-t", "make ticket")
        .unwrap();
    let claim = hub
        .store
        .hub_claim("hub", "fixture-host", &hub.boot)
        .unwrap()
        .unwrap();
    hub.store.hub_authorize(claim.id, None).unwrap();
    let scope = ToolScope::new(claim.id, "hub", "hub", &claim.repo);
    let created = hub
        .tool(
            &scope,
            "create_or_update_ticket",
            json!({
                "title": "Initial",
                "mandate": {
                    "goal": "Fix bug",
                    "allowed_paths": ["hello.txt"],
                    "forbidden_actions": ["none"],
                    "acceptance_criteria": ["passes"]
                }
            }),
        )
        .unwrap();
    let ticket_id = created["id"].as_str().unwrap().to_owned();
    let updated = hub
        .tool(
            &scope,
            "create_or_update_ticket",
            json!({"ticket_id": ticket_id, "title": "Updated", "status": "done"}),
        )
        .unwrap();
    assert_eq!(updated["title"], "Updated");
    assert_eq!(updated["status"], "done");
    hub.close().await;
}

#[tokio::test]
async fn http_routes_record_profile_changes_return_json_errors_and_report_usage() {
    let (_dir, hub, _rx) = test_hub().await;
    let app = router(hub.clone());

    let bad = app
        .clone()
        .oneshot(
            Request::post("/sessions/missing/profile")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"profile_id":"api","model":"m1"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::NOT_FOUND);
    let bad_body: Value =
        serde_json::from_slice(&bad.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(bad_body["code"], "not_found");

    let ok = app
        .clone()
        .oneshot(
            Request::post("/sessions/hub/profile")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"profile_id":"api","model":"m1"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::OK);
    let changes = hub.store.changes(None).unwrap();
    assert!(
        matches!(changes, polyclave_store::Changes::Events(ref ev) if ev.iter().any(|e| e.topic == ChangeTopic::Inbox))
    );

    hub.store
        .write(|db| {
            db.execute(
                "INSERT INTO messages(session_id,source,text,metadata_json,provider_label,created_at)
                 VALUES('hub','agent','hi','{\"usage\":{\"total_tokens\":42,\"prompt_tokens\":30,\"completion_tokens\":12,\"cost\":0.01}}','deepseek',strftime('%Y-%m-%dT%H:%M:%fZ','now'))",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    let usage_res = app
        .oneshot(Request::get("/usage").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(usage_res.status(), StatusCode::OK);
    let usage_body: Value =
        serde_json::from_slice(&usage_res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(usage_body[0]["vendor"], "DeepSeek");
    assert_eq!(usage_body[0]["tokens"], 42);
    hub.close().await;
}

#[tokio::test]
async fn hub_close_cancels_active_session_and_emits_shutdown() {
    let (_dir, hub, mut rx) = test_hub().await;
    let stop = Stop::default();
    hub.lifecycle.sessions.lock().unwrap().insert(
        "hub".into(),
        SessionDispatch {
            stop: stop.clone(),
            kicked: false,
        },
    );
    let hub_clone = hub.clone();
    let closer = tokio::spawn(async move { hub_clone.close().await });
    stop.wait().await;
    hub.lifecycle.sessions.lock().unwrap().clear();
    hub.lifecycle.drained.notify_waiters();
    closer.await.unwrap();
    let mut saw_shutdown = false;
    while let Ok((_, ev)) = rx.try_recv() {
        if matches!(ev, StreamEvent::Shutdown) {
            saw_shutdown = true;
        }
    }
    assert!(saw_shutdown);
}

struct MockCouncilProvider {
    summary: String,
    vendor_leak_check: String,
}
impl HubModelProvider for MockCouncilProvider {
    fn complete(
        &self,
        request: HubRequest,
        _events: broadcast::Sender<ProviderEvent>,
    ) -> polyclave_hub::provider::ProviderFuture<'_> {
        let summary = self.summary.clone();
        let leak = self.vendor_leak_check.clone();
        Box::pin(async move {
            assert!(!request.messages.iter().any(|m| m.content.contains(&leak)));
            Ok(HubReply {
                text: json!({
                    "summary": summary,
                    "approach": format!("Approach for {summary}"),
                    "touched_paths": ["src/lib.rs"],
                    "public_interface_changes": false,
                    "new_dependencies": [],
                    "security_sensitive": false,
                    "beyond_request": false,
                    "open_questions": [],
                    "pros": ["simple"],
                    "cons": []
                })
                .to_string(),
                ..Default::default()
            })
        })
    }
}

#[derive(Default)]
struct MockScmState {
    disabled_nodes: Mutex<Vec<String>>,
    enabled_nodes: Mutex<Vec<(String, String)>>,
    submitted_reviews: Mutex<Vec<SubmitReviewRequest>>,
}

struct MockScmClient {
    state: Arc<MockScmState>,
    polled: Option<Arc<Notify>>,
}

impl ScmClient for MockScmClient {
    fn preflight_repo<'a>(
        &'a self,
        _repo_slug: &'a str,
        base_branch: &'a str,
    ) -> BoxFuture<'a, std::result::Result<PreflightReport, ScmError>> {
        let base_branch = base_branch.to_owned();
        Box::pin(async move {
            Ok(PreflightReport {
                bot_login: "polyclave-bot".into(),
                reviewer_login: "polyclave-reviewer".into(),
                default_branch: base_branch,
                branch_protected: true,
                approval_required: true,
                required_approving_review_count: 1,
                squash_merge_allowed: true,
                auto_merge_allowed: true,
                passed: true,
                issues: Vec::new(),
            })
        })
    }

    fn open_or_find_pr<'a>(
        &'a self,
        req: OpenPrRequest,
    ) -> BoxFuture<'a, std::result::Result<PullRequestInfo, ScmError>> {
        Box::pin(async move {
            Ok(PullRequestInfo {
                number: 42,
                node_id: "PR_node_42".into(),
                head_sha: format!("head-for-{}", req.head_branch),
                state: "open".into(),
                html_url: "https://github.com/org/repo/pull/42".into(),
                created: true,
            })
        })
    }

    fn submit_review<'a>(
        &'a self,
        req: SubmitReviewRequest,
    ) -> BoxFuture<'a, std::result::Result<SubmittedReview, ScmError>> {
        let state = self.state.clone();
        Box::pin(async move {
            let commit_id = req.reviewed_sha.clone();
            state.submitted_reviews.lock().unwrap().push(req);
            Ok(SubmittedReview {
                id: 9001,
                commit_id,
                state: "APPROVED".into(),
                html_url: "https://github.com/org/repo/pull/42#pullrequestreview-9001".into(),
            })
        })
    }

    fn enable_auto_merge<'a>(
        &'a self,
        pr_node_id: &'a str,
        expected_head_oid: &'a str,
    ) -> BoxFuture<'a, std::result::Result<(), ScmError>> {
        let state = self.state.clone();
        let node = pr_node_id.to_owned();
        let sha = expected_head_oid.to_owned();
        Box::pin(async move {
            state.enabled_nodes.lock().unwrap().push((node, sha));
            Ok(())
        })
    }

    fn disable_auto_merge<'a>(
        &'a self,
        pr_node_id: &'a str,
    ) -> BoxFuture<'a, std::result::Result<(), ScmError>> {
        let state = self.state.clone();
        let node = pr_node_id.to_owned();
        Box::pin(async move {
            state.disabled_nodes.lock().unwrap().push(node);
            Ok(())
        })
    }

    fn get_pr_state<'a>(
        &'a self,
        _repo_slug: &'a str,
        pr_number: u64,
    ) -> BoxFuture<'a, std::result::Result<PrState, ScmError>> {
        let polled = self.polled.clone();
        Box::pin(async move {
            if let Some(notify) = polled {
                notify.notify_waiters();
            }
            Ok(PrState {
                number: pr_number,
                node_id: format!("PR_node_{pr_number}"),
                state: "open".into(),
                merged: false,
                mergeable: Some(true),
                mergeable_state: Some("clean".into()),
                head_sha: "sha_observed".into(),
                auto_merge_enabled: true,
                html_url: format!("https://github.com/org/repo/pull/{pr_number}"),
            })
        })
    }

    fn list_pr_checks_and_comments<'a>(
        &'a self,
        _repo_slug: &'a str,
        _pr_number: u64,
        _head_sha: &'a str,
    ) -> BoxFuture<'a, std::result::Result<PrChecksAndComments, ScmError>> {
        Box::pin(async move {
            Ok(PrChecksAndComments {
                check_runs: Vec::new(),
                checks_complete: true,
                comments: vec![polyclave_hub::scm::PrCommentInfo {
                    id: 501,
                    author_login: "alice".into(),
                    body: "Looks good to me".into(),
                    created_at: "2026-11-27T00:00:00Z".into(),
                }],
                reviews: Vec::new(),
            })
        })
    }
}

struct MockReviewer;
impl ReviewerExecutor for MockReviewer {
    fn check_available(&self, _spec: &ReviewerSpec) -> HubResult<()> {
        Ok(())
    }
    fn run_review<'a>(
        &'a self,
        req: ReviewWorktreeRequest,
    ) -> BoxFuture<'a, HubResult<ReviewOutcome>> {
        Box::pin(async move {
            assert!(req.worktree_path.exists());
            Ok(ReviewOutcome {
                verdict: polyclave_proto::ReviewVerdict::Approve,
                comments: "Clean diff".into(),
            })
        })
    }
}

struct MockPusher {
    pushed: Mutex<Vec<(String, String)>>,
    /// The base branches the gate asked about before pushing anything.
    base_checked: Mutex<Vec<String>>,
}
impl MockPusher {
    fn new() -> Self {
        Self {
            pushed: Mutex::new(Vec::new()),
            base_checked: Mutex::new(Vec::new()),
        }
    }
}
impl MirrorPusher for MockPusher {
    fn confirm_review_base<'a>(
        &'a self,
        _target: &'a polyclave_hub::publish_origin::PublishTarget,
        _mirror_path: &'a FsPath,
        _base_branch: &'a str,
        _base_sha: &'a str,
    ) -> BoxFuture<'a, HubResult<()>> {
        self.base_checked
            .lock()
            .unwrap()
            .push(_base_branch.to_owned());
        Box::pin(async { Ok(()) })
    }
    fn push_verified_sha<'a>(
        &'a self,
        _target: &'a polyclave_hub::publish_origin::PublishTarget,
        _mirror_path: &'a FsPath,
        sha: &'a str,
        branch: &'a str,
        _expected_remote_tip: Option<&'a str>,
    ) -> BoxFuture<'a, HubResult<()>> {
        self.pushed
            .lock()
            .unwrap()
            .push((sha.to_owned(), branch.to_owned()));
        Box::pin(async { Ok(()) })
    }
}

fn daemon_router(hub: &Hub) -> Router {
    crate::routes::router(
        crate::routes::App {
            store: hub.store.clone(),
            runs: hub.runs.clone(),
            runner: hub.runner.clone(),
            stream: hub.stream.clone(),
            hub: Some(hub.clone()),
        },
        crate::auth::LocalAuth::new("test-only".into(), 4310),
    )
}

fn authed_get(uri: &str) -> Request<Body> {
    Request::get(uri)
        .header("host", "127.0.0.1:4310")
        .header("authorization", "Bearer test-only")
        .body(Body::empty())
        .unwrap()
}

fn authed_post(uri: &str, body: Value) -> Request<Body> {
    Request::post(uri)
        .header("host", "127.0.0.1:4310")
        .header("authorization", "Bearer test-only")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn git_cmd(dir: &FsPath, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
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

/// Builds a Run whose result comes from the real promotion path: the host
/// mirror admits the Run, the Worker pushes its attempt branch, R1 classifies
/// the tip as Done, and `sync_done` promotes it to `refs/heads/polyclave/<id>`.
fn setup_git_run_fixture(dir: &FsPath, hub: &Hub) -> (String, String, String) {
    let repo = polyclave_os::canonical_real_path(&dir.join("repo")).unwrap();
    git_cmd(&repo, &["init", "-b", "main", "--template="]);
    git_cmd(&repo, &["config", "user.name", "Fixture"]);
    git_cmd(&repo, &["config", "user.email", "fixture@example.invalid"]);
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join("src/lib.rs"), "pub fn v() -> i32 { 1 }\n").unwrap();
    git_cmd(&repo, &["add", "."]);
    git_cmd(&repo, &["commit", "-m", "initial"]);

    let run_id = "ra0000ad1".to_owned();
    let mirrors = MirrorManager::new(dir).unwrap();
    let run_git = mirrors.admit(&repo, &run_id, "main").unwrap();
    let base_sha = run_git.base.clone();
    let attempt = polyclave_core::mirror::AttemptGit {
        run: run_git.clone(),
        number: 1,
    };

    let work = dir.join("work");
    git_cmd(
        dir,
        &["clone", repo.to_str().unwrap(), work.to_str().unwrap()],
    );
    git_cmd(&work, &["config", "user.name", "Worker"]);
    git_cmd(&work, &["config", "user.email", "worker@example.invalid"]);
    std::fs::write(work.join("src/lib.rs"), "pub fn v() -> i32 { 2 }\n").unwrap();
    git_cmd(&work, &["add", "."]);
    git_cmd(
        &work,
        &[
            "commit",
            "-m",
            "change",
            "--trailer",
            &format!("Run-Id: {run_id}"),
        ],
    );
    let tip = git_cmd(&work, &["rev-parse", "HEAD"]);
    git_cmd(
        &work,
        &[
            "push",
            run_git.mirror.to_str().unwrap(),
            &format!("HEAD:{}", attempt.full_ref()),
        ],
    );

    // R1: the accepted receipt SHA, the mirror tip and the trailers agree.
    let verdict = polyclave_core::classify::classify_run(
        polyclave_proto::WriterCheck::Excluded,
        Some(&tip),
        mirrors.reconcile(&attempt),
    );
    assert_eq!(
        verdict.status,
        polyclave_proto::RunStatus::Done,
        "{verdict:?}"
    );
    let head_sha = verdict.result_sha.clone().unwrap();
    mirrors.sync_done(&attempt, &head_sha).unwrap();
    assert_eq!(
        git_cmd(
            &repo,
            &["rev-parse", &format!("refs/heads/polyclave/{run_id}")]
        ),
        head_sha
    );

    let repo_str = repo.to_str().unwrap().to_owned();
    let mirror_str = run_git.mirror.to_str().unwrap().to_owned();
    let base_clone = base_sha.clone();
    let head_clone = head_sha.clone();
    let run_clone = run_id.clone();
    hub.store
        .write(move |db| {
            db.execute(
                "INSERT INTO sessions(id,name,role,vendor,repo_cwd,profile_id,provider_model,created_at)
                 VALUES('w-gate','Worker','worker','codex',?1,'api','gpt-5','now')",
                [&repo_str],
            )?;
            db.execute(
                "INSERT INTO tickets(id,hub_session_id,repo_cwd,title,mandate_json,source_kind,max_runs,runs_used,status,created_at,updated_at)
                 VALUES('t-gate','hub',?1,'Gate test','{\"goal\":\"g\",\"allowed_paths\":[\"src/**\"],\"forbidden_actions\":[],\"acceptance_criteria\":[]}','explicit_user',4,1,'active','now','now')",
                [&repo_str],
            )?;
            db.execute(
                "INSERT INTO runs(id,ticket_id,hub_session_id,worker_session_id,request_id,repo_cwd,vendor,model,instructions,mirror_remote,base_branch,base_sha,observed_sha,result_sha,status,accepted_attempt_id,result_source,created_at,updated_at)
                 VALUES(?1,'t-gate','hub','w-gate','req-gate',?2,'codex','gpt-5','do it',?3,'main',?4,?5,?5,'done',71,'agent','now','now')",
                params![run_clone, repo_str, mirror_str, base_clone, head_clone],
            )?;
            // The provenance the gate requires: the accepted Attempt, the turn
            // that reported it, and R1's accepted receipt for exactly this SHA.
            db.execute(
                "INSERT INTO attempts(id,run_id,attempt_no,branch,worktree_path,status,created_at,ended_at)
                 VALUES(71,?1,1,?2,?3,'completed','now','now')",
                params![run_clone, format!("agent/{run_clone}/a1"), repo_str],
            )?;
            db.execute(
                "INSERT INTO turns(id,session_id,run_id,attempt_id,mode,status,phase,host_instance_id,boot_epoch,mcp_token,started_at,ended_at)
                 VALUES(71,'w-gate',?1,71,'start','completed','authorized','host','boot','tok-71','now','now')",
                [&run_clone],
            )?;
            db.execute(
                "INSERT INTO results(run_id,attempt_id,turn_id,accepted,payload_json,created_at)
                 VALUES(?1,71,71,1,?2,'now')",
                params![run_clone, format!("{{\"sha\":\"{head_clone}\",\"tests\":null}}")],
            )?;
            polyclave_store::insert_run_origin_snapshot(
                db,
                &run_clone,
                &polyclave_store::OriginSnapshot {
                    urls: vec!["https://github.com/org/repo.git".into()],
                    push_urls: vec![],
                },
            )?;
            Ok(())
        })
        .unwrap();
    (run_id, base_sha, head_sha)
}

#[tokio::test]
async fn pr_gate_refuses_a_run_with_only_an_observed_sha() {
    let (dir, hub, _rx) = test_hub().await;
    let (run_id, base_sha, head_sha) = setup_git_run_fixture(dir.path(), &hub);
    // A second Run of the same ticket whose tip was observed but never verified.
    let mirror: String = hub
        .store
        .read({
            let run_id = run_id.clone();
            move |db| {
                Ok(db.query_row(
                    "SELECT mirror_remote FROM runs WHERE id=?1",
                    [&run_id],
                    |r| r.get(0),
                )?)
            }
        })
        .unwrap();
    let repo = hub.runs.get(&run_id).unwrap().repo_cwd;
    let observed = head_sha.clone();
    let base = base_sha.clone();
    hub.store
        .write(move |db| {
            db.execute(
                "INSERT INTO sessions(id,name,role,vendor,repo_cwd,profile_id,provider_model,created_at)
                 VALUES('w-gate-2','Worker 2','worker','codex',?1,'api','gpt-5','now')",
                [&repo],
            )?;
            db.execute(
                "INSERT INTO runs(id,ticket_id,hub_session_id,worker_session_id,request_id,repo_cwd,vendor,model,instructions,mirror_remote,base_branch,base_sha,observed_sha,status,created_at,updated_at)
                 VALUES('ra0000ad2','t-gate','hub','w-gate-2','req-gate-2',?1,'codex','gpt-5','do it',?2,'main',?3,?4,'needs_review','now','now')",
                params![repo, mirror, base, observed],
            )?;
            Ok(())
        })
        .unwrap();

    // The revise executor's SHA source refuses an observed-only Run.
    let unverified = hub.runs.get("ra0000ad2").unwrap();
    assert_eq!(unverified.observed_sha.as_deref(), Some(head_sha.as_str()));
    assert!(verified_result_sha(&hub.runs, &unverified).is_err());
    assert_eq!(
        verified_result_sha(&hub.runs, &hub.runs.get(&run_id).unwrap()).unwrap(),
        head_sha
    );

    let pusher = Arc::new(MockPusher::new());
    hub.set_scm(Arc::new(MockScmClient {
        state: Arc::new(MockScmState::default()),
        polled: None,
    }));
    hub.set_reviewer(Arc::new(MockReviewer));
    hub.set_pusher(pusher.clone());
    hub.set_default_reviewers(vec![ReviewerSpec {
        vendor: Vendor::Claude,
        model: "claude-sonnet-4-6".into(),
    }]);
    let app = daemon_router(&hub);
    // Neither the observed tip nor a caller-supplied head_sha can stand in for
    // a verified result.
    for body in [
        json!({"repo_slug": "org/repo", "branch": "feat/unverified"}),
        json!({"repo_slug": "org/repo", "branch": "feat/unverified", "head_sha": head_sha}),
    ] {
        let res = app
            .clone()
            .oneshot(authed_post("/api/pr-gates/ra0000ad2/run", body))
            .await
            .unwrap();
        assert!(res.status().is_client_error(), "{}", res.status());
    }
    let res = app
        .clone()
        .oneshot(authed_post(
            &format!("/api/pr-gates/{run_id}/run"),
            json!({"repo_slug": "org/repo", "branch": "feat/other", "head_sha": base_sha}),
        ))
        .await
        .unwrap();
    assert!(res.status().is_client_error(), "{}", res.status());
    assert!(pusher.pushed.lock().unwrap().is_empty());
    hub.close().await;
}

/// A Run whose Worker pushed an attempt branch R1 never accepted: the commit
/// carries no `Run-Id` trailer, there is no `submit_result` receipt, and the
/// Run sits in `needs_review`.
fn setup_unreceipted_run(dir: &FsPath, hub: &Hub, run_id: &str, attempt_id: i64) -> String {
    let repo = polyclave_os::canonical_real_path(&dir.join("repo")).unwrap();
    let mirrors = MirrorManager::new(dir).unwrap();
    let run_git = mirrors.admit(&repo, run_id, "main").unwrap();
    let attempt = polyclave_core::mirror::AttemptGit {
        run: run_git.clone(),
        number: 1,
    };
    let work = dir.join(format!("work-{run_id}"));
    git_cmd(
        dir,
        &["clone", repo.to_str().unwrap(), work.to_str().unwrap()],
    );
    git_cmd(&work, &["config", "user.name", "Worker"]);
    git_cmd(&work, &["config", "user.email", "worker@example.invalid"]);
    std::fs::write(work.join("src/lib.rs"), "pub fn v() -> i32 { 3 }\n").unwrap();
    git_cmd(&work, &["add", "."]);
    git_cmd(&work, &["commit", "-m", "untrailed change"]);
    let tip = git_cmd(&work, &["rev-parse", "HEAD"]);
    git_cmd(
        &work,
        &[
            "push",
            run_git.mirror.to_str().unwrap(),
            &format!("HEAD:{}", attempt.full_ref()),
        ],
    );
    // R1 refuses this tip; nothing accepted it.
    match mirrors.reconcile(&attempt) {
        polyclave_core::classify::BranchCheck::Present {
            problem: Some(_), ..
        } => (),
        other => panic!("expected an unexplained tip, got {other:?}"),
    }

    let repo_str = repo.to_str().unwrap().to_owned();
    let mirror_str = run_git.mirror.to_str().unwrap().to_owned();
    let base = run_git.base.clone();
    let tip_row = tip.clone();
    let id = run_id.to_owned();
    hub.store
        .write(move |db| {
            db.execute(
                "INSERT INTO sessions(id,name,role,vendor,repo_cwd,profile_id,provider_model,created_at)
                 VALUES(?1,?1,'worker','codex',?2,'api','gpt-5','now')",
                params![format!("w-{id}"), repo_str],
            )?;
            db.execute(
                "INSERT INTO runs(id,ticket_id,hub_session_id,worker_session_id,request_id,repo_cwd,vendor,model,instructions,mirror_remote,base_branch,base_sha,observed_sha,status,current_attempt,created_at,updated_at)
                 VALUES(?1,'t-gate','hub',?2,?3,?4,'codex','gpt-5','do it',?5,'main',?6,?7,'needs_review',1,'now','now')",
                params![
                    id.clone(),
                    format!("w-{id}"),
                    format!("req-{id}"),
                    repo_str,
                    mirror_str,
                    base,
                    tip_row
                ],
            )?;
            db.execute(
                "INSERT INTO attempts(id,run_id,attempt_no,branch,worktree_path,status,created_at,ended_at)
                 VALUES(?1,?2,1,?3,?4,'interrupted','now','now')",
                params![
                    attempt_id,
                    id,
                    format!("agent/{id}/a1"),
                    format!("work-{id}")
                ],
            )?;
            Ok(())
        })
        .unwrap();
    tip
}

/// The workspace builds `polyclave-fake-agent` for the core suite; it lives in
/// the same target directory as this test binary.
fn fake_agent_binary() -> std::path::PathBuf {
    let exe = std::env::current_exe().unwrap();
    let path = exe
        .parent()
        .and_then(|p| p.parent())
        .map(|p| {
            p.join(format!(
                "polyclave-fake-agent{}",
                std::env::consts::EXE_SUFFIX
            ))
        })
        .unwrap();
    assert!(
        path.is_file(),
        "the offline Worker fixture is missing: {}",
        path.display()
    );
    path
}

/// A Hub whose TurnRunner really spawns the offline Worker fixture and whose MCP
/// server really accepts its receipt: the whole A -> B chain, no test double.
async fn worker_hub() -> (
    tempfile::TempDir,
    Hub,
    String,
    tokio::task::JoinHandle<std::io::Result<()>>,
) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git_cmd(&repo, &["init", "-b", "main", "--template="]);
    git_cmd(&repo, &["config", "user.name", "Fixture"]);
    git_cmd(&repo, &["config", "user.email", "fixture@example.invalid"]);
    std::fs::write(repo.join("README.md"), "base\n").unwrap();
    git_cmd(&repo, &["add", "."]);
    git_cmd(&repo, &["commit", "-m", "base"]);
    let repo = polyclave_os::canonical_real_path(&repo).unwrap();
    let repo_str = repo.to_str().unwrap().to_owned();
    let repo_row = repo_str.clone();
    let store = Store::open(dir.path().join("hub.db")).unwrap();
    store
        .write(move |db| {
            db.execute(
                "INSERT INTO provider_profiles(id,display_name,kind,models_json,created_at)
                 VALUES('p','Fixture','codex_cli','[]','now')",
                [],
            )?;
            db.execute(
                "INSERT INTO sessions(id,name,role,repo_cwd,profile_id,provider_model,created_at)
                 VALUES('hub','Hub','hub',?1,'p','fixture','now')",
                [&repo_row],
            )?;
            Ok(())
        })
        .unwrap();
    let runs = RunManager::new(
        store.clone(),
        Arc::new(MirrorManager::new(dir.path()).unwrap()),
    );
    let host = "revision-suite".to_owned();
    let (addr, server) =
        polyclave_adapters::mcp::Mcp::new(store.clone(), host.clone(), Arc::new(runs.clone()))
            .listen()
            .await
            .unwrap();
    let fake = fake_agent_binary();
    let runner = TurnRunner::new(
        runs.clone(),
        dir.path(),
        host,
        RunnerConfig {
            supervisor: fake.clone(),
            mcp_url: format!("http://{addr}/mcp"),
            codex: fake,
            claude: "unavailable".into(),
            claude_isolation: None,
        },
        |_, _| {},
    )
    .unwrap();
    let (stream, _rx) = broadcast::channel(64);
    let hub = Hub::new(
        store,
        runs,
        runner,
        "fixture-host".into(),
        dir.path().to_path_buf(),
        stream,
    )
    .await
    .unwrap();
    (dir, hub, repo_str, server)
}

/// `(id, attempt_no)` for every Attempt of a Run, in order.
fn attempts_of(hub: &Hub, run_id: &str) -> Vec<(i64, u32)> {
    hub.store
        .read({
            let run_id = run_id.to_owned();
            move |db| {
                let mut stmt = db.prepare(
                    "SELECT id,attempt_no FROM attempts WHERE run_id=?1 ORDER BY attempt_no",
                )?;
                let rows = stmt
                    .query_map([run_id], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            }
        })
        .unwrap()
}

/// One Worker turn through the production scheduler.
async fn run_turn(
    hub: &Hub,
    run_id: &str,
    mode: TurnMode,
    prompt: &str,
) -> polyclave_proto::RunDto {
    let job = hub
        .runner
        .schedule(run_id, mode, prompt.to_owned())
        .unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(60),
        tokio::task::spawn_blocking(move || job.join().unwrap()),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap()
}

/// Item 2.3: a PR-gate revision is a new Attempt branched from the verified
/// SHA, and it needs its own receipt. Without one, the previous SHA and the
/// previous approval are not reused.
#[tokio::test]
async fn pr_gate_revision_turns_a_verified_run_into_a_new_attempt_with_its_own_receipt() {
    let (dir, hub, repo, server) = worker_hub().await;
    let message = hub
        .store
        .hub_enqueue("hub", "req-revision", "change the loader")
        .unwrap();
    let ticket = hub
        .store
        .hub_ticket(
            "hub",
            message.id,
            "Revision",
            json!({
                "goal": "change the loader",
                "allowed_paths": ["result-*.txt"],
                "forbidden_actions": [],
                "acceptance_criteria": []
            }),
            4,
        )
        .unwrap();
    let (run, _) = hub
        .runs
        .accept_local_worker(polyclave_core::runs::RunRequest {
            hub_session_id: "hub".into(),
            request_id: "req-revision".into(),
            vendor: Vendor::Codex,
            model: None,
            repo_cwd: std::path::PathBuf::from(&repo),
            instructions: "Fixture: first delivery".into(),
            base_branch: "main".into(),
            ticket_id: Some(ticket.id.clone()),
        })
        .unwrap();
    let run_id = run.id.clone();
    let a = run_turn(&hub, &run_id, TurnMode::Start, "Fixture: first delivery").await;
    assert_eq!(a.status, polyclave_proto::RunStatus::Done, "{a:?}");
    let a_sha = a.result_sha.clone().expect("A is verified");
    assert_eq!(a.current_attempt, 1);

    let gate = hub
        .store
        .create_pr_gate(polyclave_store::NewPrGate {
            run_id: run_id.clone(),
            repo_slug: "org/repo".into(),
            branch: "feat/revision".into(),
            base_sha: a.base_sha.clone(),
            head_sha: a_sha.clone(),
            touched_paths: vec!["result-*.txt".into()],
        })
        .unwrap();

    // The real executor: a new Attempt from A, then a Worker turn in it.
    let executor = HostWorkerReviseExecutor {
        runner: hub.runner.clone(),
    };
    let b_sha = executor
        .revise_pr(&gate, "Please extend the change")
        .await
        .unwrap();
    assert_ne!(b_sha, a_sha, "the revision delivered a new commit");

    let revised = hub.runs.get(&run_id).unwrap();
    assert_eq!(
        revised.status,
        polyclave_proto::RunStatus::Done,
        "{revised:?}"
    );
    assert_eq!(revised.result_sha.as_deref(), Some(b_sha.as_str()));
    assert_eq!(
        revised.result_source,
        Some(polyclave_proto::ResultSource::Agent)
    );
    assert_eq!(revised.current_attempt, 2);
    assert_eq!(
        revised.base_sha, a.base_sha,
        "the reviewed baseline never moves"
    );
    let attempts = attempts_of(&hub, &run_id);
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[1].1, 2);
    assert_eq!(
        hub.store
            .attempt_from_sha(attempts[1].0)
            .unwrap()
            .as_deref(),
        Some(a_sha.as_str()),
        "the revision branched from A"
    );
    assert_eq!(
        revised.accepted_attempt_id,
        Some(attempts[1].0),
        "B has its own accepted receipt, not A's"
    );
    assert_eq!(
        verified_result_sha(&hub.runs, &revised).unwrap(),
        b_sha,
        "the gate publishes B"
    );

    // A revision that submits no receipt never falls back to B, and the Run is
    // left for a human instead.
    let failed = executor
        .revise_pr(&gate, "Fixture: no receipt")
        .await
        .unwrap_err();
    assert!(matches!(failed, Error::Policy), "{failed:?}");
    let stalled = hub.runs.get(&run_id).unwrap();
    assert_eq!(stalled.status, polyclave_proto::RunStatus::NeedsReview);
    assert_eq!(stalled.result_sha, None, "B is not reused as a result");
    // The unpublished tip is a new commit nobody accepted: neither A nor B can
    // stand in for it.
    let observed = stalled.observed_sha.clone().expect("the pushed tip");
    assert_ne!(observed, a_sha);
    assert_ne!(observed, b_sha);
    assert!(verified_result_sha(&hub.runs, &stalled).is_err());
    let attempts = attempts_of(&hub, &run_id);
    assert_eq!(attempts.len(), 3);
    assert_eq!(
        hub.store
            .attempt_from_sha(attempts[2].0)
            .unwrap()
            .as_deref(),
        Some(b_sha.as_str())
    );
    let receipts: i64 = hub
        .store
        .read({
            let attempt = attempts[2].0;
            move |db| {
                Ok(db.query_row(
                    "SELECT COUNT(*) FROM results WHERE attempt_id=?1 AND accepted=1",
                    [attempt],
                    |r| r.get(0),
                )?)
            }
        })
        .unwrap();
    assert_eq!(receipts, 0, "the third Attempt has no accepted receipt");

    server.abort();
    hub.close().await;
    drop(dir);
}

/// Item 2.6: the daemon starts with an empty database; two projects are
/// registered afterwards. Each SCM event goes to the Hub that owns the Run, wakes
/// that Hub, and is stored with the host-observation identity.
#[tokio::test]
async fn scm_events_route_to_the_owning_hub_and_wake_it() {
    let (dir, hub, _rx) = test_hub().await;
    let mut owners = Vec::new();
    for (suffix, slug) in [("a", "org/repo-a"), ("b", "org/repo-b")] {
        let hub_id = format!("hub-{suffix}");
        let worker = format!("w-{suffix}");
        let ticket = format!("t-{suffix}");
        let run = format!("run-{suffix}");
        let repo = dir.path().join(format!("repo-{suffix}"));
        let repo_str = repo.to_str().unwrap().to_owned();
        let (hub_id_row, worker_row, ticket_row, run_row) =
            (hub_id.clone(), worker.clone(), ticket.clone(), run.clone());
        let store = hub.store.clone();
        let gate_store = store.clone();
        store
            .write(move |db| {
                db.execute(
                    "INSERT INTO sessions(id,name,role,repo_cwd,profile_id,provider_model,created_at)
                     VALUES(?1,?1,'hub',?2,'api','gpt-5','now')",
                    params![hub_id_row, repo_str],
                )?;
                db.execute(
                    "INSERT INTO sessions(id,name,role,repo_cwd,profile_id,provider_model,created_at)
                     VALUES(?1,?1,'worker',?2,'api','gpt-5','now')",
                    params![worker_row, repo_str],
                )?;
                db.execute(
                    "INSERT INTO tickets(id,hub_session_id,repo_cwd,title,mandate_json,source_kind,max_runs,runs_used,status,created_at,updated_at)
                     VALUES(?1,?2,?3,'Project','{\"goal\":\"g\",\"allowed_paths\":[\"src/**\"]}','explicit_user',4,1,'active','now','now')",
                    params![ticket_row, hub_id_row, repo_str],
                )?;
                db.execute(
                    "INSERT INTO runs(id,ticket_id,hub_session_id,worker_session_id,request_id,repo_cwd,vendor,model,instructions,mirror_remote,base_branch,base_sha,status,created_at,updated_at)
                     VALUES(?1,?2,?3,?4,'req',?5,'codex','gpt-5','do it','origin','main','base','done','now','now')",
                    params![run_row, ticket_row, hub_id_row, worker_row, repo_str],
                )?;
                Ok(())
            })
            .unwrap();
        let gate = gate_store
            .create_pr_gate(polyclave_store::NewPrGate {
                run_id: run.clone(),
                repo_slug: slug.to_owned(),
                branch: "feat/x".into(),
                base_sha: "base".into(),
                head_sha: "sha_observed".into(),
                touched_paths: vec!["src/**".into()],
            })
            .unwrap();
        gate_store
            .record_pr_opened(&gate.id, 42, "https://github.com/org/repo/pull/42")
            .unwrap();
        owners.push((hub_id, slug));
    }

    let scm = Arc::new(MockScmClient {
        state: Arc::new(MockScmState::default()),
        polled: None,
    });
    // The production observer: it wakes the owning Hub through `Hub::kick`.
    let events = hub.scm_observer(scm).poll_once().await.unwrap();
    assert_eq!(events.len(), 2, "{events:?}");

    for (owner, slug) in &owners {
        let other = if owner == "hub-a" { "hub-b" } else { "hub-a" };
        // Being woken means the Hub driver claimed the queued message.
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let claimed = hub
                    .store
                    .hub_messages(owner)
                    .unwrap()
                    .iter()
                    .all(|m| m.turn_id.is_some());
                if claimed {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{owner} was not woken"));
        let messages = hub.store.hub_messages(owner).unwrap();
        assert_eq!(messages.len(), 1, "{owner}: {messages:?}");
        assert_eq!(messages[0].source, "host");
        assert_eq!(
            messages[0].metadata,
            Some(json!({"kind": polyclave_store::HOST_OBSERVATION_KIND}))
        );
        assert!(messages[0].text.contains(slug), "{messages:?}");
        // The other project's Hub never sees this event, and is never woken by it.
        assert!(hub
            .store
            .hub_messages(other)
            .unwrap()
            .iter()
            .all(|m| !m.text.contains(slug)));
        assert_eq!(
            hub.store.hub_messages(other).unwrap().len(),
            1,
            "{other} has only its own event"
        );
    }
    hub.close().await;
}

/// Item 2.2: the reviewed base and the authorized path scope come from the
/// persisted Run and its Ticket mandate. A caller that tries to widen either is
/// refused before anything is pushed, over HTTP and through the MCP tool.
#[tokio::test]
async fn pr_gate_refuses_a_caller_that_tries_to_widen_the_reviewed_scope() {
    let (dir, hub, _rx) = test_hub().await;
    let (run_id, base_sha, head_sha) = setup_git_run_fixture(dir.path(), &hub);
    let pusher = Arc::new(MockPusher::new());
    let scm_state = Arc::new(MockScmState::default());
    hub.set_scm(Arc::new(MockScmClient {
        state: scm_state.clone(),
        polled: None,
    }));
    hub.set_reviewer(Arc::new(MockReviewer));
    hub.set_pusher(pusher.clone());
    hub.set_default_reviewers(vec![ReviewerSpec {
        vendor: Vendor::Claude,
        model: "claude-sonnet-4-6".into(),
    }]);
    let app = daemon_router(&hub);

    // Every one of these fields used to be a caller override.
    for body in [
        json!({"repo_slug": "org/repo", "branch": "feat/widen", "touched_paths": ["**"]}),
        json!({"repo_slug": "org/repo", "branch": "feat/widen", "base_sha": base_sha}),
        json!({"repo_slug": "org/repo", "branch": "feat/widen", "base_branch": "release"}),
        // An empty reviewed range is refused on its own.
        json!({"repo_slug": "org/repo", "branch": "feat/widen", "base_sha": head_sha}),
    ] {
        let res = app
            .clone()
            .oneshot(authed_post(&format!("/api/pr-gates/{run_id}/run"), body))
            .await
            .unwrap();
        assert!(res.status().is_client_error(), "{}", res.status());
    }

    // The tool refuses the same attempt, and the schema no longer offers them.
    let schema = tool_list(true);
    let gate_tool = schema
        .iter()
        .find(|t| t["name"] == "run_pr_gate")
        .expect("run_pr_gate tool");
    for field in ["base_sha", "base_branch", "touched_paths"] {
        assert!(
            gate_tool["parameters"]["properties"].get(field).is_none(),
            "{field} is not a caller choice"
        );
    }
    hub.store
        .hub_enqueue("hub", "req-widen", "widen scope")
        .unwrap();
    let claim = hub
        .store
        .hub_claim("hub", "fixture-host", &hub.boot)
        .unwrap()
        .unwrap();
    hub.store.hub_authorize(claim.id, None).unwrap();
    let scope = ToolScope::new(claim.id, "hub", "hub", &claim.repo);
    assert!(matches!(
        hub.call_tool(
            &scope,
            "run_pr_gate",
            json!({
                "run_id": run_id,
                "repo_slug": "org/repo",
                "branch": "feat/widen",
                "pr_title": "Widen",
                "touched_paths": ["**"]
            }),
        )
        .await,
        Err(Error::Policy)
    ));

    // Nothing was published, reviewed or merged, and no gate row was created.
    assert!(pusher.pushed.lock().unwrap().is_empty());
    assert!(pusher.base_checked.lock().unwrap().is_empty());
    assert!(scm_state.submitted_reviews.lock().unwrap().is_empty());
    assert!(scm_state.enabled_nodes.lock().unwrap().is_empty());
    assert!(hub.store.list_pr_gates().unwrap().is_empty());
    hub.close().await;
}

/// Item 2.1: a person may still accept a tip by hand, but that accept stores no
/// receipt, so it can never authorize review, push or auto-merge.
#[tokio::test]
async fn pr_gate_refuses_a_hand_accepted_tip_that_r1_never_receipted() {
    let (dir, hub, _rx) = test_hub().await;
    let (_verified_run, _base, _head) = setup_git_run_fixture(dir.path(), &hub);
    let run_id = "ra0000ad3";
    let tip = setup_unreceipted_run(dir.path(), &hub, run_id, 72);

    // The production path for a human decision: accept the mirror tip by hand.
    let accepted = hub
        .runs
        .resolve_seen(
            run_id,
            polyclave_core::runs::Resolution::Accept,
            "I read the diff and accept this by hand",
            Some(&tip),
        )
        .unwrap();
    assert_eq!(accepted.status, polyclave_proto::RunStatus::Done);
    assert_eq!(
        accepted.result_source,
        Some(polyclave_proto::ResultSource::User)
    );
    assert_eq!(accepted.result_sha.as_deref(), Some(tip.as_str()));
    let receipts: i64 = hub
        .store
        .read({
            let run_id = run_id.to_owned();
            move |db| {
                Ok(db.query_row(
                    "SELECT COUNT(*) FROM results WHERE run_id=?1 AND accepted=1",
                    [&run_id],
                    |r| r.get(0),
                )?)
            }
        })
        .unwrap();
    assert_eq!(receipts, 0, "a hand accept stores no receipt");

    // The gate's SHA source refuses it even though `result_sha` is set.
    assert!(verified_result_sha(&hub.runs, &accepted).is_err());

    let pusher = Arc::new(MockPusher::new());
    let scm_state = Arc::new(MockScmState::default());
    hub.set_scm(Arc::new(MockScmClient {
        state: scm_state.clone(),
        polled: None,
    }));
    hub.set_reviewer(Arc::new(MockReviewer));
    hub.set_pusher(pusher.clone());
    hub.set_default_reviewers(vec![ReviewerSpec {
        vendor: Vendor::Claude,
        model: "claude-sonnet-4-6".into(),
    }]);
    let app = daemon_router(&hub);
    let res = app
        .clone()
        .oneshot(authed_post(
            &format!("/api/pr-gates/{run_id}/run"),
            json!({"repo_slug": "org/repo", "branch": "feat/hand-accepted", "head_sha": tip}),
        ))
        .await
        .unwrap();
    assert!(res.status().is_client_error(), "{}", res.status());
    assert!(
        pusher.pushed.lock().unwrap().is_empty(),
        "nothing was pushed"
    );
    assert!(scm_state.submitted_reviews.lock().unwrap().is_empty());
    assert!(scm_state.enabled_nodes.lock().unwrap().is_empty());
    assert!(hub.store.list_pr_gates().unwrap().is_empty());
    hub.close().await;
}

#[tokio::test]
async fn host_reviewer_preflight_refuses_a_vendor_without_cli_or_api_profile() {
    let (dir, hub, _rx) = test_hub().await;
    let host = HostReviewerExecutor {
        store: hub.store.clone(),
        state: dir.path().to_path_buf(),
    };
    let spec = |vendor| ReviewerSpec {
        vendor,
        model: "m1".into(),
    };
    // Agy and DeepSeek have no one-shot CLI reviewer, and the fixture's only API
    // profile has no endpoint or keyring account, so neither resolves.
    for vendor in [Vendor::Agy, Vendor::Dsh] {
        assert!(matches!(
            host.check_available(&spec(vendor)),
            Err(Error::Credential)
        ));
    }
    // A configured API profile for Agy makes an Agy reviewer available.
    hub.store
        .write(|db| {
            db.execute_batch(
                "INSERT INTO provider_profiles(id,display_name,kind,base_url,keyring_account,models_json,created_at)
                 VALUES('agy-review','Agy','openai_compat','https://generativelanguage.googleapis.com/v1beta/openai','agy-account','[\"m1\"]','now');",
            )?;
            Ok(())
        })
        .unwrap();
    host.check_available(&spec(Vendor::Agy)).unwrap();
    assert!(host.check_available(&spec(Vendor::Dsh)).is_err());
    hub.close().await;
}

#[tokio::test]
async fn councils_routes_enforce_blind_projection_and_reveal_after_resolve() {
    let (_dir, hub, _rx) = test_hub().await;
    hub.set_council_participants(vec![
        CouncilParticipant {
            vendor: Vendor::Codex,
            model: "gpt-5.3-codex".into(),
            provider: Arc::new(MockCouncilProvider {
                summary: "Alpha architecture".into(),
                vendor_leak_check: "Beta architecture".into(),
            }),
        },
        CouncilParticipant {
            vendor: Vendor::Claude,
            model: "claude-sonnet-4-6".into(),
            provider: Arc::new(MockCouncilProvider {
                summary: "Beta architecture".into(),
                vendor_leak_check: "Alpha architecture".into(),
            }),
        },
    ]);
    let app = daemon_router(&hub);

    // Auth check on new endpoint
    let unauth = app
        .clone()
        .oneshot(
            Request::get("/api/councils")
                .header("host", "127.0.0.1:4310")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(unauth.status(), StatusCode::UNAUTHORIZED);

    let created = app
        .clone()
        .oneshot(authed_post(
            "/api/councils",
            json!({
                "hub_session_id": "hub",
                "level": "direction",
                "prompt": "Choose architecture"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&created.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(body["status"], "awaiting_user");
    assert_eq!(body["proposals"].as_array().unwrap().len(), 2);
    for p in body["proposals"].as_array().unwrap() {
        assert!(p["label"].as_str().unwrap().starts_with("方案"));
        assert!(p["vendor"].is_null());
        assert!(p["model"].is_null());
    }
    let council_id = body["id"].as_str().unwrap().to_owned();
    let chosen_label = body["proposals"][0]["label"].as_str().unwrap().to_owned();

    // Direction-level council never auto-starts dev
    assert!(hub.runs.list().unwrap().is_empty());

    // GET /api/councils preserves blind stripping
    let listed = app
        .clone()
        .oneshot(authed_get("/api/councils"))
        .await
        .unwrap();
    assert_eq!(listed.status(), StatusCode::OK);
    let list_body: Value =
        serde_json::from_slice(&listed.into_body().collect().await.unwrap().to_bytes()).unwrap();
    for p in list_body[0]["proposals"].as_array().unwrap() {
        assert!(p["vendor"].is_null());
        assert!(p["model"].is_null());
    }

    // POST /api/councils/{id}/resolve reveals vendor & model
    let resolved = app
        .clone()
        .oneshot(authed_post(
            &format!("/api/councils/{council_id}/resolve"),
            json!({
                "chosen_label": chosen_label,
                "decision_note": "approved"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resolved.status(), StatusCode::OK);
    let resolved_body: Value =
        serde_json::from_slice(&resolved.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(resolved_body["status"], "decided");
    for p in resolved_body["proposals"].as_array().unwrap() {
        assert!(p["vendor"].is_string());
        assert!(p["model"].is_string());
    }

    hub.close().await;
}

#[tokio::test]
async fn pr_gate_routes_run_gate_and_expose_reviews_scope_and_auto_merge() {
    let (dir, hub, _rx) = test_hub().await;
    let (run_id, _base, head_sha) = setup_git_run_fixture(dir.path(), &hub);
    let scm_state = Arc::new(MockScmState::default());
    let scm = Arc::new(MockScmClient {
        state: scm_state.clone(),
        polled: None,
    });
    let pusher = Arc::new(MockPusher::new());
    hub.set_scm(scm);
    hub.set_reviewer(Arc::new(MockReviewer));
    hub.set_pusher(pusher.clone());
    hub.set_default_reviewers(vec![ReviewerSpec {
        vendor: Vendor::Claude,
        model: "claude-sonnet-4-6".into(),
    }]);

    let app = daemon_router(&hub);
    let run_res = app
        .clone()
        .oneshot(authed_post(
            &format!("/api/pr-gates/{run_id}/run"),
            json!({
                "repo_slug": "org/repo",
                "branch": "feat/gate-1",
                "pr_title": "Add v2"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(run_res.status(), StatusCode::OK);
    let gate: Value =
        serde_json::from_slice(&run_res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(gate["status"], "auto_merge_armed");
    assert_eq!(gate["auto_merge_armed"], true);
    assert_eq!(gate["pr_number"], 42);
    assert_eq!(gate["reviews"].as_array().unwrap().len(), 1);
    assert_eq!(gate["reviews"][0]["reviewed_sha"], head_sha);
    assert!(gate["scope_violations"].as_array().unwrap().is_empty());

    let gate_id = gate["id"].as_str().unwrap().to_owned();
    let list_res = app
        .clone()
        .oneshot(authed_get("/api/pr-gates"))
        .await
        .unwrap();
    assert_eq!(list_res.status(), StatusCode::OK);
    let list_body: Value =
        serde_json::from_slice(&list_res.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(list_body.as_array().unwrap().len(), 1);

    let detail_res = app
        .oneshot(authed_get(&format!("/api/pr-gates/{gate_id}")))
        .await
        .unwrap();
    assert_eq!(detail_res.status(), StatusCode::OK);
    let detail_body: Value =
        serde_json::from_slice(&detail_res.into_body().collect().await.unwrap().to_bytes())
            .unwrap();
    assert_eq!(detail_body["id"], gate_id);
    assert_eq!(detail_body["auto_merge_armed"], true);
    assert_eq!(scm_state.enabled_nodes.lock().unwrap().len(), 1);
    assert_eq!(pusher.pushed.lock().unwrap().len(), 1);

    hub.close().await;
}

#[tokio::test]
async fn pause_and_resume_routes_handle_missing_token_revoke_armed_prs_and_block_with_409() {
    let (dir, hub, _rx) = test_hub().await;
    let (run_id, _base, _head) = setup_git_run_fixture(dir.path(), &hub);
    let app = daemon_router(&hub);

    // 1. Initially unpaused
    let initial = app.clone().oneshot(authed_get("/api/pause")).await.unwrap();
    assert_eq!(initial.status(), StatusCode::OK);
    let initial_body: Value =
        serde_json::from_slice(&initial.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(initial_body["paused"], false);

    // 2. Pause without GitHub token still persists pause and returns state cleanly
    let paused_no_tok = app
        .clone()
        .oneshot(authed_post("/api/pause", json!({ "reason": "incident" })))
        .await
        .unwrap();
    assert_eq!(paused_no_tok.status(), StatusCode::OK);
    let paused_body: Value = serde_json::from_slice(
        &paused_no_tok
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes(),
    )
    .unwrap();
    assert_eq!(paused_body["paused"], true);
    assert_eq!(paused_body["reason"], "incident");

    // 3. While paused, Run create, Run retry, Council start, and PR Gate run all return 409 Conflict
    let run_create = app
        .clone()
        .oneshot(authed_post(
            "/api/runs",
            json!({
                "hub_session_id": "hub",
                "request_id": "blocked-req",
                "vendor": "codex",
                "model": null,
                "repo_cwd": dir.path().join("repo"),
                "instructions": "should fail",
                "base_branch": "main",
                "ticket_id": null
            }),
        ))
        .await
        .unwrap();
    assert_eq!(run_create.status(), StatusCode::CONFLICT);
    let err_run: Value =
        serde_json::from_slice(&run_create.into_body().collect().await.unwrap().to_bytes())
            .unwrap();
    assert_eq!(err_run["code"], "paused");

    let run_retry = app
        .clone()
        .oneshot(authed_post(&format!("/api/runs/{run_id}/retry"), json!({})))
        .await
        .unwrap();
    assert_eq!(run_retry.status(), StatusCode::CONFLICT);

    let council_blocked = app
        .clone()
        .oneshot(authed_post(
            "/api/councils",
            json!({ "hub_session_id": "hub", "prompt": "blocked" }),
        ))
        .await
        .unwrap();
    assert_eq!(council_blocked.status(), StatusCode::CONFLICT);

    let gate_blocked = app
        .clone()
        .oneshot(authed_post(
            &format!("/api/pr-gates/{run_id}/run"),
            json!({ "repo_slug": "org/repo", "branch": "b" }),
        ))
        .await
        .unwrap();
    assert_eq!(gate_blocked.status(), StatusCode::CONFLICT);

    // 4. Resume clears pause
    let resumed = app
        .clone()
        .oneshot(authed_post("/api/resume", json!({})))
        .await
        .unwrap();
    assert_eq!(resumed.status(), StatusCode::OK);
    let resumed_body: Value =
        serde_json::from_slice(&resumed.into_body().collect().await.unwrap().to_bytes()).unwrap();
    assert_eq!(resumed_body["paused"], false);

    // 5. Pause with SCM client revokes auto_merge_armed PRs
    let scm_state = Arc::new(MockScmState::default());
    hub.set_scm(Arc::new(MockScmClient {
        state: scm_state.clone(),
        polled: None,
    }));
    let gate = hub
        .store
        .create_pr_gate(polyclave_store::NewPrGate {
            run_id,
            repo_slug: "org/repo".into(),
            branch: "feat/armed".into(),
            base_sha: "base".into(),
            head_sha: "head".into(),
            touched_paths: vec!["src/**".into()],
        })
        .unwrap();
    hub.store
        .record_pr_opened(&gate.id, 42, "https://github.com/org/repo/pull/42")
        .unwrap();
    hub.store
        .set_pr_gate_status(&gate.id, polyclave_proto::PrGateStatus::Approved)
        .unwrap();
    hub.store.arm_auto_merge(&gate.id, "head").unwrap().unwrap();

    let paused_with_scm = app
        .oneshot(authed_post(
            "/api/pause",
            json!({ "reason": "revoke check" }),
        ))
        .await
        .unwrap();
    assert_eq!(paused_with_scm.status(), StatusCode::OK);
    assert_eq!(
        scm_state.disabled_nodes.lock().unwrap().as_slice(),
        &["PR_node_42".to_owned()]
    );
    let after = hub.store.pr_gate(&gate.id).unwrap();
    assert!(!after.auto_merge_armed);
    assert_eq!(after.status, polyclave_proto::PrGateStatus::Paused);

    hub.close().await;
}

#[tokio::test]
async fn hub_tools_start_council_and_run_pr_gate_enforce_schema_pause_and_direction_invariant() {
    let (dir, hub, _rx) = test_hub().await;
    let tools = tool_list(true);
    assert!(tools.iter().any(|t| t["name"] == "start_council"));
    assert!(tools.iter().any(|t| t["name"] == "run_pr_gate"));
    let manager_tools = tool_list(false);
    assert!(!manager_tools.iter().any(|t| t["name"] == "start_council"));
    assert!(!manager_tools.iter().any(|t| t["name"] == "run_pr_gate"));

    let (run_id, _base, _head) = setup_git_run_fixture(dir.path(), &hub);
    hub.set_council_participants(vec![CouncilParticipant {
        vendor: Vendor::Codex,
        model: "gpt-5".into(),
        provider: Arc::new(MockCouncilProvider {
            summary: "Tool plan".into(),
            vendor_leak_check: "never".into(),
        }),
    }]);
    let scm_state = Arc::new(MockScmState::default());
    hub.set_scm(Arc::new(MockScmClient {
        state: scm_state,
        polled: None,
    }));
    hub.set_reviewer(Arc::new(MockReviewer));
    hub.set_pusher(Arc::new(MockPusher::new()));
    hub.set_default_reviewers(vec![ReviewerSpec {
        vendor: Vendor::Claude,
        model: "claude-sonnet-4-6".into(),
    }]);

    hub.store
        .hub_enqueue("hub", "req-tools", "run tools")
        .unwrap();
    let claim = hub
        .store
        .hub_claim("hub", "fixture-host", &hub.boot)
        .unwrap()
        .unwrap();
    hub.store.hub_authorize(claim.id, None).unwrap();
    let scope = ToolScope::new(claim.id, "hub", "hub", &claim.repo);

    // start_council via tool keeps direction awaiting_user and never auto-dispatches dev
    let council_val = hub
        .call_tool(
            &scope,
            "start_council",
            json!({
                "prompt": "Evaluate design",
                "level": "direction"
            }),
        )
        .await
        .unwrap();
    assert_eq!(council_val["status"], "awaiting_user");
    assert!(council_val["proposals"][0]["vendor"].is_null());
    assert_eq!(hub.runs.list().unwrap().len(), 1); // only the pre-seeded r-gate-1

    // run_pr_gate via tool completes gate
    let gate_val = hub
        .call_tool(
            &scope,
            "run_pr_gate",
            json!({
                "run_id": run_id,
                "repo_slug": "org/repo",
                "branch": "feat/tool",
                "pr_title": "Tool PR"
            }),
        )
        .await
        .unwrap();
    assert_eq!(gate_val["status"], "auto_merge_armed");
    assert_eq!(gate_val["auto_merge_armed"], true);

    // Global Pause blocks dispatch_task, start_council, and run_pr_gate tools
    hub.store.set_pause(true, Some("paused".into())).unwrap();
    assert!(matches!(
        hub.call_tool(
            &scope,
            "dispatch_task",
            json!({
                "ticket_id": "t-gate",
                "hub_session_id": "hub",
                "request_id": "paused-dispatch",
                "vendor": "codex",
                "repo_cwd": claim.repo,
                "base_branch": "main",
                "instructions": "must refuse"
            })
        )
        .await,
        Err(Error::Policy)
    ));
    assert!(matches!(
        hub.call_tool(&scope, "start_council", json!({ "prompt": "must refuse" }))
            .await,
        Err(Error::Policy)
    ));
    assert!(matches!(
        hub.call_tool(
            &scope,
            "run_pr_gate",
            json!({
                "run_id": run_id,
                "repo_slug": "org/repo",
                "branch": "feat/paused",
                "pr_title": "Must refuse"
            })
        )
        .await,
        Err(Error::Policy)
    ));

    hub.close().await;
}

#[tokio::test]
async fn scm_observer_starts_polls_and_stops_cleanly_on_hub_close() {
    let (dir, hub, _rx) = test_hub().await;
    let (run_id, _base, _head) = setup_git_run_fixture(dir.path(), &hub);
    let gate = hub
        .store
        .create_pr_gate(polyclave_store::NewPrGate {
            run_id,
            repo_slug: "org/repo".into(),
            branch: "feat/obs".into(),
            base_sha: "base".into(),
            head_sha: "sha_observed".into(),
            touched_paths: vec!["src/**".into()],
        })
        .unwrap();
    hub.store
        .record_pr_opened(&gate.id, 77, "https://github.com/org/repo/pull/77")
        .unwrap();

    let polled = Arc::new(Notify::new());
    let polled_wait = polled.notified();
    let scm = Arc::new(MockScmClient {
        state: Arc::new(MockScmState::default()),
        polled: Some(polled.clone()),
    });

    hub.start_scm_observer(scm, Duration::from_secs(60)).await;
    tokio::time::timeout(Duration::from_secs(2), polled_wait)
        .await
        .expect("SCM Observer should poll immediately on startup");

    // Must cancel and exit promptly without waiting for the 60s poll interval
    tokio::time::timeout(Duration::from_secs(2), hub.close())
        .await
        .expect("hub.close() should gracefully cancel SCM Observer without hanging");
    assert!(hub.lifecycle.observer.lock().await.is_none());
}

#[tokio::test]
async fn stage3_pairing_dual_identity_auth_revocation_mesh_and_private_notifications() {
    use axum::body::to_bytes;
    use axum::http::{header, Request, StatusCode};
    use tower::ServiceExt;

    let (_dir, hub, _rx) = test_hub().await;
    let store = hub.store.clone();
    let app = crate::routes::router(
        crate::routes::App {
            store: store.clone(),
            runs: hub.runs.clone(),
            runner: hub.runner.clone(),
            stream: hub.stream.clone(),
            hub: Some(hub.clone()),
        },
        crate::auth::LocalAuth::with_store("cap-token".into(), 4310, Some(store.clone())),
    );

    // 1. Create a 10-minute one-time pairing ticket with loopback capability token.
    let req = Request::builder()
        .method("POST")
        .uri("/api/pairing/tickets")
        .header(header::HOST, "127.0.0.1:4310")
        .header(header::AUTHORIZATION, "Bearer cap-token")
        .header(header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(
            json!({"client_kind":"ios","device_label":"Dust iPhone"}).to_string(),
        ))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&to_bytes(resp.into_body(), 1024 * 1024).await.unwrap()).unwrap();
    let secret = body["secret"].as_str().unwrap().to_owned();
    assert!(body["qr_uri"]
        .as_str()
        .unwrap()
        .starts_with("polyclave://pair?ticket="));

    // 2. Exchange the ticket for a device bearer token bound to a Tailscale identity.
    let ex_req = Request::builder()
        .method("POST")
        .uri("/api/pairing/exchange")
        .header(header::HOST, "polyclave.tailnet.ts.net")
        .header("Tailscale-User-Login", "dust@tailnet")
        .header(header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(
            json!({"secret": secret}).to_string(),
        ))
        .unwrap();
    let ex_resp = app.clone().oneshot(ex_req).await.unwrap();
    assert_eq!(ex_resp.status(), StatusCode::OK);
    let ex_body: Value =
        serde_json::from_slice(&to_bytes(ex_resp.into_body(), 1024 * 1024).await.unwrap()).unwrap();
    let device_id = ex_body["device"]["id"].as_str().unwrap().to_owned();
    let device_token = ex_body["token"].as_str().unwrap().to_owned();

    // 3. Dual-identity remote request succeeds with matching Tailscale-User-Login,
    //    and fails with mismatched identity.
    let ok_remote = Request::builder()
        .method("GET")
        .uri("/api/devices")
        .header(header::HOST, "polyclave.tailnet.ts.net")
        .header("Tailscale-User-Login", "dust@tailnet")
        .header(header::AUTHORIZATION, format!("Bearer {device_token}"))
        .body(axum::body::Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(ok_remote).await.unwrap().status(),
        StatusCode::OK
    );

    let bad_remote = Request::builder()
        .method("GET")
        .uri("/api/devices")
        .header(header::HOST, "polyclave.tailnet.ts.net")
        .header("Tailscale-User-Login", "attacker@tailnet")
        .header(header::AUTHORIZATION, format!("Bearer {device_token}"))
        .body(axum::body::Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(bad_remote).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );

    // 4. Revoke device and confirm immediate 401.
    let rev_req = Request::builder()
        .method("POST")
        .uri(format!("/api/devices/{device_id}/revoke"))
        .header(header::HOST, "127.0.0.1:4310")
        .header(header::AUTHORIZATION, "Bearer cap-token")
        .body(axum::body::Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(rev_req).await.unwrap().status(),
        StatusCode::OK
    );

    let post_rev = Request::builder()
        .method("GET")
        .uri("/api/devices")
        .header(header::HOST, "polyclave.tailnet.ts.net")
        .header("Tailscale-User-Login", "dust@tailnet")
        .header(header::AUTHORIZATION, format!("Bearer {device_token}"))
        .body(axum::body::Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(post_rev).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );

    // 5. Reject public notification endpoint; accept local mock ntfy + test delivery.
    let pub_ch = Request::builder()
        .method("POST")
        .uri("/api/notifications/channels")
        .header(header::HOST, "127.0.0.1:4310")
        .header(header::AUTHORIZATION, "Bearer cap-token")
        .header(header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(
            json!({
                "kind":"ntfy","label":"Public","endpoint":"https://ntfy.sh","topic":"poly"
            })
            .to_string(),
        ))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(pub_ch).await.unwrap().status(),
        StatusCode::CONFLICT
    );

    let ntfy_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let ntfy_addr = ntfy_listener.local_addr().unwrap();
    let ntfy_srv = tokio::spawn(async move {
        axum::serve(
            ntfy_listener,
            axum::Router::new().route("/poly", axum::routing::post(|| async { "ok" })),
        )
        .await
        .unwrap();
    });

    let priv_ch = Request::builder()
        .method("POST")
        .uri("/api/notifications/channels")
        .header(header::HOST, "127.0.0.1:4310")
        .header(header::AUTHORIZATION, "Bearer cap-token")
        .header(header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(
            json!({
                "kind":"ntfy","label":"Local ntfy","endpoint":format!("http://{ntfy_addr}"),"topic":"poly"
            })
            .to_string(),
        ))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(priv_ch).await.unwrap().status(),
        StatusCode::OK
    );

    let test_req = Request::builder()
        .method("POST")
        .uri("/api/notifications/test")
        .header(header::HOST, "127.0.0.1:4310")
        .header(header::AUTHORIZATION, "Bearer cap-token")
        .body(axum::body::Body::empty())
        .unwrap();
    let test_resp = app.clone().oneshot(test_req).await.unwrap();
    assert_eq!(test_resp.status(), StatusCode::OK);
    let test_body: Value =
        serde_json::from_slice(&to_bytes(test_resp.into_body(), 1024 * 1024).await.unwrap())
            .unwrap();
    assert_eq!(test_body["delivered"], 1);
    ntfy_srv.abort();
    hub.close().await;
}

#[tokio::test]
async fn stage4_cua_reclaim_evolution_skills_and_opt_in_community_routes() {
    use axum::body::to_bytes;
    use axum::http::{header, Request, StatusCode};
    use tower::ServiceExt;

    let (_dir, hub, _rx) = test_hub().await;
    let store = hub.store.clone();
    let app = crate::routes::router(
        crate::routes::App {
            store: store.clone(),
            runs: hub.runs.clone(),
            runner: hub.runner.clone(),
            stream: hub.stream.clone(),
            hub: Some(hub.clone()),
        },
        crate::auth::LocalAuth::with_store("cap-token".into(), 4310, Some(store)),
    );

    let post_json = |uri: &str, body: Value| {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header(header::HOST, "127.0.0.1:4310")
            .header(header::AUTHORIZATION, "Bearer cap-token")
            .header(header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(body.to_string()))
            .unwrap()
    };

    // CUA acquire + busy + 1-click reclaim.
    assert_eq!(
        app.clone()
            .oneshot(post_json(
                "/api/cua/acquire",
                json!({"holder_run_id":"r1","target_window":"Polyclave","observed_idle_secs":45})
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        app.clone()
            .oneshot(post_json(
                "/api/cua/acquire",
                json!({"holder_run_id":"r2","target_window":"Polyclave","observed_idle_secs":45})
            ))
            .await
            .unwrap()
            .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        app.clone()
            .oneshot(post_json("/api/cua/reclaim", json!({})))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    // Community draft refused when opt_in=false, succeeds when opt_in=true.
    let get_draft = Request::builder()
        .method("GET")
        .uri("/api/community/draft")
        .header(header::HOST, "127.0.0.1:4310")
        .header(header::AUTHORIZATION, "Bearer cap-token")
        .body(axum::body::Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(get_draft).await.unwrap().status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        app.clone()
            .oneshot(post_json("/api/community/opt-in", json!({"opt_in": true})))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let get_draft_ok = Request::builder()
        .method("GET")
        .uri("/api/community/draft")
        .header(header::HOST, "127.0.0.1:4310")
        .header(header::AUTHORIZATION, "Bearer cap-token")
        .body(axum::body::Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(get_draft_ok).await.unwrap().status(),
        StatusCode::OK
    );

    // Distill + Teach-Once skill sync.
    let dist_resp = app
        .clone()
        .oneshot(post_json("/api/evolution/distill", json!({})))
        .await
        .unwrap();
    assert_eq!(dist_resp.status(), StatusCode::OK);
    let skill_resp = app
        .clone()
        .oneshot(post_json(
            "/api/skills",
            json!({
                "name":"smoke-check",
                "description":"Run workspace check",
                "body_md":"# Smoke\ncargo test",
                "synced_vendors":["codex","dsh"]
            }),
        ))
        .await
        .unwrap();
    assert_eq!(skill_resp.status(), StatusCode::OK);
    let skill_val: Value =
        serde_json::from_slice(&to_bytes(skill_resp.into_body(), 1024 * 1024).await.unwrap())
            .unwrap();
    assert_eq!(skill_val["version"], 1);
    assert!(hub
        .state
        .join("isolated-homes/codex/skills/smoke-check/SKILL.md")
        .is_file());
    hub.close().await;
}

#[tokio::test]
async fn notification_dispatcher_delivers_metadata_only_for_attention_changes() {
    use axum::{routing::post, Json, Router};
    use polyclave_hub::notify::{
        dispatch_pending_notifications, HttpPrivateNotifier, NotificationMetadata,
    };
    use polyclave_proto::{ChangeTopic, NotificationChannelKind};
    use std::sync::{Arc, Mutex};

    let (_dir, hub, _rx) = test_hub().await;
    let store = hub.store.clone();
    let seen = Arc::new(Mutex::new(Vec::<NotificationMetadata>::new()));
    let seen_c = seen.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let srv = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new().route(
                "/poly",
                post(move |Json(m): Json<NotificationMetadata>| {
                    let s = seen_c.clone();
                    async move {
                        s.lock().unwrap().push(m);
                        "ok"
                    }
                }),
            ),
        )
        .await
        .unwrap();
    });

    store
        .upsert_notification_channel(polyclave_store::stage3::NewNotificationChannel {
            id: None,
            kind: NotificationChannelKind::Ntfy,
            label: "Local ntfy".into(),
            endpoint: format!("http://{addr}"),
            topic: "poly".into(),
            enabled: true,
        })
        .unwrap();

    store
        .write(|db| {
            Store::append_change(
                db,
                ChangeTopic::Council,
                "c123",
                &json!({"status": "awaiting_user", "prompt": "sensitive code"}),
            )
        })
        .unwrap();

    let notifier = HttpPrivateNotifier::new().unwrap();
    let (_cursor, delivered) = dispatch_pending_notifications(&store, &notifier, 0)
        .await
        .unwrap();
    assert_eq!(delivered, 1);
    {
        let captured = seen.lock().unwrap();
        assert_eq!(captured.len(), 1);
        assert_eq!(captured[0].topic, "council");
        assert_eq!(captured[0].entity_id, "c123");
        assert_eq!(captured[0].title, "Council awaiting decision");
    }

    srv.abort();
    hub.close().await;
}

#[tokio::test]
async fn mesh_heartbeat_probe_and_attenuated_remote_dispatch_work_end_to_end() {
    use polyclave_proto::MeshNodeStatus;
    use polyclave_store::stage3::NewMeshNode;

    let shared_key = b"0123456789abcdef0123456789abcdef".to_vec();
    let (_p_dir, primary, _rx1) = test_hub().await;
    primary.set_mesh_key(shared_key.clone());

    let (_r_dir, remote, _rx2) = test_hub().await;
    remote.set_mesh_key(shared_key);
    remote
        .store
        .upsert_mesh_node(NewMeshNode {
            id: "node-win".into(),
            name: "Win Box".into(),
            endpoint: "http://127.0.0.1:1".into(),
            os_family: "windows".into(),
            allowed_repos: vec!["repo".into()],
            allowed_actions: vec!["ping".into()],
            status: MeshNodeStatus::Online,
        })
        .unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let remote_app = axum::Router::new().nest("/mesh", crate::hub_api::mesh_router(remote.clone()));
    let srv = tokio::spawn(async move {
        axum::serve(listener, remote_app).await.unwrap();
    });

    // Register remote node as initially offline on primary, then probe to Online.
    primary
        .store
        .upsert_mesh_node(NewMeshNode {
            id: "node-win".into(),
            name: "Win Box".into(),
            endpoint: format!("http://{addr}"),
            os_family: "windows".into(),
            allowed_repos: vec!["repo".into()],
            allowed_actions: vec!["ping".into()],
            status: MeshNodeStatus::Offline,
        })
        .unwrap();
    assert_eq!(primary.probe_mesh_nodes_once().await, 1);
    assert_eq!(
        primary.store.get_mesh_node("node-win").unwrap().status,
        MeshNodeStatus::Online
    );

    // Hub dispatch_remote succeeds for allowlisted action and rejects unpermitted action.
    let scope = ToolScope::new(1, "s1", "hub", "repo");
    let res = primary
        .call_tool(
            &scope,
            "dispatch_remote",
            json!({"node_id": "node-win", "action": "ping", "instructions": ""}),
        )
        .await
        .unwrap();
    assert_eq!(res["ok"], true);
    assert_eq!(res["node_id"], "node-win");

    assert!(primary
        .call_tool(
            &scope,
            "dispatch_remote",
            json!({"node_id": "node-win", "action": "shell_exec", "instructions": "id"}),
        )
        .await
        .is_err());

    // Once remote peer stops, next heartbeat probe marks it Offline.
    srv.abort();
    let _ = primary.probe_mesh_nodes_once().await;
    assert_eq!(
        primary.store.get_mesh_node("node-win").unwrap().status,
        MeshNodeStatus::Offline
    );

    remote.close().await;
    primary.close().await;
}

#[tokio::test]
async fn community_submit_route_and_applied_evolution_system_prompt_work() {
    use axum::http::{header, Request, StatusCode};
    use polyclave_proto::EvolutionStatus;
    use tower::ServiceExt;

    let (_dir, hub, _rx) = test_hub().await;
    let store = hub.store.clone();
    let app = crate::routes::router(
        crate::routes::App {
            store: store.clone(),
            runs: hub.runs.clone(),
            runner: hub.runner.clone(),
            stream: hub.stream.clone(),
            hub: Some(hub.clone()),
        },
        crate::auth::LocalAuth::with_store("cap-token".into(), 4310, Some(store.clone())),
    );

    // Community submit rejects when opt_in=false or confirmed=false.
    let submit_req = Request::builder()
        .method("POST")
        .uri("/api/community/submit")
        .header(header::HOST, "127.0.0.1:4310")
        .header(header::AUTHORIZATION, "Bearer cap-token")
        .header(header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(
            json!({"title":"Learning","body":"Body","confirmed":true}).to_string(),
        ))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(submit_req).await.unwrap().status(),
        StatusCode::CONFLICT
    );

    // Applied evolution guidance is injected into Hub system prompt and removed on rollback.
    let created = polyclave_hub::evolution::distill_local_suggestions(&store).unwrap();
    assert!(!created.is_empty());
    let id = created[0].id.clone();
    store
        .set_evolution_status(&id, EvolutionStatus::Applied)
        .unwrap();
    let prompt_with_evo = crate::hub_api::build_system_prompt(&store);
    assert!(prompt_with_evo.contains("<applied_evolution_guidance>"));
    assert!(prompt_with_evo.contains(&created[0].title));

    store
        .set_evolution_status(&id, EvolutionStatus::RolledBack)
        .unwrap();
    let prompt_rolled = crate::hub_api::build_system_prompt(&store);
    assert!(!prompt_rolled.contains("<applied_evolution_guidance>"));

    hub.close().await;
}

/// Builds a Hub whose runner launches `codex` from a fake CLI fixture. The
/// supervisor stays offline, so a scheduled Worker ends without any real agent.
async fn mesh_worker_hub(dir: &FsPath) -> Hub {
    let fake_codex = dir.join(if cfg!(windows) {
        "fake-codex.cmd"
    } else {
        "fake-codex"
    });
    let script = if cfg!(windows) {
        "@echo off\r\nexit /b 0\r\n"
    } else {
        "#!/bin/sh\nexit 0\n"
    };
    std::fs::write(&fake_codex, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake_codex, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let store = Store::open(dir.join("hub.db")).unwrap();
    let runs = RunManager::new(store.clone(), Arc::new(MirrorManager::new(dir).unwrap()));
    let runner = TurnRunner::new(
        runs.clone(),
        dir,
        "mesh-remote-host".into(),
        RunnerConfig {
            supervisor: "offline".into(),
            mcp_url: "http://127.0.0.1:1/mcp".into(),
            codex: fake_codex,
            claude: "offline".into(),
            claude_isolation: None,
        },
        |_, _| {},
    )
    .unwrap();
    let (stream, _) = broadcast::channel(64);
    Hub::new(
        store,
        runs,
        runner,
        "mesh-remote-host".into(),
        dir.to_path_buf(),
        stream,
    )
    .await
    .unwrap()
}

/// A committed `main` branch the mirror can admit as a Run baseline.
fn mesh_git_repo(path: &FsPath) -> String {
    std::fs::create_dir_all(path).unwrap();
    let repo = polyclave_os::canonical_real_path(path).unwrap();
    git_cmd(&repo, &["init", "-b", "main", "--template="]);
    git_cmd(&repo, &["config", "user.name", "Fixture"]);
    git_cmd(&repo, &["config", "user.email", "fixture@example.invalid"]);
    std::fs::write(repo.join("README.md"), "mesh fixture\n").unwrap();
    git_cmd(&repo, &["add", "."]);
    git_cmd(&repo, &["commit", "-m", "initial"]);
    repo.to_str().unwrap().to_owned()
}

fn mesh_node(id: &str, endpoint: String, repos: &[&str]) -> polyclave_store::stage3::NewMeshNode {
    polyclave_store::stage3::NewMeshNode {
        id: id.into(),
        name: "Win Box".into(),
        endpoint,
        os_family: "windows".into(),
        allowed_repos: repos.iter().map(|r| (*r).to_owned()).collect(),
        allowed_actions: vec!["worker_run".into(), "run_status".into()],
        status: polyclave_proto::MeshNodeStatus::Online,
    }
}

async fn post_mesh_dispatch(hub: &Hub, body: Value) -> (StatusCode, Value) {
    let res = crate::hub_api::mesh_router(hub.clone())
        .oneshot(
            Request::post("/dispatch")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn mesh_worker_run_rejects_unauthorized_and_cross_repository_tokens() {
    let dir = tempfile::tempdir().unwrap();
    let remote = mesh_worker_hub(dir.path()).await;
    remote.set_mesh_key(b"0123456789abcdef0123456789abcdef".to_vec());
    let repo_a = mesh_git_repo(&dir.path().join("repo-a"));
    let repo_b = mesh_git_repo(&dir.path().join("repo-b"));
    remote
        .store
        .upsert_mesh_node(mesh_node(
            "node-win",
            "http://127.0.0.1:1".into(),
            &[&repo_a, &repo_b],
        ))
        .unwrap();
    let policy = remote.mesh_policy();
    let scope = |repos: Vec<String>, actions: &[&str]| polyclave_mesh::MeshScope {
        node_id: "node-win".into(),
        allowed_repos: repos,
        allowed_actions: actions.iter().map(|a| (*a).to_owned()).collect(),
        expires_at_epoch: polyclave_mesh::current_epoch_secs() + 120,
    };
    let request = |repo: &str, action: &str, token: &polyclave_mesh::AttenuatedToken| {
        let mut req =
            polyclave_mesh::RemoteDispatchRequest::new(repo, action, "do it", token.clone());
        req.request_id = Some("req-denied".into());
        json!(req)
    };

    // A token that only grants run_status cannot start a Worker.
    let status_only = policy
        .issue(scope(vec![repo_a.clone()], &["run_status"]))
        .unwrap();
    let (code, _) = post_mesh_dispatch(&remote, request(&repo_a, "worker_run", &status_only)).await;
    assert_eq!(code, StatusCode::FORBIDDEN);

    // A token attenuated down to repo-b cannot start a Worker in repo-a, even though
    // the node itself allows both repositories.
    let parent = policy
        .issue(scope(
            vec![repo_a.clone(), repo_b.clone()],
            &["worker_run", "run_status"],
        ))
        .unwrap();
    let only_b = policy
        .attenuate(
            &parent,
            scope(vec![repo_b.clone()], &["worker_run", "run_status"]),
        )
        .unwrap();
    let (code, _) = post_mesh_dispatch(&remote, request(&repo_a, "worker_run", &only_b)).await;
    assert_eq!(code, StatusCode::FORBIDDEN);

    // A token signed with another root key is rejected.
    let foreign = polyclave_mesh::MeshPolicy::new(b"ffffffffffffffffffffffffffffffff".to_vec())
        .unwrap()
        .issue(scope(vec![repo_a.clone()], &["worker_run"]))
        .unwrap();
    let (code, _) = post_mesh_dispatch(&remote, request(&repo_a, "worker_run", &foreign)).await;
    assert_eq!(code, StatusCode::FORBIDDEN);

    // Nothing was admitted by any rejected request.
    assert!(remote.runs.list().unwrap().is_empty());

    // Global Pause answers 409 before any admission.
    remote.store.set_pause(true, None).unwrap();
    let (code, _) = post_mesh_dispatch(&remote, request(&repo_a, "worker_run", &parent)).await;
    assert_eq!(code, StatusCode::CONFLICT);
    assert!(remote.runs.list().unwrap().is_empty());

    remote.close().await;
}

#[tokio::test]
async fn mesh_worker_run_and_run_status_work_end_to_end_through_dispatch_remote() {
    let shared_key = b"0123456789abcdef0123456789abcdef".to_vec();
    let (_p_dir, primary, _rx) = test_hub().await;
    primary.set_mesh_key(shared_key.clone());

    let r_dir = tempfile::tempdir().unwrap();
    let remote = mesh_worker_hub(r_dir.path()).await;
    remote.set_mesh_key(shared_key);
    let repo = mesh_git_repo(&r_dir.path().join("repo"));
    let other = mesh_git_repo(&r_dir.path().join("other"));
    remote
        .store
        .upsert_mesh_node(mesh_node(
            "node-win",
            "http://127.0.0.1:1".into(),
            &[&repo, &other],
        ))
        .unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let remote_app = axum::Router::new().nest("/mesh", crate::hub_api::mesh_router(remote.clone()));
    let srv = tokio::spawn(async move {
        axum::serve(listener, remote_app).await.unwrap();
    });
    primary
        .store
        .upsert_mesh_node(mesh_node("node-win", format!("http://{addr}"), &[&repo]))
        .unwrap();

    let scope = ToolScope::new(1, "hub", "hub", &repo);
    insert_ticket(&primary.store, "t-mesh", "hub", &repo, 1, "active");
    let worker_run = json!({
        "node_id": "node-win",
        "action": "worker_run",
        "instructions": "Add a CHANGELOG entry.",
        "vendor": "codex",
        "model": "gpt-5",
        "base_branch": "main",
        "request_id": "req-mesh-1",
        "ticket_id": "t-mesh"
    });
    let started: polyclave_mesh::RemoteWorkerResult = serde_json::from_value(
        primary
            .call_tool(&scope, "dispatch_remote", worker_run.clone())
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(started.node_id, "node-win");
    assert!(started.run_id.starts_with('r'), "{started:?}");

    // The Run landed in the remote ledger under a single-Run explicit ticket owned
    // by the mesh Hub session; the primary ledger admitted nothing.
    let run = remote.runs.get(&started.run_id).unwrap();
    assert_eq!(run.repo_cwd, repo);
    assert_eq!(run.vendor, Vendor::Codex);
    assert_eq!(run.model.as_deref(), Some("gpt-5"));
    assert_eq!(run.base_branch, "main");
    // The wire request_id carries the calling Hub session, so two sessions can
    // never collide on the same name at the receiver.
    assert_eq!(run.request_id, "hub/req-mesh-1");
    let (source, max_runs, session_name): (String, u32, String) = remote
        .store
        .read(|db| {
            Ok(db.query_row(
                "SELECT t.source_kind,t.max_runs,s.name FROM runs r
                 JOIN tickets t ON t.id=r.ticket_id JOIN sessions s ON s.id=r.hub_session_id
                 WHERE r.id=?1",
                [&started.run_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?)
        })
        .unwrap();
    assert_eq!(source, "explicit_user");
    assert_eq!(max_runs, 1);
    assert_eq!(session_name, "Mesh remote");
    assert!(primary.runs.list().unwrap().is_empty());

    // Repeating the request_id is idempotent: same Run, nothing new admitted.
    let repeated: polyclave_mesh::RemoteWorkerResult = serde_json::from_value(
        primary
            .call_tool(&scope, "dispatch_remote", worker_run)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(repeated.run_id, started.run_id);
    assert_eq!(remote.runs.list().unwrap().len(), 1);

    // run_status reports what the remote ledger holds for that Run.
    let status: polyclave_mesh::RemoteWorkerResult = serde_json::from_value(
        primary
            .call_tool(
                &scope,
                "dispatch_remote",
                json!({"node_id": "node-win", "action": "run_status", "run_id": started.run_id}),
            )
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(status.node_id, "node-win");
    assert_eq!(status.run_id, started.run_id);
    let current = remote.runs.get(&started.run_id).unwrap();
    assert_eq!(
        json!(status.status),
        serde_json::to_value(current.status).unwrap()
    );
    assert_eq!(status.result_sha, current.result_sha);

    // Unknown Runs and missing run_id are refused.
    assert!(primary
        .call_tool(
            &scope,
            "dispatch_remote",
            json!({"node_id": "node-win", "action": "run_status", "run_id": "r-missing"}),
        )
        .await
        .is_err());
    assert!(primary
        .call_tool(
            &scope,
            "dispatch_remote",
            json!({"node_id": "node-win", "action": "run_status"}),
        )
        .await
        .is_err());

    // A run_status token for another repository cannot read this Run.
    let other_token = remote
        .mesh_policy()
        .issue(polyclave_mesh::MeshScope {
            node_id: "node-win".into(),
            allowed_repos: vec![other.clone()],
            allowed_actions: vec!["run_status".into()],
            expires_at_epoch: polyclave_mesh::current_epoch_secs() + 120,
        })
        .unwrap();
    let mut probe =
        polyclave_mesh::RemoteDispatchRequest::new(&other, "run_status", "", other_token);
    probe.run_id = Some(started.run_id.clone());
    let (code, _) = post_mesh_dispatch(&remote, json!(probe)).await;
    assert_eq!(code, StatusCode::FORBIDDEN);

    srv.abort();
    remote.close().await;
    primary.close().await;
}

#[tokio::test]
async fn mesh_worker_run_requires_request_id_and_admits_one_run_per_id() {
    let dir = tempfile::tempdir().unwrap();
    let remote = mesh_worker_hub(dir.path()).await;
    remote.set_mesh_key(b"0123456789abcdef0123456789abcdef".to_vec());
    let repo = mesh_git_repo(&dir.path().join("repo"));
    remote
        .store
        .upsert_mesh_node(mesh_node("node-win", "http://127.0.0.1:1".into(), &[&repo]))
        .unwrap();
    let token = remote
        .mesh_policy()
        .issue(polyclave_mesh::MeshScope {
            node_id: "node-win".into(),
            allowed_repos: vec![repo.clone()],
            allowed_actions: vec!["worker_run".into()],
            expires_at_epoch: polyclave_mesh::current_epoch_secs() + 120,
        })
        .unwrap();
    let request = |request_id: Option<&str>| {
        let mut req = polyclave_mesh::RemoteDispatchRequest::new(
            &repo,
            "worker_run",
            "Add a CHANGELOG entry.",
            token.clone(),
        );
        req.request_id = request_id.map(str::to_owned);
        json!(req)
    };

    // A missing or blank request_id is refused before anything is admitted.
    let (code, _) = post_mesh_dispatch(&remote, request(None)).await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    let (code, _) = post_mesh_dispatch(&remote, request(Some("   "))).await;
    assert_eq!(code, StatusCode::BAD_REQUEST);
    assert!(remote.runs.list().unwrap().is_empty());

    // The same request_id sent twice, as a retry after a lost reply would be,
    // yields one Run.
    let (code, first) = post_mesh_dispatch(&remote, request(Some("req-retry-1"))).await;
    assert_eq!(code, StatusCode::OK, "{first}");
    let (code, second) = post_mesh_dispatch(&remote, request(Some("req-retry-1"))).await;
    assert_eq!(code, StatusCode::OK, "{second}");
    assert_eq!(first["run_id"], second["run_id"]);
    let runs = remote.runs.list().unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].request_id, "req-retry-1");

    remote.close().await;
}

#[tokio::test]
async fn dispatch_remote_worker_run_without_request_id_is_refused_before_sending() {
    let (_dir, primary, _rx) = test_hub().await;
    primary.set_mesh_key(b"0123456789abcdef0123456789abcdef".to_vec());
    // Accept connections but record whether anything was sent.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = hits.clone();
    let app = axum::Router::new().fallback(move || {
        let seen = seen.clone();
        async move {
            seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            StatusCode::OK
        }
    });
    let srv = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let repo = "/repo/mesh-primary";
    primary
        .store
        .upsert_mesh_node(mesh_node("node-win", format!("http://{addr}"), &[repo]))
        .unwrap();
    let scope = ToolScope::new(1, "hub", "hub", repo);
    for request_id in [None, Some(""), Some("  ")] {
        let mut args = json!({
            "node_id": "node-win",
            "action": "worker_run",
            "instructions": "Add a CHANGELOG entry."
        });
        if let Some(id) = request_id {
            args["request_id"] = json!(id);
        }
        assert!(primary
            .call_tool(&scope, "dispatch_remote", args)
            .await
            .is_err());
    }
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 0);

    srv.abort();
    primary.close().await;
}

/// Waits until the Worker of `run_id` has finished on its own: no turn is still
/// running and the Run left `pending`/`running`. Bounded by a timeout.
async fn mesh_run_settled(hub: &Hub, run_id: &str) {
    let wait = async {
        loop {
            let id = run_id.to_owned();
            let busy: bool = hub
                .store
                .read(move |db| {
                    Ok(db.query_row(
                        "SELECT EXISTS(SELECT 1 FROM turns WHERE run_id=?1 AND status='running')
                            OR EXISTS(SELECT 1 FROM runs WHERE id=?1 AND status IN ('pending','running'))",
                        [&id],
                        |r| r.get(0),
                    )?)
                })
                .unwrap();
            if !busy {
                return;
            }
            tokio::task::yield_now().await;
        }
    };
    tokio::time::timeout(Duration::from_secs(30), wait)
        .await
        .expect("mesh Worker did not finish");
}

/// Sets a Run's status directly, after its Worker finished, to model a Run that
/// still holds (`running`) or has released (`done`) its concurrency slot.
fn force_run_status(hub: &Hub, run_id: &str, status: &'static str) {
    let id = run_id.to_owned();
    hub.store
        .write(move |db| {
            db.execute("UPDATE runs SET status=?2 WHERE id=?1", params![id, status])?;
            Ok(())
        })
        .unwrap();
}

struct MeshCapFixture {
    _dir: tempfile::TempDir,
    remote: Hub,
    repo: String,
    request: Box<dyn Fn(&str) -> Value + Send + Sync>,
}

async fn mesh_cap_fixture() -> MeshCapFixture {
    let dir = tempfile::tempdir().unwrap();
    let remote = mesh_worker_hub(dir.path()).await;
    remote.set_mesh_key(b"0123456789abcdef0123456789abcdef".to_vec());
    let repo = mesh_git_repo(&dir.path().join("repo"));
    remote
        .store
        .upsert_mesh_node(mesh_node("node-win", "http://127.0.0.1:1".into(), &[&repo]))
        .unwrap();
    let token = remote
        .mesh_policy()
        .issue(polyclave_mesh::MeshScope {
            node_id: "node-win".into(),
            allowed_repos: vec![repo.clone()],
            allowed_actions: vec!["worker_run".into()],
            expires_at_epoch: polyclave_mesh::current_epoch_secs() + 600,
        })
        .unwrap();
    let repo_for_request = repo.clone();
    let request = Box::new(move |request_id: &str| {
        let mut req = polyclave_mesh::RemoteDispatchRequest::new(
            &repo_for_request,
            "worker_run",
            "Add a CHANGELOG entry.",
            token.clone(),
        );
        req.request_id = Some(request_id.to_owned());
        json!(req)
    });
    MeshCapFixture {
        _dir: dir,
        remote,
        repo,
        request,
    }
}

/// Admits `request_id` and leaves its Run holding a slot (`running`).
async fn admit_running(f: &MeshCapFixture, request_id: &str) -> String {
    let (code, body) = post_mesh_dispatch(&f.remote, (f.request)(request_id)).await;
    assert_eq!(code, StatusCode::OK, "{body}");
    let run_id = body["run_id"].as_str().unwrap().to_owned();
    mesh_run_settled(&f.remote, &run_id).await;
    force_run_status(&f.remote, &run_id, "running");
    run_id
}

#[tokio::test]
async fn mesh_worker_run_refuses_a_fourth_concurrent_run_and_admits_after_a_slot_frees() {
    let f = mesh_cap_fixture().await;
    let limit = f.remote.store.mesh_node_limit("node-win").unwrap();
    assert_eq!((limit.max_concurrent_runs, limit.active_runs), (3, 0));

    let first = admit_running(&f, "req-cap-1").await;
    admit_running(&f, "req-cap-2").await;
    admit_running(&f, "req-cap-3").await;
    assert_eq!(
        f.remote
            .store
            .mesh_node_limit("node-win")
            .unwrap()
            .active_runs,
        3
    );

    // A 4th concurrent Run is refused with a retryable 429 and nothing is admitted.
    let (code, body) = post_mesh_dispatch(&f.remote, (f.request)("req-cap-4")).await;
    assert_eq!(code, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(body["code"], "mesh_concurrency_limit");
    assert_eq!(body["retryable"], true);
    assert_eq!(body["limit"], 3);
    assert_eq!(f.remote.runs.list().unwrap().len(), 3);

    // A resend of an admitted request_id still returns its Run at the limit.
    let (code, body) = post_mesh_dispatch(&f.remote, (f.request)("req-cap-1")).await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(body["run_id"], first);
    assert_eq!(f.remote.runs.list().unwrap().len(), 3);

    // Runs that need review or were interrupted hold no Worker and no slot.
    force_run_status(&f.remote, &first, "needs_review");
    assert_eq!(
        f.remote
            .store
            .mesh_node_limit("node-win")
            .unwrap()
            .active_runs,
        2
    );
    force_run_status(&f.remote, &first, "running");

    // When one finishes, the slot frees and the same request_id is admitted.
    force_run_status(&f.remote, &first, "done");
    let (code, body) = post_mesh_dispatch(&f.remote, (f.request)("req-cap-4")).await;
    assert_eq!(code, StatusCode::OK, "{body}");
    let fourth = body["run_id"].as_str().unwrap().to_owned();
    assert_eq!(f.remote.runs.list().unwrap().len(), 4);
    mesh_run_settled(&f.remote, &fourth).await;

    // It is a concurrency cap, not a total: more Runs follow as slots free.
    force_run_status(&f.remote, &fourth, "failed");
    admit_running(&f, "req-cap-5").await;
    assert_eq!(f.remote.runs.list().unwrap().len(), 5);

    f.remote.close().await;
}

#[tokio::test]
async fn mesh_worker_run_concurrent_admissions_cannot_both_take_the_last_slot() {
    let f = mesh_cap_fixture().await;
    admit_running(&f, "req-race-1").await;
    admit_running(&f, "req-race-2").await;

    let (a, b) = tokio::join!(
        post_mesh_dispatch(&f.remote, (f.request)("req-race-a")),
        post_mesh_dispatch(&f.remote, (f.request)("req-race-b")),
    );
    let mut codes = [a.0, b.0];
    codes.sort();
    assert_eq!(
        codes,
        [StatusCode::OK, StatusCode::TOO_MANY_REQUESTS],
        "{} / {}",
        a.1,
        b.1
    );
    assert_eq!(f.remote.runs.list().unwrap().len(), 3);
    let winner = if a.0 == StatusCode::OK { &a.1 } else { &b.1 };
    mesh_run_settled(&f.remote, winner["run_id"].as_str().unwrap()).await;

    f.remote.close().await;
}

#[tokio::test]
async fn mesh_concurrency_setting_persists_through_the_local_api_and_is_enforced() {
    let f = mesh_cap_fixture().await;
    let store = f.remote.store.clone();
    let app = crate::routes::router(
        crate::routes::App {
            store: store.clone(),
            runs: f.remote.runs.clone(),
            runner: f.remote.runner.clone(),
            stream: f.remote.stream.clone(),
            hub: Some(f.remote.clone()),
        },
        crate::auth::LocalAuth::with_store("cap-token".into(), 4310, Some(store.clone())),
    );
    let call = |method: &str, uri: &str, body: Option<Value>, auth: bool| {
        let mut req = Request::builder()
            .method(method)
            .uri(uri)
            .header(axum::http::header::HOST, "127.0.0.1:4310")
            .header(axum::http::header::CONTENT_TYPE, "application/json");
        if auth {
            req = req.header(axum::http::header::AUTHORIZATION, "Bearer cap-token");
        }
        req.body(Body::from(body.map(|b| b.to_string()).unwrap_or_default()))
            .unwrap()
    };
    let send = |req: Request<Body>| {
        let app = app.clone();
        async move {
            let res = app.oneshot(req).await.unwrap();
            let status = res.status();
            let bytes = res.into_body().collect().await.unwrap().to_bytes();
            (
                status,
                serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null),
            )
        }
    };

    // The route is authenticated.
    let (code, _) = send(call(
        "POST",
        "/api/mesh/nodes/node-win/concurrency",
        Some(json!({"max_concurrent_runs": 1})),
        false,
    ))
    .await;
    assert_eq!(code, StatusCode::UNAUTHORIZED);
    assert_eq!(
        store
            .mesh_node_limit("node-win")
            .unwrap()
            .max_concurrent_runs,
        3
    );

    // The default is 3; out-of-range values and unknown nodes are refused.
    let (code, list) = send(call("GET", "/api/mesh/limits", None, true)).await;
    assert_eq!(code, StatusCode::OK);
    assert_eq!(
        list,
        json!([{"node_id": "node-win", "max_concurrent_runs": 3, "active_runs": 0}])
    );
    for bad in [0, 17] {
        let (code, _) = send(call(
            "POST",
            "/api/mesh/nodes/node-win/concurrency",
            Some(json!({"max_concurrent_runs": bad})),
            true,
        ))
        .await;
        assert_eq!(code, StatusCode::BAD_REQUEST, "{bad}");
    }
    let (code, _) = send(call(
        "POST",
        "/api/mesh/nodes/node-missing/concurrency",
        Some(json!({"max_concurrent_runs": 2})),
        true,
    ))
    .await;
    assert_eq!(code, StatusCode::NOT_FOUND);

    let (code, body) = send(call(
        "POST",
        "/api/mesh/nodes/node-win/concurrency",
        Some(json!({"max_concurrent_runs": 1})),
        true,
    ))
    .await;
    assert_eq!(code, StatusCode::OK, "{body}");
    assert_eq!(body["max_concurrent_runs"], 1);

    // Re-registering the node keeps the owner's setting.
    store
        .upsert_mesh_node(mesh_node(
            "node-win",
            "http://127.0.0.1:1".into(),
            &[&f.repo],
        ))
        .unwrap();
    assert_eq!(
        store
            .mesh_node_limit("node-win")
            .unwrap()
            .max_concurrent_runs,
        1
    );

    // It is enforced: with 1 Run active, the next one is refused.
    admit_running(&f, "req-set-1").await;
    let (code, body) = post_mesh_dispatch(&f.remote, (f.request)("req-set-2")).await;
    assert_eq!(code, StatusCode::TOO_MANY_REQUESTS, "{body}");
    assert_eq!(body["limit"], 1);
    assert_eq!(f.remote.runs.list().unwrap().len(), 1);

    f.remote.close().await;
}

/// Inserts a Ticket for `repo` owned by `hub` with a valid mandate.
fn insert_ticket(store: &Store, id: &str, hub: &str, repo: &str, max_runs: u32, status: &str) {
    let (id, hub, repo, status) = (
        id.to_owned(),
        hub.to_owned(),
        repo.to_owned(),
        status.to_owned(),
    );
    store
        .write(move |db| {
            db.execute(
                "INSERT INTO tickets(id,hub_session_id,repo_cwd,title,mandate_json,source_kind,max_runs,
                    status,created_at,updated_at)
                 VALUES(?1,?2,?3,'Remote work',?4,'explicit_user',?5,?6,'now','now')",
                params![
                    id,
                    hub,
                    repo,
                    json!({"goal":"Add a CHANGELOG entry","allowed_paths":["CHANGELOG.md"],
                        "forbidden_actions":[],"acceptance_criteria":["entry exists"]})
                    .to_string(),
                    max_runs,
                    status
                ],
            )?;
            Ok(())
        })
        .unwrap();
}

fn ticket_use(store: &Store, id: &str) -> (u32, String) {
    let id = id.to_owned();
    store
        .read(move |db| {
            Ok(db.query_row(
                "SELECT runs_used,status FROM tickets WHERE id=?1",
                [&id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?)
        })
        .unwrap()
}

struct MeshPair {
    _p_dir: tempfile::TempDir,
    _r_dir: tempfile::TempDir,
    primary: Hub,
    remote: Hub,
    repo: String,
    srv: tokio::task::JoinHandle<()>,
}

async fn mesh_pair() -> MeshPair {
    let shared_key = b"0123456789abcdef0123456789abcdef".to_vec();
    let (p_dir, primary, _rx) = test_hub().await;
    primary.set_mesh_key(shared_key.clone());
    let r_dir = tempfile::tempdir().unwrap();
    let remote = mesh_worker_hub(r_dir.path()).await;
    remote.set_mesh_key(shared_key);
    let repo = mesh_git_repo(&r_dir.path().join("repo"));
    remote
        .store
        .upsert_mesh_node(mesh_node("node-win", "http://127.0.0.1:1".into(), &[&repo]))
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = axum::Router::new().nest("/mesh", crate::hub_api::mesh_router(remote.clone()));
    let srv = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    primary
        .store
        .upsert_mesh_node(mesh_node("node-win", format!("http://{addr}"), &[&repo]))
        .unwrap();
    MeshPair {
        _p_dir: p_dir,
        _r_dir: r_dir,
        primary,
        remote,
        repo,
        srv,
    }
}

fn remote_worker_run(ticket: &str, request_id: &str) -> Value {
    json!({
        "node_id": "node-win",
        "action": "worker_run",
        "instructions": "Add a CHANGELOG entry.",
        "base_branch": "main",
        "ticket_id": ticket,
        "request_id": request_id
    })
}

#[tokio::test]
async fn dispatch_remote_charges_the_primary_ticket_once_per_request_id() {
    let p = mesh_pair().await;
    insert_ticket(&p.primary.store, "t-remote", "hub", &p.repo, 2, "active");
    let scope = ToolScope::new(1, "hub", "hub", &p.repo);

    // The first dispatch reserves one run and records the key.
    let first = p
        .primary
        .call_tool(
            &scope,
            "dispatch_remote",
            remote_worker_run("t-remote", "req-a"),
        )
        .await
        .unwrap();
    let run_a = first["run_id"].as_str().unwrap().to_owned();
    assert_eq!(
        ticket_use(&p.primary.store, "t-remote"),
        (1, "active".into())
    );
    let row = p
        .primary
        .store
        .remote_dispatch("hub", "req-a")
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            row.node_id.as_str(),
            row.ticket_id.as_str(),
            row.outcome.as_str()
        ),
        ("node-win", "t-remote", "admitted")
    );
    assert_eq!(row.remote_run_id.as_deref(), Some(run_a.as_str()));
    // The remote Worker got the Ticket mandate with the task, as dispatch_task does.
    let remote_run = p.remote.runs.get(&run_a).unwrap();
    assert!(remote_run
        .instructions
        .starts_with("Owner Ticket mandate (host data):"));
    assert!(remote_run.instructions.ends_with("Add a CHANGELOG entry."));
    // No Run is admitted on the primary.
    assert!(p.primary.runs.list().unwrap().is_empty());

    // A resend of the same key is not charged and reaches the same Run.
    let again = p
        .primary
        .call_tool(
            &scope,
            "dispatch_remote",
            remote_worker_run("t-remote", "req-a"),
        )
        .await
        .unwrap();
    assert_eq!(again["run_id"], first["run_id"]);
    assert_eq!(
        ticket_use(&p.primary.store, "t-remote"),
        (1, "active".into())
    );
    assert_eq!(p.remote.runs.list().unwrap().len(), 1);

    // The same key cannot be moved to another Ticket.
    insert_ticket(&p.primary.store, "t-other", "hub", &p.repo, 2, "active");
    assert!(p
        .primary
        .call_tool(
            &scope,
            "dispatch_remote",
            remote_worker_run("t-other", "req-a")
        )
        .await
        .is_err());
    assert_eq!(
        ticket_use(&p.primary.store, "t-other"),
        (0, "active".into())
    );

    p.srv.abort();
    p.remote.close().await;
    p.primary.close().await;
}

#[tokio::test]
async fn dispatch_remote_resend_after_a_crash_is_not_charged_twice_and_exhaustion_blocks() {
    let p = mesh_pair().await;
    insert_ticket(&p.primary.store, "t-crash", "hub", &p.repo, 2, "active");
    let scope = ToolScope::new(1, "hub", "hub", &p.repo);

    // Crash after the reservation, before anything was sent: the row exists,
    // is still `reserved`, and the receiver has no Run.
    let charged = p
        .primary
        .store
        .charge_remote_dispatch("hub", "req-crash", "node-win", "t-crash", &p.repo)
        .unwrap();
    assert!(matches!(
        charged,
        polyclave_store::remote_dispatch::RemoteCharge::Charged(_)
    ));
    assert_eq!(
        ticket_use(&p.primary.store, "t-crash"),
        (1, "active".into())
    );
    assert!(p.remote.runs.list().unwrap().is_empty());

    // On resume the Hub finds its earlier dispatch by key and resends it:
    // the Run is admitted once and the Ticket is not charged again.
    let resumed = p
        .primary
        .call_tool(
            &scope,
            "dispatch_remote",
            remote_worker_run("t-crash", "req-crash"),
        )
        .await
        .unwrap();
    assert_eq!(
        ticket_use(&p.primary.store, "t-crash"),
        (1, "active".into())
    );
    assert_eq!(
        p.primary
            .store
            .remote_dispatch("hub", "req-crash")
            .unwrap()
            .unwrap()
            .remote_run_id
            .as_deref(),
        resumed["run_id"].as_str()
    );
    assert_eq!(p.remote.runs.list().unwrap().len(), 1);

    // Crash after the receiver admitted the Run but before the answer was
    // recorded: the row stays `reserved`; the resend returns the same Run.
    p.primary
        .store
        .charge_remote_dispatch("hub", "req-lost", "node-win", "t-crash", &p.repo)
        .unwrap();
    assert_eq!(
        ticket_use(&p.primary.store, "t-crash"),
        (2, "active".into())
    );
    let token = p
        .remote
        .mesh_policy()
        .issue(polyclave_mesh::MeshScope {
            node_id: "node-win".into(),
            allowed_repos: vec![p.repo.clone()],
            allowed_actions: vec!["worker_run".into()],
            expires_at_epoch: polyclave_mesh::current_epoch_secs() + 120,
        })
        .unwrap();
    let (_, mandate_text) = p
        .primary
        .triage
        .remote_ticket("hub", "t-crash", &p.repo, "Add a CHANGELOG entry.", false)
        .unwrap();
    let mut direct =
        polyclave_mesh::RemoteDispatchRequest::new(&p.repo, "worker_run", mandate_text, token);
    direct.base_branch = Some("main".into());
    // Exactly what `dispatch_remote` puts on the wire for `req-lost`: the Hub
    // session namespaces the request_id so two sessions cannot collide.
    direct.request_id = Some("hub/req-lost".into());
    let (code, delivered) = post_mesh_dispatch(&p.remote, json!(direct)).await;
    assert_eq!(code, StatusCode::OK, "{delivered}");
    // The last slot was used, so the Ticket is exhausted yet still resendable.
    let resent = p
        .primary
        .call_tool(
            &scope,
            "dispatch_remote",
            remote_worker_run("t-crash", "req-lost"),
        )
        .await
        .unwrap();
    assert_eq!(resent["run_id"], delivered["run_id"]);
    assert_eq!(p.remote.runs.list().unwrap().len(), 2);
    assert_eq!(ticket_use(&p.primary.store, "t-crash").0, 2);

    // A new key on the exhausted Ticket is refused before any network call and
    // the Ticket waits on the user, exactly as dispatch_task does.
    assert!(p
        .primary
        .call_tool(
            &scope,
            "dispatch_remote",
            remote_worker_run("t-crash", "req-new")
        )
        .await
        .is_err());
    assert_eq!(
        ticket_use(&p.primary.store, "t-crash"),
        (2, "blocked_on_user".into())
    );
    assert!(p
        .primary
        .store
        .remote_dispatch("hub", "req-new")
        .unwrap()
        .is_none());
    assert_eq!(p.remote.runs.list().unwrap().len(), 2);
    // The atomic charge refuses on its own too, with BudgetExceededError.
    insert_ticket(&p.primary.store, "t-one", "hub", &p.repo, 1, "active");
    p.primary
        .store
        .charge_remote_dispatch("hub", "req-one-a", "node-win", "t-one", &p.repo)
        .unwrap();
    assert!(matches!(
        p.primary
            .store
            .charge_remote_dispatch("hub", "req-one-b", "node-win", "t-one", &p.repo),
        Err(polyclave_store::Error::BudgetExceededError)
    ));
    assert_eq!(
        ticket_use(&p.primary.store, "t-one"),
        (1, "blocked_on_user".into())
    );

    p.srv.abort();
    p.remote.close().await;
    p.primary.close().await;
}

#[tokio::test]
async fn dispatch_remote_refuses_a_foreign_wrong_repo_or_closed_ticket_before_sending() {
    let (_dir, primary, _rx) = test_hub().await;
    primary.set_mesh_key(b"0123456789abcdef0123456789abcdef".to_vec());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = hits.clone();
    let app = axum::Router::new().fallback(move || {
        let seen = seen.clone();
        async move {
            seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            StatusCode::OK
        }
    });
    let srv = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let repo = "/repo/mesh-primary";
    primary
        .store
        .upsert_mesh_node(mesh_node("node-win", format!("http://{addr}"), &[repo]))
        .unwrap();
    primary
        .store
        .write(|db| {
            db.execute(
                "INSERT INTO sessions(id,name,role,repo_cwd,profile_id,provider_model,created_at)
                 VALUES('hub2','Other Hub','hub','/repo/mesh-primary','api','m1','now')",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    insert_ticket(&primary.store, "t-foreign", "hub2", repo, 4, "active");
    insert_ticket(
        &primary.store,
        "t-elsewhere",
        "hub",
        "/repo/other",
        4,
        "active",
    );
    insert_ticket(&primary.store, "t-done", "hub", repo, 4, "done");
    insert_ticket(&primary.store, "t-cancelled", "hub", repo, 4, "cancelled");
    insert_ticket(&primary.store, "t-proposed", "hub", repo, 4, "proposed");
    insert_ticket(
        &primary.store,
        "t-blocked",
        "hub",
        repo,
        4,
        "blocked_on_user",
    );
    let scope = ToolScope::new(1, "hub", "hub", repo);

    let mut cases: Vec<Value> = [
        "t-foreign",
        "t-elsewhere",
        "t-done",
        "t-cancelled",
        "t-proposed",
        "t-blocked",
        "t-missing",
    ]
    .iter()
    .enumerate()
    .map(|(i, t)| remote_worker_run(t, &format!("req-bad-{i}")))
    .collect();
    // A missing or blank ticket_id is refused too.
    let mut no_ticket = remote_worker_run("", "req-bad-none");
    no_ticket.as_object_mut().unwrap().remove("ticket_id");
    cases.push(no_ticket);
    cases.push(remote_worker_run("  ", "req-bad-blank"));
    for args in cases {
        assert!(
            primary
                .call_tool(&scope, "dispatch_remote", args.clone())
                .await
                .is_err(),
            "{args}"
        );
    }
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 0);
    for t in [
        "t-foreign",
        "t-elsewhere",
        "t-done",
        "t-cancelled",
        "t-proposed",
        "t-blocked",
    ] {
        assert_eq!(ticket_use(&primary.store, t).0, 0, "{t}");
    }
    let rows: i64 = primary
        .store
        .read(|db| Ok(db.query_row("SELECT COUNT(*) FROM remote_dispatches", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(rows, 0);

    srv.abort();
    primary.close().await;
}

#[tokio::test]
async fn dispatch_remote_at_the_receiver_limit_is_retryable_and_charged_once() {
    let p = mesh_pair().await;
    insert_ticket(&p.primary.store, "t-limit", "hub", &p.repo, 3, "active");
    let scope = ToolScope::new(1, "hub", "hub", &p.repo);
    p.remote
        .store
        .set_mesh_node_concurrency("node-win", 1)
        .unwrap();

    let first = p
        .primary
        .call_tool(
            &scope,
            "dispatch_remote",
            remote_worker_run("t-limit", "req-l1"),
        )
        .await
        .unwrap();
    let first_run = first["run_id"].as_str().unwrap().to_owned();
    mesh_run_settled(&p.remote, &first_run).await;
    force_run_status(&p.remote, &first_run, "running");

    // The receiver is full: a clear 429 comes back, nothing is admitted, and the
    // row records it. The reservation is kept.
    let refused = p
        .primary
        .call_tool(
            &scope,
            "dispatch_remote",
            remote_worker_run("t-limit", "req-l2"),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(refused, Error::Http { status: 429, .. }),
        "{refused:?}"
    );
    assert_eq!(p.remote.runs.list().unwrap().len(), 1);
    assert_eq!(
        ticket_use(&p.primary.store, "t-limit"),
        (2, "active".into())
    );
    assert_eq!(
        p.primary
            .store
            .remote_dispatch("hub", "req-l2")
            .unwrap()
            .unwrap()
            .outcome,
        "at_limit"
    );

    // Once the slot frees, the same request_id goes through without a new charge.
    force_run_status(&p.remote, &first_run, "done");
    let second = p
        .primary
        .call_tool(
            &scope,
            "dispatch_remote",
            remote_worker_run("t-limit", "req-l2"),
        )
        .await
        .unwrap();
    assert_ne!(second["run_id"], first["run_id"]);
    assert_eq!(
        ticket_use(&p.primary.store, "t-limit"),
        (2, "active".into())
    );
    let row = p
        .primary
        .store
        .remote_dispatch("hub", "req-l2")
        .unwrap()
        .unwrap();
    assert_eq!(row.outcome, "admitted");
    assert_eq!(row.remote_run_id.as_deref(), second["run_id"].as_str());
    mesh_run_settled(&p.remote, second["run_id"].as_str().unwrap()).await;

    p.srv.abort();
    p.remote.close().await;
    p.primary.close().await;
}

/// Pairs over the tailnet host and keeps both pieces of evidence in one place.
async fn paired_over_tailnet(app: &axum::Router, store: &Store) -> (String, String) {
    use axum::body::to_bytes;
    use axum::http::header;

    let (_, secret) = store
        .create_pairing_ticket(
            polyclave_proto::ClientKind::Ios,
            "Dust iPhone".to_owned(),
            600,
        )
        .unwrap();
    let req = Request::builder()
        .method("POST")
        .uri("/api/pairing/exchange")
        .header(header::HOST, "polyclave.tailnet.ts.net")
        .header("Tailscale-User-Login", "dust@tailnet")
        .header(header::ORIGIN, "https://polyclave.tailnet.ts.net")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "secret": secret }).to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&to_bytes(resp.into_body(), 1024 * 1024).await.unwrap()).unwrap();
    let device = body["device"]["id"].as_str().unwrap().to_owned();
    let token = body["token"].as_str().unwrap().to_owned();
    assert_eq!(body["device"]["peer_identity"], json!("dust@tailnet"));
    (device, token)
}

#[tokio::test]
async fn phone_page_origin_is_accepted_and_a_cross_site_origin_is_refused() {
    use axum::http::header;

    let (_dir, hub, _rx) = test_hub().await;
    let store = hub.store.clone();
    let app = crate::routes::router(
        crate::routes::App {
            store: store.clone(),
            runs: hub.runs.clone(),
            runner: hub.runner.clone(),
            stream: hub.stream.clone(),
            hub: Some(hub.clone()),
        },
        crate::auth::LocalAuth::with_store("cap-token".into(), 4310, Some(store.clone())),
    );
    let (_device, token) = paired_over_tailnet(&app, &store).await;

    // The phone approves from the page it loaded: https://<tailnet host>.
    // Before the fix this was a 403 and the phone could not approve at all.
    let phone = Request::builder()
        .method("GET")
        .uri("/api/devices")
        .header(header::HOST, "polyclave.tailnet.ts.net")
        .header("Tailscale-User-Login", "dust@tailnet")
        .header(header::ORIGIN, "https://polyclave.tailnet.ts.net")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(phone).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .and_then(|v| v.to_str().ok()),
        Some("https://polyclave.tailnet.ts.net")
    );

    // A page on another site is refused even if it somehow has the device token.
    let cross_site = Request::builder()
        .method("GET")
        .uri("/api/devices")
        .header(header::HOST, "polyclave.tailnet.ts.net")
        .header("Tailscale-User-Login", "dust@tailnet")
        .header(header::ORIGIN, "https://evil.example")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(cross_site).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );

    // Same host but plain http is not the page Tailscale serves; refuse it.
    let http_origin = Request::builder()
        .method("GET")
        .uri("/api/devices")
        .header(header::HOST, "polyclave.tailnet.ts.net")
        .header("Tailscale-User-Login", "dust@tailnet")
        .header(header::ORIGIN, "http://polyclave.tailnet.ts.net")
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(http_origin).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );

    // The exchange makes the device usable for the phone's own page: a second
    // exchange from a foreign origin is refused too.
    let (_, secret2) = store
        .create_pairing_ticket(polyclave_proto::ClientKind::Ios, "Second".to_owned(), 600)
        .unwrap();
    let foreign_exchange = Request::builder()
        .method("POST")
        .uri("/api/pairing/exchange")
        .header(header::HOST, "polyclave.tailnet.ts.net")
        .header("Tailscale-User-Login", "dust@tailnet")
        .header(header::ORIGIN, "https://evil.example")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "secret": secret2 }).to_string()))
        .unwrap();
    assert_eq!(
        app.clone()
            .oneshot(foreign_exchange)
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
}

/// Item 3.2: the QR names the origin the phone can actually reach, and with no
/// configured origin it hands out no link at all instead of an unusable one.
#[tokio::test]
async fn pairing_qr_points_at_the_configured_tailscale_origin() {
    use axum::body::to_bytes;
    use axum::http::header;

    // Only a bare https origin is accepted: a path, a query, userinfo or plain
    // http would make the scanned link point somewhere else.
    assert_eq!(
        pairing_origin_from_env(Some("https://polyclave.tailnet.ts.net/".into())).as_deref(),
        Some("https://polyclave.tailnet.ts.net")
    );
    for bad in [
        "http://polyclave.tailnet.ts.net",
        "https://polyclave.tailnet.ts.net/app",
        // Userinfo in the authority: built at run time so no literal address is
        // committed, and refused all the same.
        &format!("https://{}@polyclave.tailnet.ts.net", "user"),
        "https://polyclave.tailnet.ts.net?x=1",
        "polyclave.tailnet.ts.net",
        "https://",
    ] {
        assert_eq!(pairing_origin_from_env(Some(bad.into())), None, "{bad}");
    }

    let (_dir, hub, _rx) = test_hub().await;
    let app = daemon_router(&hub);
    let create = |label: &'static str| {
        authed_post(
            "/api/pairing/tickets",
            json!({"client_kind": "ios", "device_label": label, "ttl_secs": 600}),
        )
    };

    // No origin configured: the secret is still issued, but no link is invented.
    let res = app.clone().oneshot(create("Unconfigured")).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&to_bytes(res.into_body(), 1024 * 1024).await.unwrap()).unwrap();
    assert!(body["secret"].as_str().is_some_and(|s| !s.is_empty()));
    assert_eq!(body["origin"], Value::Null);
    assert_eq!(body["qr_uri"], Value::Null);

    // Configured: the link is the page the phone opens, with the secret in the
    // fragment so it never reaches a server log.
    hub.set_pairing_origin(Some("https://polyclave.tailnet.ts.net".into()));
    let res = app.clone().oneshot(create("Configured")).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&to_bytes(res.into_body(), 1024 * 1024).await.unwrap()).unwrap();
    let secret = body["secret"].as_str().unwrap();
    assert_eq!(body["origin"], json!("https://polyclave.tailnet.ts.net"));
    assert_eq!(
        body["qr_uri"],
        json!(format!("https://polyclave.tailnet.ts.net/#pair={secret}"))
    );

    // The link is only useful if that origin can pair: the exchange accepts the
    // phone's own https origin, which is the one the QR names.
    let exchange = Request::builder()
        .method("POST")
        .uri("/api/pairing/exchange")
        .header(header::HOST, "polyclave.tailnet.ts.net")
        .header("Tailscale-User-Login", "dust@tailnet")
        .header(header::ORIGIN, "https://polyclave.tailnet.ts.net")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "secret": secret }).to_string()))
        .unwrap();
    let res = app.clone().oneshot(exchange).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&to_bytes(res.into_body(), 1024 * 1024).await.unwrap()).unwrap();
    assert_eq!(body["device"]["peer_identity"], json!("dust@tailnet"));
    hub.close().await;
}

/// Item 3.1: a device token only passes when the identity presented on the wire
/// is exactly the identity the device was paired with, and the only identity that
/// counts is the one Tailscale Serve sets.
#[tokio::test]
async fn remote_device_auth_requires_tailscale_identity_and_a_matching_binding() {
    use axum::body::to_bytes;
    use axum::http::header;

    let (_dir, hub, _rx) = test_hub().await;
    let store = hub.store.clone();
    let app = crate::routes::router(
        crate::routes::App {
            store: store.clone(),
            runs: hub.runs.clone(),
            runner: hub.runner.clone(),
            stream: hub.stream.clone(),
            hub: Some(hub.clone()),
        },
        crate::auth::LocalAuth::with_store("cap-token".into(), 4310, Some(store.clone())),
    );
    let (_, bound) = paired_over_tailnet(&app, &store).await;
    // A device paired on this machine has no persisted binding at all.
    let (_, local_secret) = store
        .create_pairing_ticket(polyclave_proto::ClientKind::Ios, "Local".to_owned(), 600)
        .unwrap();
    let (unbound, unbound_body) = store.exchange_pairing_ticket(&local_secret, None).unwrap();
    assert_eq!(unbound.peer_identity, None);
    let unbound_token = unbound_body;

    let remote = |uri: &str, token: &str| {
        let b = Request::builder()
            .method("GET")
            .uri(uri)
            .header(header::HOST, "polyclave.tailnet.ts.net")
            .header(header::ORIGIN, "https://polyclave.tailnet.ts.net")
            .header(header::AUTHORIZATION, format!("Bearer {token}"));
        b
    };

    // 1. No Tailscale identity at all: refused before the token is even read.
    let missing = remote("/api/devices", &bound).body(Body::empty()).unwrap();
    assert_eq!(
        app.clone().oneshot(missing).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );

    // 2. A Mesh node header is a caller-written label, not an identity.
    let mesh_only = remote("/api/devices", &bound)
        .header("X-AgentMesh-Node", "dust@tailnet")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(mesh_only).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );

    // 3. A token paired locally (no binding) cannot pass a remote check, even
    //    when it presents a well-formed Tailscale identity.
    let unbound_remote = remote("/api/devices", &unbound_token)
        .header("Tailscale-User-Login", "dust@tailnet")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(unbound_remote).await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );

    // The same unbound device still works from this machine: local pairing is
    // exactly what an unbound token is for.
    let loopback = Request::builder()
        .method("GET")
        .uri("/api/devices")
        .header(header::HOST, "127.0.0.1:4310")
        .header(header::AUTHORIZATION, format!("Bearer {unbound_token}"))
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        app.clone().oneshot(loopback).await.unwrap().status(),
        StatusCode::OK
    );

    // 4. Pairing itself refuses a request that only carries the mesh header.
    let (_, secret) = store
        .create_pairing_ticket(polyclave_proto::ClientKind::Ios, "Phone".to_owned(), 600)
        .unwrap();
    let mesh_exchange = Request::builder()
        .method("POST")
        .uri("/api/pairing/exchange")
        .header(header::HOST, "polyclave.tailnet.ts.net")
        .header(header::ORIGIN, "https://polyclave.tailnet.ts.net")
        .header("X-AgentMesh-Node", "dust@tailnet")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "secret": secret }).to_string()))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(mesh_exchange).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
    // The ticket was not spent by the refused attempt.
    let (device, _token) = store
        .exchange_pairing_ticket(&secret, Some("dust@tailnet".into()))
        .unwrap();
    assert_eq!(device.peer_identity.as_deref(), Some("dust@tailnet"));

    // A bound device still works from its own Tailscale identity.
    let bound_ok = remote("/api/devices", &bound)
        .header("Tailscale-User-Login", "dust@tailnet")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(bound_ok).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let _ = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    hub.close().await;
}

#[tokio::test]
async fn pairing_never_trusts_a_peer_identity_from_the_request_body() {
    use axum::body::to_bytes;
    use axum::http::header;

    let (_dir, hub, _rx) = test_hub().await;
    let store = hub.store.clone();
    let app = crate::routes::router(
        crate::routes::App {
            store: store.clone(),
            runs: hub.runs.clone(),
            runner: hub.runner.clone(),
            stream: hub.stream.clone(),
            hub: Some(hub.clone()),
        },
        crate::auth::LocalAuth::with_store("cap-token".into(), 4310, Some(store.clone())),
    );

    // A body field naming an identity is not part of the contract any more:
    // otherwise any client could claim someone else's tailnet login.
    let (_, secret) = store
        .create_pairing_ticket(polyclave_proto::ClientKind::Ios, "X".to_owned(), 600)
        .unwrap();
    let forged = Request::builder()
        .method("POST")
        .uri("/api/pairing/exchange")
        .header(header::HOST, "127.0.0.1:4310")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({"secret": secret, "peer_identity": "dust@tailnet"}).to_string(),
        ))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(forged).await.unwrap().status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );

    // Paired from the daemon's own machine, the device stays unbound to any
    // peer identity: nothing in the request can set one.
    let (_, secret2) = store
        .create_pairing_ticket(polyclave_proto::ClientKind::Ios, "Local".to_owned(), 600)
        .unwrap();
    let local = Request::builder()
        .method("POST")
        .uri("/api/pairing/exchange")
        .header(header::HOST, "127.0.0.1:4310")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "secret": secret2 }).to_string()))
        .unwrap();
    let resp = app.clone().oneshot(local).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value =
        serde_json::from_slice(&to_bytes(resp.into_body(), 1024 * 1024).await.unwrap()).unwrap();
    assert_eq!(body["device"]["peer_identity"], Value::Null);
}

/// Two Hub sessions that both use `fix-1` must get two Runs, not one shared.
#[tokio::test]
async fn two_hub_sessions_with_the_same_request_id_get_two_remote_runs() {
    let p = mesh_pair().await;
    p.primary
        .store
        .write(|db| {
            db.execute_batch(
                "INSERT INTO sessions(id,name,role,repo_cwd,profile_id,provider_model,created_at)
                 SELECT 'hub2','Second hub',role,repo_cwd,profile_id,provider_model,created_at
                 FROM sessions WHERE id='hub';",
            )?;
            Ok(())
        })
        .unwrap();
    insert_ticket(&p.primary.store, "t-a", "hub", &p.repo, 2, "active");
    insert_ticket(&p.primary.store, "t-b", "hub2", &p.repo, 2, "active");

    let first = p
        .primary
        .call_tool(
            &ToolScope::new(1, "hub", "hub", &p.repo),
            "dispatch_remote",
            remote_worker_run("t-a", "fix-1"),
        )
        .await
        .unwrap();
    let second = p
        .primary
        .call_tool(
            &ToolScope::new(2, "hub2", "hub", &p.repo),
            "dispatch_remote",
            remote_worker_run("t-b", "fix-1"),
        )
        .await
        .unwrap();

    assert_ne!(first["run_id"], second["run_id"]);
    assert_eq!(ticket_use(&p.primary.store, "t-a"), (1, "active".into()));
    assert_eq!(ticket_use(&p.primary.store, "t-b"), (1, "active".into()));
    let a = p
        .primary
        .store
        .remote_dispatch("hub", "fix-1")
        .unwrap()
        .unwrap();
    let b = p
        .primary
        .store
        .remote_dispatch("hub2", "fix-1")
        .unwrap()
        .unwrap();
    assert_eq!(a.remote_run_id.as_deref(), first["run_id"].as_str());
    assert_eq!(b.remote_run_id.as_deref(), second["run_id"].as_str());

    mesh_run_settled(&p.remote, first["run_id"].as_str().unwrap()).await;
    mesh_run_settled(&p.remote, second["run_id"].as_str().unwrap()).await;
    p.srv.abort();
    p.remote.close().await;
    p.primary.close().await;
}
