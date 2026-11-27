use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use polyclave_adapters::mcp::{AttemptScope, HostTools, Mcp};
use polyclave_store::{Error, Store};
use serde_json::{json, Value};
use std::sync::{Arc, Barrier};
use tower::ServiceExt;

const WORKER: &str = "fixture-active";
const CONSULT: &str = "fixture-consult";
struct TestHost {
    gate: Option<Arc<Barrier>>,
}
impl HostTools for TestHost {
    fn verified_tip(&self, attempt: &AttemptScope, sha: &str) -> Result<bool, Error> {
        assert_eq!(attempt.branch, "agent/run/a1");
        assert_eq!(attempt.mirror, "host-mirror");
        if let Some(gate) = &self.gate {
            gate.wait();
            gate.wait();
        }
        if sha == "b".repeat(40) {
            return Err(Error::Closed);
        }
        Ok(sha == "a".repeat(40))
    }
    fn consult_read_only(&self, attempt: &AttemptScope, q: &str) -> Result<String, Error> {
        Ok(format!("{}: {q}", attempt.run))
    }
}

fn fixture(gate: Option<Arc<Barrier>>) -> (tempfile::TempDir, Store, Router) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("temp.sqlite")).unwrap();
    store.write(|db|{
        db.execute_batch("
            INSERT INTO provider_profiles(id,display_name,kind,models_json,created_at)
                VALUES('p','Test','codex_cli','[]','now');
            INSERT INTO sessions(id,name,role,repo_cwd,profile_id,provider_model,created_at)
                VALUES('s','Hub','hub','repo','p','test','now'), ('w','Worker','worker','repo','p','test','now');
            INSERT INTO tickets(id,hub_session_id,repo_cwd,title,mandate_json,source_kind,status,created_at,updated_at)
                VALUES('t','s','repo','Test','{}','explicit_user','active','now','now');
            INSERT INTO runs(id,ticket_id,hub_session_id,worker_session_id,request_id,repo_cwd,vendor,instructions,
                mirror_remote,base_branch,base_sha,status,current_attempt,created_at,updated_at)
                VALUES('run','t','s','w','request','repo','codex','test','host-mirror','main','base','running',1,'now','now');
            INSERT INTO attempts(run_id,attempt_no,branch,worktree_path,status,created_at)
                VALUES('run',1,'agent/run/a1','worktree','running','now');
            INSERT INTO turns(id,session_id,run_id,attempt_id,mode,status,phase,host_instance_id,boot_epoch,mcp_token,started_at)
                VALUES(1,'w','run',1,'start','running','authorized','host','boot','fixture-active','now'),
                      (2,'w','run',1,'worker_chat','completed','authorized','host','boot','fixture-ended','now'),
                      (3,'s',NULL,NULL,'hub','running','prepared','host','boot','fixture-prepared','now'),
                      (4,'s',NULL,NULL,'hub','completed','authorized','other','boot','fixture-other','now');
            INSERT INTO turns(id,session_id,run_id,attempt_id,parent_turn_id,mode,status,phase,host_instance_id,boot_epoch,mcp_token,started_at)
                VALUES(5,'w','run',1,1,'consult','running','authorized','host','boot','fixture-consult','now');
        ")?;
        Ok(())
    }).unwrap();
    let app = Mcp::new(store.clone(), "host", Arc::new(TestHost { gate })).router();
    (dir, store, app)
}

async fn rpc(app: Router, token: &str, method: &str, args: Value) -> (StatusCode, Value) {
    let request = Request::post("/mcp")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(
            json!({"jsonrpc":"2.0","id":1,"method":method,"params":args}).to_string(),
        ))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    (
        response.status(),
        serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap())
            .unwrap_or(Value::Null),
    )
}
async fn call(app: Router, token: &str, name: &str, args: Value) -> (StatusCode, Value) {
    rpc(
        app,
        token,
        "tools/call",
        json!({"name":name,"arguments":args}),
    )
    .await
}
fn submit(sha: &str) -> Value {
    json!({"run_id":"run","branch":"agent/run/a1","sha":sha.repeat(40)})
}
fn accepted(store: &Store) -> i64 {
    store
        .read(|db| {
            Ok(
                db.query_row("SELECT count(*) FROM results WHERE accepted=1", [], |r| {
                    r.get(0)
                })?,
            )
        })
        .unwrap()
}

