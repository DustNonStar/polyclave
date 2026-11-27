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
