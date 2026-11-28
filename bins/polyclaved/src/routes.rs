use crate::auth::{self, LocalAuth};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    middleware,
    response::{
        sse::{Event, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{get, post},
    Extension, Json, Router,
};
use polyclave_core::{
    runner::TurnRunner,
    runs::{Resolution, RunManager, RunRequest},
};
use polyclave_proto::{ApiError, ChangeTopic, RunStatus, TurnMode};
use polyclave_store::{new_id, now, Changes, Error, Store};
use rusqlite::params;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{convert::Infallible, path::PathBuf, time::Duration};
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct App {
    pub store: Store,
    pub runs: RunManager,
    pub runner: TurnRunner,
    pub stream: broadcast::Sender<(String, crate::hub_api::StreamEvent)>,
    pub hub: Option<crate::hub_api::Hub>,
}

pub fn router(app: App, auth: LocalAuth) -> Router {
    let mesh_hub = app.hub.clone();
    let extension = app.hub.clone().map(crate::hub_api::router);
    let public_api = Router::new()
        .route("/pairing/exchange", post(exchange_pairing))
        .with_state(app.clone())
        .layer(axum::Extension(auth.clone()));
    let api = Router::new()
        .route("/sessions", get(sessions).post(register))
        .route("/runs", get(runs).post(accept))
        .route("/runs/{id}", get(run))
        .route("/runs/{id}/worker-chat", post(chat))
        .route("/runs/{id}/cancel", post(cancel))
        .route("/runs/{id}/retry", post(retry))
        .route("/runs/{id}/resolve", post(resolve))
        .route("/events", get(events))
        .fallback(|| async { StatusCode::NOT_FOUND })
        .with_state(app);
    let api = if let Some(extension) = extension {
        api.merge(extension)
    } else {
        api
    };
    let api = api
        .layer(middleware::from_fn_with_state(auth, auth::require))
        .merge(public_api);
    let directory = resolve_ui_dir();
    let root = Router::new().nest("/api", api);
    let root = if let Some(hub) = mesh_hub {
        root.nest("/mesh", crate::hub_api::mesh_router(hub))
    } else {
        root
    };
    root.fallback_service(tower_http::set_header::SetResponseHeader::overriding(
        tower_http::services::ServeDir::new(directory),axum::http::header::CONTENT_SECURITY_POLICY,
        axum::http::HeaderValue::from_static("default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; connect-src 'self'; base-uri 'none'; frame-ancestors 'none'")
    ))
}

fn resolve_ui_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("POLYCLAVE_UI_DIR") {
        return PathBuf::from(dir);
    }
    let cwd_dist = PathBuf::from("apps/polyclave-app/ui/dist");
    if cwd_dist.is_dir() {
        return cwd_dist;
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for candidate in [
                dir.join("ui/dist"),
                dir.join("../Resources/ui/dist"),
                dir.join("../../apps/polyclave-app/ui/dist"),
                dir.join("../../../apps/polyclave-app/ui/dist"),
            ] {
                if candidate.is_dir() {
                    return candidate;
                }
            }
        }
    }
    cwd_dist
}