#[tokio::test]
async fn invalid_wrong_instance_prepared_and_ended_turn_tokens_are_rejected() {
    let (_dir, store, app) = fixture(None);
    store.write(|db| {
        db.execute_batch("UPDATE turns SET status='completed' WHERE id=3; UPDATE turns SET status='running' WHERE id=4;")?;
        Ok(())
    }).unwrap();
    assert_eq!(
        rpc(app.clone(), "fixture-other", "ping", json!({})).await.0,
        StatusCode::UNAUTHORIZED
    );
    store.write(|db| {
        db.execute_batch("UPDATE turns SET status='completed' WHERE id=4; UPDATE turns SET status='running' WHERE id=3;")?;
        Ok(())
    }).unwrap();
    for token in ["fixture-unknown", "fixture-prepared", "fixture-ended"] {
        assert_eq!(
            rpc(app.clone(), token, "ping", json!({})).await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(rpc(app, WORKER, "ping", json!({})).await.0, StatusCode::OK);
}

#[tokio::test]
async fn idempotent_receipt_and_conflicting_sha_have_one_accepted_result() {
    let (_dir, store, app) = fixture(None);
    let req = submit("a");
    let (a, b) = tokio::join!(
        call(app.clone(), WORKER, "submit_result", req.clone()),
        call(app.clone(), WORKER, "submit_result", req)
    );
    assert_eq!(a.0, StatusCode::OK);
    assert_eq!(b.0, StatusCode::OK);
    let messages = [a, b].map(|(_, v)| {
        v["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .to_owned()
    });
    assert!(messages
        .iter()
        .any(|s| s == "Accepted and stored by the host."));
    assert!(messages.iter().any(|s| s == "Already accepted."));
    assert_eq!(accepted(&store), 1);
    let (_, result) = call(app, WORKER, "submit_result", submit("c")).await;
    assert_eq!(result["result"]["isError"], true);
    assert_eq!(accepted(&store), 1);
}

#[tokio::test]
async fn cancellation_during_host_verification_prevents_a_receipt() {
    let gate = Arc::new(Barrier::new(2));
    let (_dir, store, app) = fixture(Some(gate.clone()));
    let dispatch = tokio::spawn(call(app, WORKER, "submit_result", submit("a")));
    let s = store.clone();
    tokio::task::spawn_blocking(move || {
        gate.wait();
        let result = s.write(|db| {
            db.execute("UPDATE turns SET status='cancelled' WHERE id=1", [])?;
            Ok(())
        });
        gate.wait();
        result.unwrap();
    })
    .await
    .unwrap();
    assert_eq!(dispatch.await.unwrap().0, StatusCode::UNAUTHORIZED);
    assert_eq!(accepted(&store), 0);
}

#[tokio::test]
async fn candidate_proposal_cannot_escalate_or_be_read_as_verified() {
    let (_dir, store, app) = fixture(None);
    let p = json!({"kind":"decision","title":"candidateword","content":"candidateword"});
    let (_, proposed) = call(app.clone(), WORKER, "memory_propose", p.clone()).await;
    assert_eq!(proposed["result"]["isError"], false);
    let (_, read) = call(
        app.clone(),
        WORKER,
        "memory_search",
        json!({"query":"candidateword"}),
    )
    .await;
    assert_eq!(read["result"]["content"][0]["text"], "[]");
    let mut promotion = p;
    promotion["status"] = json!("verified");
    let (_, refused) = call(app.clone(), WORKER, "memory_propose", promotion).await;
    assert_eq!(refused["error"]["code"], -32602);
    for name in ["memory_commit", "dispatch_task"] {
        let (_, result) = call(app.clone(), WORKER, name, json!({})).await;
        assert_eq!(result["error"]["code"], -32601);
    }
    for name in ["consult", "memory_propose", "submit_result"] {
        let (_, result) = call(app.clone(), CONSULT, name, json!({})).await;
        assert_eq!(result["error"]["code"], -32601);
    }
    let (_, consulted) = call(app, WORKER, "consult", json!({"question":"what changed?"})).await;
    assert_eq!(
        consulted["result"]["content"][0]["text"],
        "run: what changed?"
    );
    let status = store
        .read(|db| {
            Ok(db.query_row("SELECT status FROM memory_items", [], |r| {
                r.get::<_, String>(0)
            })?)
        })
        .unwrap();
    assert_eq!(status, "candidate");
}

#[tokio::test]
async fn memory_brief_respects_serialized_json_byte_budget_and_search_requires_query() {
    let (_dir, store, app) = fixture(None);
    let (_, bad_search) = call(app.clone(), WORKER, "memory_search", json!({})).await;
    assert_eq!(bad_search["error"]["code"], -32602);
    store
        .write(|db| {
            for i in 0..8 {
                db.execute(
                    "INSERT INTO memory_items(id,scope,repo_cwd,kind,title,content,tags_json,status,created_by,created_at,updated_at)
                     VALUES(?1,'project','repo','decision',?2,?3,'[]','verified','user','now','now')",
                    rusqlite::params![format!("m{i}"), format!("Title {i}"), "x".repeat(1200)],
                )?;
            }
            Ok(())
        })
        .unwrap();
    let (status, brief) = call(app, WORKER, "memory_brief", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    let text = brief["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.len() <= 6000, "serialized json {} > 6000", text.len());
    assert!(!text.is_empty());
}
