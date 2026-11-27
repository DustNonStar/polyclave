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
}
impl MockPusher {
    fn new() -> Self {
        Self {
            pushed: Mutex::new(Vec::new()),
        }
    }
}
impl MirrorPusher for MockPusher {
    fn push_mirror_branch<'a>(
        &'a self,
        _mirror_path: &'a FsPath,
        refspec: &'a str,
        branch: &'a str,
    ) -> BoxFuture<'a, HubResult<()>> {
        self.pushed
            .lock()
            .unwrap()
            .push((refspec.to_owned(), branch.to_owned()));
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

fn setup_git_run_fixture(dir: &FsPath, hub: &Hub) -> (String, String, String) {
    let repo = polyclave_os::canonical_real_path(&dir.join("repo")).unwrap();
    git_cmd(&repo, &["init", "-b", "main", "--template="]);
    git_cmd(&repo, &["config", "user.name", "Fixture"]);
    git_cmd(&repo, &["config", "user.email", "fixture@example.invalid"]);
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join("src/lib.rs"), "pub fn v() -> i32 { 1 }\n").unwrap();
    git_cmd(&repo, &["add", "."]);
    git_cmd(&repo, &["commit", "-m", "initial"]);
    let base_sha = git_cmd(&repo, &["rev-parse", "HEAD"]);

    let mirror = dir.join("mirror.git");
    git_cmd(
        dir,
        &[
            "clone",
            "--bare",
            repo.to_str().unwrap(),
            mirror.to_str().unwrap(),
        ],
    );
    let mirror = polyclave_os::canonical_real_path(&mirror).unwrap();

    let work = dir.join("work");
    git_cmd(
        dir,
        &["clone", repo.to_str().unwrap(), work.to_str().unwrap()],
    );
    git_cmd(&work, &["config", "user.name", "Worker"]);
    git_cmd(&work, &["config", "user.email", "worker@example.invalid"]);
    std::fs::write(work.join("src/lib.rs"), "pub fn v() -> i32 { 2 }\n").unwrap();
    git_cmd(&work, &["add", "."]);
    git_cmd(&work, &["commit", "-m", "change"]);
    let head_sha = git_cmd(&work, &["rev-parse", "HEAD"]);

    let run_id = "r-gate-1".to_owned();
    let final_ref = format!("refs/polyclave/runs/{run_id}/final");
    git_cmd(
        &work,
        &[
            "push",
            mirror.to_str().unwrap(),
            &format!("HEAD:{final_ref}"),
        ],
    );

    let repo_str = repo.to_str().unwrap().to_owned();
    let mirror_str = mirror.to_str().unwrap().to_owned();
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
                "INSERT INTO runs(id,ticket_id,hub_session_id,worker_session_id,request_id,repo_cwd,vendor,model,instructions,mirror_remote,base_branch,base_sha,result_sha,status,created_at,updated_at)
                 VALUES(?1,'t-gate','hub','w-gate','req-gate',?2,'codex','gpt-5','do it',?3,'main',?4,?5,'done','now','now')",
                params![run_clone, repo_str, mirror_str, base_clone, head_clone],
            )?;
            Ok(())
        })
        .unwrap();
    (run_id, base_sha, head_sha)
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
    hub.store.arm_auto_merge(&gate.id, "head").unwrap();

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

    hub.start_scm_observer(scm, Some("hub".into()), Duration::from_secs(60))
        .await;
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