pub(crate) type Reply = std::result::Result<Json<Value>, HttpError>;
pub(crate) struct HttpError {
    status: StatusCode,
    code: &'static str,
    message: std::borrow::Cow<'static, str>,
}
#[allow(non_snake_case)]
pub(crate) fn HttpError(
    status: StatusCode,
    code: &'static str,
    message: impl Into<std::borrow::Cow<'static, str>>,
) -> HttpError {
    HttpError {
        status,
        code,
        message: message.into(),
    }
}
impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ApiError {
                code: self.code.into(),
                message: self.message.into_owned(),
            }),
        )
            .into_response()
    }
}
impl From<Error> for HttpError {
    fn from(error: Error) -> Self {
        match error {
            Error::NotFound => HttpError(StatusCode::NOT_FOUND, "not_found", "Not found"),
            Error::BusyError => HttpError(StatusCode::CONFLICT, "busy", "The run is busy"),
            Error::BudgetExceededError => HttpError(
                StatusCode::CONFLICT,
                "budget_exceeded",
                "Run or Attempt budget exhausted",
            ),
            Error::ConflictError => HttpError(
                StatusCode::CONFLICT,
                "conflict",
                "Not permitted in the current state",
            ),
            Error::Invalid(detail) => HttpError(StatusCode::BAD_REQUEST, "invalid_request", detail),
            _ => HttpError(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "Host operation failed",
            ),
        }
    }
}
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, Error> + Send + 'static,
) -> Result<T, HttpError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| {
            HttpError(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                "Host operation failed",
            )
        })?
        .map_err(Into::into)
}
async fn runs(State(app): State<App>) -> Reply {
    Ok(Json(json!(blocking(move || app.runs.list()).await?)))
}
async fn run(State(app): State<App>, Path(id): Path<String>) -> Reply {
    Ok(Json(
        blocking(move || {
            let run = app.runs.get(&id)?;
            let commit = app.runs.commit_for_run(&run)?;
            let evidence = app.runs.evidence_for_run(&run)?;
            Ok(json!({"run":run,"commit":commit,"evidence":evidence}))
        })
        .await?,
    ))
}
async fn accept(State(app): State<App>, Json(req): Json<RunRequest>) -> Reply {
    if blocking({
        let store = app.store.clone();
        move || store.pause_state()
    })
    .await?
    .paused
    {
        return Err(HttpError(
            StatusCode::CONFLICT,
            "paused",
            "Host is globally paused",
        ));
    }
    Ok(Json(json!(
        blocking(move || {
            let (run, new) = app.runs.accept_local_worker(req)?;
            if new {
                let attempt =
                    app.runner
                        .schedule(&run.id, TurnMode::Start, run.instructions.clone())?;
                if let Some(hub) = &app.hub {
                    hub.track(run.id.clone(), attempt);
                }
            }
            Ok(run)
        })
        .await?
    )))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Chat {
    text: String,
}
async fn chat(State(app): State<App>, Path(id): Path<String>, Json(c): Json<Chat>) -> Reply {
    if c.text.trim().is_empty() || c.text.len() > 1024 * 1024 {
        return Err(Error::Invalid("missing or oversized message").into());
    }
    blocking(move || {
        let run = app.runs.get(&id)?;
        if matches!(run.status, RunStatus::Done | RunStatus::Failed) {
            return Err(Error::ConflictError);
        }
        app.runs
            .check_local_worker_scope(run.vendor, &run.hub_session_id, Some(&run.ticket_id))?;
        let attempt = app.runner.schedule(&id, TurnMode::WorkerChat, c.text)?;
        if let Some(hub) = &app.hub {
            hub.track(id, attempt);
        }
        Ok(())
    })
    .await?;
    Ok(Json(json!({"accepted":true})))
}
async fn cancel(State(app): State<App>, Path(id): Path<String>) -> Reply {
    blocking(move || {
        app.runner.cancel(&id)?;
        app.runner.wait_for_cancelled_staging(&id)
    })
    .await?;
    Ok(Json(json!({"accepted":true,"broker_stopped":true})))
}
async fn retry(State(app): State<App>, Path(id): Path<String>) -> Reply {
    if blocking({
        let store = app.store.clone();
        move || store.pause_state()
    })
    .await?
    .paused
    {
        return Err(HttpError(
            StatusCode::CONFLICT,
            "paused",
            "Host is globally paused",
        ));
    }
    blocking(move || {
        let run = app.runs.get(&id)?;
        app.runs
            .check_local_worker_scope(run.vendor, &run.hub_session_id, Some(&run.ticket_id))?;
        let attempt = app
            .runner
            .schedule(&id, TurnMode::Retry, run.instructions)?;
        if let Some(hub) = &app.hub {
            hub.track(id, attempt);
        }
        Ok(())
    })
    .await?;
    Ok(Json(json!({"accepted":true})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Resolve {
    decision: Resolution,
    note: String,
    seen_sha: Option<String>,
}
async fn resolve(State(app): State<App>, Path(id): Path<String>, Json(r): Json<Resolve>) -> Reply {
    Ok(Json(json!(
        blocking(move || app
            .runs
            .resolve_seen(&id, r.decision, &r.note, r.seen_sha.as_deref()))
        .await?
    )))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Register {
    name: String,
    repo_cwd: PathBuf,
}
async fn register(State(app): State<App>, Json(input): Json<Register>) -> Reply {
    if input.name.trim().is_empty() || input.name.len() > 256 {
        return Err(Error::Invalid("invalid session name").into());
    }
    Ok(Json(blocking(move||{
        let repo=polyclave_os::canonical_real_path(&input.repo_cwd).map_err(|_|Error::Invalid("invalid repository path"))?;
        let repo=repo.to_str().ok_or(Error::Invalid("repository path is not valid Unicode"))?.to_owned();
        app.store.write(move|db|{
            let stamp=now(db)?;
            db.execute("INSERT OR IGNORE INTO provider_profiles(id,display_name,kind,models_json,created_at)
                VALUES('builtin-hub','Local hub','codex_cli','[]',?1)",[&stamp])?;
            let id=new_id('s');
            db.execute("INSERT INTO sessions(id,name,role,repo_cwd,profile_id,provider_model,created_at)
                VALUES(?1,?2,'hub',?3,'builtin-hub','default',?4)",params![id,input.name,repo,stamp])?;
            let session = json!({"id":id,"name":input.name,"role":"hub","repo_cwd":repo,"profile_id":"builtin-hub","provider_model":"default",
                "vendor":null,"rolling_summary":"","agent_thread_id":null,"created_at":stamp});
            Store::append_change(db,ChangeTopic::Inbox,&id,&session)?;
            Ok(session)
        })
    }).await?))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExchangePairingBody {
    secret: String,
}

/// Exchanges a one-time pairing ticket for a device token.
///
/// This route has no token yet — the phone has not got one — so it validates the
/// request instead: exactly one `Host`, no self-declared identity, and an
/// `Origin` that is either absent or this very host over https. The peer
/// identity comes only from the proxy header, never from the body: a body field
/// would let any client claim `dust@tailnet` and satisfy the dual-identity check
/// for a device it never proved.
async fn exchange_pairing(
    State(app): State<App>,
    Extension(auth): Extension<LocalAuth>,
    headers: HeaderMap,
    Json(body): Json<ExchangePairingBody>,
) -> Reply {
    // Only the identity the trusted proxy sets. `X-AgentMesh-Node` is a routing
    // label any client can send, so it can never stand as a pairing identity.
    let peer = headers
        .get("tailscale-user-login")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let host = if headers.get_all(axum::http::header::HOST).iter().count() == 1 {
        headers
            .get(axum::http::header::HOST)
            .and_then(|v| v.to_str().ok())
    } else {
        None
    };
    // Either the loopback page (the daemon's own machine) or a request the
    // trusted proxy tagged with an identity may pair.
    let refused = || {
        HttpError(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Pairing request refused",
        )
    };
    let Some(host) = host else {
        return Err(refused());
    };
    if host != auth.loopback_host() && peer.is_none() {
        return Err(refused());
    }
    match headers.get(axum::http::header::ORIGIN) {
        Some(_) if headers.get_all(axum::http::header::ORIGIN).iter().count() != 1 => {
            return Err(refused());
        }
        // The rule the rest of the API uses: the desktop page, the Tauri shell
        // and the phone's own https page may pair; nothing else may.
        Some(origin)
            if !origin
                .to_str()
                .is_ok_and(|o| auth.origin_allowed(o, Some(host), peer.as_deref())) =>
        {
            return Err(refused());
        }
        _ => {}
    }
    let (device, token) =
        blocking(move || app.store.exchange_pairing_ticket(&body.secret, peer)).await?;
    Ok(Json(json!({ "device": device, "token": token })))
}

async fn sessions(State(app): State<App>) -> Reply {
    Ok(Json(json!(blocking(move||app.store.read(|db|{
        let mut q=db.prepare("SELECT id,name,role,repo_cwd,profile_id,provider_model,vendor,rolling_summary,agent_thread_id,created_at FROM sessions ORDER BY id")?;
        let list=q.query_map([],|r|Ok(json!({
            "id":r.get::<_,String>(0)?,"name":r.get::<_,String>(1)?,"role":r.get::<_,String>(2)?,
            "repo_cwd":r.get::<_,String>(3)?,"profile_id":r.get::<_,String>(4)?,"provider_model":r.get::<_,String>(5)?,
            "vendor":r.get::<_,Option<String>>(6)?,"rolling_summary":r.get::<_,String>(7)?,
            "agent_thread_id":r.get::<_,Option<String>>(8)?,"created_at":r.get::<_,String>(9)?
        })))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(list)
    })).await?)))
}

async fn events(State(app): State<App>, headers: HeaderMap) -> Result<Response, HttpError> {
    let mut cursor =
        match headers.get("last-event-id") {
            None => None,
            Some(v) => Some(v.to_str().ok().and_then(|v| v.parse::<i64>().ok()).ok_or(
                HttpError(
                    StatusCode::BAD_REQUEST,
                    "invalid_cursor",
                    "Last-Event-ID must be an integer",
                ),
            )?),
        };
    let mut streaming = app.stream.subscribe();
    let changes = app.store.clone();
    let stream = async_stream::stream! {
        let mut ticks=tokio::time::interval(Duration::from_millis(200));
        let mut transient_open=true;
        loop {
            tokio::select! {
                _=ticks.tick()=>{
                    let changes=changes.clone();
                    match tokio::task::spawn_blocking(move||changes.changes(cursor)).await{
                        Ok(Ok(Changes::Events(events)))=>for item in events {
                            cursor=Some(item.seq);
                            let ev=Event::default().event("change").id(item.seq.to_string()).json_data(item);
                            if let Ok(ev)=ev {yield Ok::<Event,Infallible>(ev);}
                        },
                        _=>{
                            yield Ok(Event::default().event("resync_required").data("{}"));
                            break;
                        }
                    }
                },
                event=streaming.recv(), if transient_open=>match event{
                    Ok((run, crate::hub_api::StreamEvent::Resync)) => {
                        let _ = run;
                        yield Ok(Event::default().event("resync_required").data("{}"));
                        break;
                    }
                    Ok((_, crate::hub_api::StreamEvent::Shutdown)) => break,
                    Ok((run, message)) => {
                        if let Ok(ev)=Event::default().event("agent").json_data(json!({"run_id":run,"message":message})) {
                            yield Ok(ev);
                        }
                    },
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        yield Ok(Event::default().event("resync_required").data("{}"));
                        break;
                    }
                    Err(broadcast::error::RecvError::Closed)=>transient_open=false,
                }
            }
        }
    };
    Ok(Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request};
    use http_body_util::BodyExt;
    use polyclave_core::{mirror::MirrorManager, runner::RunnerConfig};
    use polyclave_proto::ChangeTopic;
    use std::sync::Arc;
    use tower::ServiceExt;

    fn fixture() -> (tempfile::TempDir, Store, Router) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("db")).unwrap();
        let runs = RunManager::new(
            store.clone(),
            Arc::new(MirrorManager::new(dir.path()).unwrap()),
        );
        let runner = TurnRunner::new(
            runs.clone(),
            dir.path(),
            "fixture".into(),
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
        let (stream, _) = broadcast::channel(8);
        let app = router(
            App {
                store: store.clone(),
                runs,
                runner,
                stream,
                hub: None,
            },
            LocalAuth::new("test-only".into(), 4310),
        );
        (dir, store, app)
    }

    #[tokio::test]
    async fn direct_sandboxed_dispatch_requires_an_explicit_ticket_scope() {
        let (dir, store, app) = fixture();
        for vendor in ["codex", "claude"] {
            let response = app
                .clone()
                .oneshot(
                    Request::post("/api/runs")
                        .header("host", "127.0.0.1:4310")
                        .header("authorization", "Bearer test-only")
                        .header("content-type", "application/json")
                        .body(Body::from(
                            json!({
                                "hub_session_id": "hub",
                                "request_id": vendor,
                                "vendor": vendor,
                                "repo_cwd": dir.path(),
                                "instructions": "edit the repository",
                                "base_branch": "main"
                            })
                            .to_string(),
                        ))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert!(String::from_utf8_lossy(&body).contains("Ticket with allowed paths"));
        }
        let count = store
            .read(|db| Ok(db.query_row("SELECT count(*) FROM runs", [], |r| r.get::<_, u32>(0))?))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn origin_host_and_bearer_are_independent_requirements() {
        let (_dir, _store, app) = fixture();
        for (host, origin, bearer, expected) in [
            ("127.0.0.1:4310", None, None, StatusCode::UNAUTHORIZED),
            (
                "127.0.0.1:4310",
                Some("https://wrong.example.invalid"),
                Some("Bearer test-only"),
                StatusCode::FORBIDDEN,
            ),
            (
                "wrong.example.invalid",
                None,
                Some("Bearer test-only"),
                StatusCode::FORBIDDEN,
            ),
            (
                "127.0.0.1:4310",
                None,
                Some("Bearer wrong"),
                StatusCode::UNAUTHORIZED,
            ),
            (
                "127.0.0.1:4310",
                Some("tauri://localhost"),
                Some("Bearer test-only"),
                StatusCode::OK,
            ),
        ] {
            let mut request = Request::get("/api/sessions").header("host", host);
            if let Some(o) = origin {
                request = request.header("origin", o);
            }
            if let Some(b) = bearer {
                request = request.header("authorization", b);
            }
            assert_eq!(
                app.clone()
                    .oneshot(request.body(Body::empty()).unwrap())
                    .await
                    .unwrap()
                    .status(),
                expected
            );
        }
        let query = Request::get("/api/events?token=test-only")
            .header("host", "127.0.0.1:4310")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            app.oneshot(query).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );
    }

    async fn event(app: Router, cursor: &str) -> String {
        let response = app
            .oneshot(
                Request::get("/api/events")
                    .header("host", "127.0.0.1:4310")
                    .header("authorization", "Bearer test-only")
                    .header("last-event-id", cursor)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let mut body = response.into_body();
        let frame = tokio::time::timeout(Duration::from_secs(2), body.frame())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        String::from_utf8(frame.into_data().unwrap().to_vec()).unwrap()
    }
    #[tokio::test]
    async fn sse_resumes_after_cursor_and_requests_resync_for_ahead_cursor() {
        let (_dir, store, app) = fixture();
        store
            .write(|db| {
                Store::append_change(db, ChangeTopic::Run, "first", &json!({"status":"running"}))?;
                Store::append_change(db, ChangeTopic::Run, "second", &json!({"status":"done"}))?;
                Ok(())
            })
            .unwrap();
        let replay = event(app.clone(), "1").await;
        assert!(replay.contains("event: change"));
        assert!(replay.contains("id: 2"));
        assert!(replay.contains("second"));
        assert!(!replay.contains("first"));
        assert!(event(app, "3").await.contains("event: resync_required"));
    }
    #[tokio::test]
    async fn both_terminal_run_states_reject_worker_chat_with_http_409() {
        let (_dir, store, app) = fixture();
        store.write(|db|{
            db.execute_batch("
                INSERT INTO provider_profiles(id,display_name,kind,models_json,created_at) VALUES('p','Test','codex_cli','[]','now');
                INSERT INTO sessions(id,name,role,repo_cwd,profile_id,provider_model,created_at)
                    VALUES('hub','Hub','hub','repo','p','fixture','now'),('wd','Worker','worker','repo','p','fixture','now'),('wf','Worker','worker','repo','p','fixture','now');
                INSERT INTO tickets(id,hub_session_id,repo_cwd,title,mandate_json,source_kind,status,created_at,updated_at)
                    VALUES('ticket','hub','repo','Test','{}','explicit_user','active','now','now');
                INSERT INTO runs(id,ticket_id,hub_session_id,worker_session_id,request_id,repo_cwd,vendor,instructions,
                    mirror_remote,base_branch,base_sha,status,created_at,updated_at)
                    VALUES('done','ticket','hub','wd','d','repo','codex','test','mirror','main','base','done','now','now'),
                          ('failed','ticket','hub','wf','f','repo','codex','test','mirror','main','base','failed','now','now');
            ")?;
            Ok(())
        }).unwrap();
        for id in ["done", "failed"] {
            let response = app
                .clone()
                .oneshot(
                    Request::post(format!("/api/runs/{id}/worker-chat"))
                        .header("host", "127.0.0.1:4310")
                        .header("authorization", "Bearer test-only")
                        .header("content-type", "application/json")
                        .body(Body::from(r#"{"text":"must not run"}"#))
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::CONFLICT);
        }
        store
            .read(|db| {
                assert_eq!(
                    db.query_row("SELECT count(*) FROM turns", [], |r| r.get::<_, i64>(0))?,
                    0
                );
                Ok(())
            })
            .unwrap();
    }

    #[tokio::test]
    async fn register_session_appends_change_log() {
        let (dir, store, app) = fixture();
        let repo = dir.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        let res = app
            .oneshot(
                Request::post("/api/sessions")
                    .header("host", "127.0.0.1:4310")
                    .header("authorization", "Bearer test-only")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"name": "Main", "repo_cwd": repo}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let Changes::Events(events) = store.changes(None).unwrap() else {
            panic!("expected change events");
        };
        assert!(events.iter().any(|e| e.topic == ChangeTopic::Inbox));
    }
}
