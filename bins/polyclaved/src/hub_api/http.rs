use super::*;
use crate::routes::{HttpError, Reply};

pub(super) async fn mcp(
    State(hub): State<Hub>,
    headers: HeaderMap,
    Json(input): Json<Value>,
) -> Response {
    if headers.contains_key("origin") {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(secret) = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(str::to_owned)
    else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let (store, host) = (hub.store.clone(), hub.host.clone());
    let Ok(Ok(Some((turn, session, role, repo)))) =
        spawn_blocking(move || store.hub_tool_scope(&secret, &host)).await
    else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let id = input.get("id").cloned().unwrap_or(Value::Null);
    if input.get("id").is_none() {
        return StatusCode::ACCEPTED.into_response();
    }
    let result = match input["method"].as_str() {
        Some("initialize") => {
            json!({"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"polyclave-hub","version":"0.1.0"}})
        }
        Some("tools/list") => json!({"tools":tool_list(role=="hub")}),
        Some("tools/call") => {
            let Some(name) = input["params"]["name"].as_str().map(str::to_owned) else {
                return StatusCode::BAD_REQUEST.into_response();
            };
            let args = input["params"]["arguments"].clone();
            let scope = ToolScope::new(turn, &session, &role, &repo);
            let h = hub.clone();
            match spawn_blocking(move || h.tool(&scope, &name, args)).await {
                Ok(Ok(v)) => {
                    json!({"content":[{"type":"text","text":v.to_string()}],"isError":false})
                }
                _ => {
                    json!({"content":[{"type":"text","text":"Tool request refused"}],"isError":true})
                }
            }
        }
        Some("ping") => json!({}),
        _ => {
            return Json(
                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Unknown method"}}),
            )
            .into_response()
        }
    };
    Json(json!({"jsonrpc":"2.0","id":id,"result":result})).into_response()
}

pub fn router(hub: Hub) -> Router {
    Router::new()
        .route(
            "/profiles/{id}/credential-binding",
            axum::routing::get(binding),
        )
        .route("/tickets", axum::routing::get(tickets))
        .route("/profiles", axum::routing::get(profiles))
        .route("/sessions/{id}/profile", post(profile))
        .route("/sessions/{id}/messages", post(enqueue).get(messages))
        .route("/sessions/{id}/cancel", post(cancel))
        .route("/sessions/{id}/tickets", post(ticket))
        .route("/sessions/{id}/manager", post(manager))
        .route("/sessions/{id}/repo-file", post(repo_file))
        .route("/usage", axum::routing::get(usage))
        .route("/sessions/{id}/memory", post(memory).get(memories))
        .with_state(hub)
}

async fn run_blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, HttpError> + Send + 'static,
) -> Result<T, HttpError> {
    spawn_blocking(f).await.map_err(|_| {
        HttpError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "Host operation failed",
        )
    })?
}

#[derive(Deserialize)]
struct Input {
    request_id: String,
    text: String,
}
async fn enqueue(
    State(hub): State<Hub>,
    Path(id): Path<String>,
    Json(input): Json<Input>,
) -> Reply {
    if hub.is_closed() {
        return Err(HttpError(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Hub is shutting down",
        ));
    }
    let store = hub.store.clone();
    let session = id.clone();
    let msg = run_blocking(move || {
        store
            .hub_enqueue(&session, &input.request_id, &input.text)
            .map_err(HttpError::from)
    })
    .await?;
    hub.kick(id);
    Ok(Json(json!(msg)))
}
async fn messages(State(hub): State<Hub>, Path(id): Path<String>) -> Reply {
    let list = run_blocking(move || hub.store.hub_messages(&id).map_err(HttpError::from)).await?;
    Ok(Json(json!(list)))
}
#[derive(Deserialize)]
struct Ticket {
    message_id: i64,
    title: String,
    mandate: Mandate,
    max_runs: Option<u32>,
}
async fn ticket(State(hub): State<Hub>, Path(id): Path<String>, Json(req): Json<Ticket>) -> Reply {
    let created = run_blocking(move || {
        hub.triage
            .explicit(
                &id,
                req.message_id,
                &req.title,
                req.mandate,
                req.max_runs.unwrap_or(4),
            )
            .map_err(|_| {
                HttpError(
                    StatusCode::CONFLICT,
                    "conflict",
                    "Could not create ticket in the current state",
                )
            })
    })
    .await?;
    Ok(Json(json!(created)))
}
#[derive(Deserialize)]
struct Manager {
    profile_id: String,
    model: String,
    name: String,
}
async fn manager(
    State(hub): State<Hub>,
    Path(id): Path<String>,
    Json(req): Json<Manager>,
) -> Reply {
    let manager_id = run_blocking(move || {
        hub.store
            .register_manager(&id, &req.profile_id, &req.model, &req.name)
            .map_err(HttpError::from)
    })
    .await?;
    Ok(Json(json!({"id": manager_id})))
}
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum Memory {
    DraftRun {
        run_id: String,
        kind: MemoryKind,
        title: String,
        content: String,
    },
    Draft {
        message_id: i64,
        kind: MemoryKind,
        title: String,
        content: String,
    },
    Commit {
        id: String,
    },
    Supersede {
        id: String,
        prior: String,
    },
    Archive {
        id: String,
    },
}
async fn memory(State(hub): State<Hub>, Path(id): Path<String>, Json(req): Json<Memory>) -> Reply {
    let result = run_blocking(move || {
        let (_, repo, _) = hub.store.hub_scope(&id).map_err(HttpError::from)?;
        let c = MemoryCurator::for_host(hub.store.clone(), repo.clone(), Curator::User);
        let outcome = match req {
            Memory::DraftRun {
                run_id,
                kind,
                title,
                content,
            } => hub
                .store
                .draft_run_correction(&repo, &run_id, kind, &title, &content)
                .map_err(|_| Error::Policy),
            Memory::Draft {
                message_id,
                kind,
                title,
                content,
            } => c.draft_correction(&id, message_id, kind, &title, &content),
            Memory::Commit { id } => c.memory_commit(&id),
            Memory::Supersede { id, prior } => c.memory_supersede(&id, &prior),
            Memory::Archive { id } => c.memory_archive(&id),
        };
        outcome.map_err(|_| {
            HttpError(
                StatusCode::CONFLICT,
                "conflict",
                "Memory operation rejected in the current state",
            )
        })
    })
    .await?;
    Ok(Json(json!(result)))
}

async fn profiles(State(hub): State<Hub>) -> Reply {
    let list = run_blocking(move || hub.store.hub_profiles().map_err(HttpError::from)).await?;
    Ok(Json(json!(list)))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Profile {
    profile_id: String,
    model: String,
}
async fn profile(State(hub): State<Hub>, Path(id): Path<String>, Json(p): Json<Profile>) -> Reply {
    let store = hub.store.clone();
    let session = id.clone();
    run_blocking(move || {
        store
            .hub_switch_profile(&session, &p.profile_id, &p.model)
            .map_err(HttpError::from)?;
        store
            .write(move |db| {
                Store::append_change(
                    db,
                    ChangeTopic::Inbox,
                    &session,
                    &json!({"id": session, "profile_id": p.profile_id, "provider_model": p.model}),
                )
            })
            .map_err(HttpError::from)
    })
    .await?;
    hub.kick(id);
    Ok(Json(json!({"updated":true})))
}

async fn binding(State(hub): State<Hub>, Path(id): Path<String>) -> Reply {
    let (service, account) = run_blocking(move || {
        let p = ApiHubProvider::from_registered(&hub.store, &id).map_err(|_| {
            HttpError(
                StatusCode::NOT_FOUND,
                "not_found",
                "Provider profile not found",
            )
        })?;
        let identity = p.credential_binding();
        Ok((identity.service.clone(), identity.account.clone()))
    })
    .await?;
    Ok(Json(json!({"service": service, "account": account})))
}
async fn tickets(State(hub): State<Hub>) -> Reply {
    let list = run_blocking(move || hub.store.hub_tickets().map_err(HttpError::from)).await?;
    Ok(Json(json!(list)))
}

async fn memories(State(hub): State<Hub>, Path(id): Path<String>) -> Reply {
    let list = run_blocking(move || {
        let (_, repo, _) = hub.store.hub_scope(&id).map_err(HttpError::from)?;
        hub.store
            .find_memory(MemoryFilter {
                repo: Some(&repo),
                limit: Some(50),
                ..Default::default()
            })
            .map_err(HttpError::from)
    })
    .await?;
    Ok(Json(json!(list)))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileQuery {
    path: String,
    start_line: Option<usize>,
    end_line: Option<usize>,
}
async fn repo_file(
    State(hub): State<Hub>,
    Path(id): Path<String>,
    Json(query): Json<FileQuery>,
) -> Reply {
    let text = run_blocking(move || {
        let (_, repo, _) = hub.store.hub_scope(&id).map_err(HttpError::from)?;
        let scope = RepoScope::from_registered(FsPath::new(&repo)).map_err(|_| {
            HttpError(
                StatusCode::FORBIDDEN,
                "forbidden",
                "Repository scope unavailable",
            )
        })?;
        scope
            .read_repo_file(FsPath::new(&query.path), query.start_line, query.end_line)
            .map_err(|_| {
                HttpError(
                    StatusCode::FORBIDDEN,
                    "forbidden",
                    "Requested file cannot be read",
                )
            })
    })
    .await?;
    Ok(Json(json!({"text": text})))
}
async fn usage(State(hub): State<Hub>) -> Reply {
    let rows = run_blocking(move || hub.store.hub_local_usage().map_err(HttpError::from)).await?;
    Ok(Json(json!(rows)))
}

async fn cancel(State(hub): State<Hub>, Path(id): Path<String>) -> Reply {
    let active = hub.lifecycle.sessions.lock().map_err(|_| {
        HttpError(
            StatusCode::SERVICE_UNAVAILABLE,
            "unavailable",
            "Session registry unavailable",
        )
    })?;
    let dispatch = active.get(&id).ok_or(HttpError(
        StatusCode::CONFLICT,
        "conflict",
        "No active Hub turn for session",
    ))?;
    dispatch.stop.cancel();
    Ok(Json(json!({"accepted": true})))
}
