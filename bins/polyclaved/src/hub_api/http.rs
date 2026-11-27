use super::*;
use crate::routes::{HttpError, Reply};
use polyclave_hub::{
    council::{start_council as run_start_council, StartCouncilRequest},
    pr_gate::{resume_all as pr_gate_resume_all, run_pr_gate as execute_pr_gate, ReviewerSpec},
};
use polyclave_proto::{CouncilLevel, PlanningPreference};

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
            match hub.call_tool(&scope, &name, args).await {
                Ok(v) => {
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
        .route(
            "/councils",
            axum::routing::get(councils).post(start_council),
        )
        .route("/councils/{id}/resolve", post(resolve_council))
        .route("/pr-gates", axum::routing::get(pr_gates))
        .route("/pr-gates/{id}", axum::routing::get(pr_gate))
        .route("/pr-gates/{id}/run", post(run_pr_gate))
        .route("/pause", axum::routing::get(pause_state).post(pause))
        .route("/resume", post(resume))
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

fn map_hub_err(err: Error) -> HttpError {
    match err {
        Error::Policy | Error::Profile => HttpError(
            StatusCode::CONFLICT,
            "conflict",
            "Request refused in the current state",
        ),
        Error::Credential => HttpError(
            StatusCode::CONFLICT,
            "scm_unconfigured",
            "Required credentials are not configured",
        ),
        Error::Http { .. } | Error::Transport | Error::Protocol => HttpError(
            StatusCode::BAD_GATEWAY,
            "upstream_error",
            "Upstream SCM or provider request failed",
        ),
        _ => HttpError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "Host operation failed",
        ),
    }
}

async fn councils(State(hub): State<Hub>) -> Reply {
    let list = run_blocking(move || hub.store.list_councils().map_err(HttpError::from)).await?;
    Ok(Json(json!(list)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartCouncilBody {
    council_id: Option<String>,
    hub_session_id: Option<String>,
    ticket_id: Option<String>,
    repo_cwd: Option<String>,
    level: Option<CouncilLevel>,
    preference: Option<PlanningPreference>,
    prompt: String,
}

async fn start_council(State(hub): State<Hub>, Json(body): Json<StartCouncilBody>) -> Reply {
    if body.prompt.trim().is_empty() || body.prompt.len() > 256 * 1024 {
        return Err(HttpError(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Council prompt is required",
        ));
    }
    let store = hub.store.clone();
    if run_blocking(move || store.pause_state().map_err(HttpError::from))
        .await?
        .paused
    {
        return Err(HttpError(
            StatusCode::CONFLICT,
            "paused",
            "Host is globally paused",
        ));
    }
    let hub_for_prep = hub.clone();
    let req = run_blocking(move || {
        let repo_cwd = match (body.hub_session_id.as_deref(), body.repo_cwd) {
            (Some(sid), None) => {
                let (_, repo, _) = hub_for_prep.store.hub_scope(sid).map_err(HttpError::from)?;
                repo
            }
            (Some(sid), Some(repo)) => {
                let (_, scoped_repo, _) =
                    hub_for_prep.store.hub_scope(sid).map_err(HttpError::from)?;
                if scoped_repo != repo {
                    return Err(HttpError(
                        StatusCode::CONFLICT,
                        "conflict",
                        "Repository does not match hub session",
                    ));
                }
                repo
            }
            (None, Some(repo)) if !repo.trim().is_empty() => repo,
            _ => {
                return Err(HttpError(
                    StatusCode::BAD_REQUEST,
                    "invalid_request",
                    "Repository path or hub_session_id is required",
                ));
            }
        };
        let participants = hub_for_prep
            .resolve_council_participants(&repo_cwd)
            .map_err(map_hub_err)?;
        if participants.is_empty() {
            return Err(HttpError(
                StatusCode::CONFLICT,
                "no_participants",
                "No council participants are configured",
            ));
        }
        Ok(StartCouncilRequest {
            council_id: body.council_id,
            hub_session_id: body.hub_session_id,
            ticket_id: body.ticket_id,
            repo_cwd,
            level: body.level,
            preference: body.preference.unwrap_or(PlanningPreference::Balanced),
            prompt: body.prompt,
            participants,
        })
    })
    .await?;
    let council = run_start_council(&hub.store, req)
        .await
        .map_err(map_hub_err)?;
    Ok(Json(json!(council)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResolveCouncilBody {
    chosen_label: String,
    decision_note: Option<String>,
}

async fn resolve_council(
    State(hub): State<Hub>,
    Path(id): Path<String>,
    Json(body): Json<ResolveCouncilBody>,
) -> Reply {
    if body.chosen_label.trim().is_empty() {
        return Err(HttpError(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "chosen_label is required",
        ));
    }
    let decided = run_blocking(move || {
        hub.store
            .resolve_council(&id, &body.chosen_label, body.decision_note)
            .map_err(HttpError::from)
    })
    .await?;
    Ok(Json(json!(decided)))
}

async fn pr_gates(State(hub): State<Hub>) -> Reply {
    let list = run_blocking(move || hub.store.list_pr_gates().map_err(HttpError::from)).await?;
    Ok(Json(json!(list)))
}

async fn pr_gate(State(hub): State<Hub>, Path(id): Path<String>) -> Reply {
    let gate = run_blocking(move || hub.store.pr_gate(&id).map_err(HttpError::from)).await?;
    Ok(Json(json!(gate)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReviewerInput {
    pub vendor: Vendor,
    pub model: String,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RunPrGateBody {
    repo_slug: Option<String>,
    branch: Option<String>,
    base_branch: Option<String>,
    base_sha: Option<String>,
    head_sha: Option<String>,
    pr_title: Option<String>,
    pr_body: Option<String>,
    touched_paths: Option<Vec<String>>,
    is_high_risk: Option<bool>,
    reviewers: Option<Vec<ReviewerInput>>,
    hub_session_id: Option<String>,
}

async fn run_pr_gate(
    State(hub): State<Hub>,
    Path(id): Path<String>,
    body: Option<Json<RunPrGateBody>>,
) -> Reply {
    let body = body.map(|Json(b)| b).unwrap_or_default();
    let reviewers = body.reviewers.map(|items| {
        items
            .into_iter()
            .map(|r| ReviewerSpec {
                vendor: r.vendor,
                model: r.model,
            })
            .collect()
    });
    let hub_for_prep = hub.clone();
    let (req, deps, _) = run_blocking(move || {
        hub_for_prep
            .prepare_pr_gate_run(PrGateRunInputs {
                target_id: id,
                hub_session_id: body.hub_session_id,
                repo_slug: body.repo_slug,
                branch: body.branch,
                base_branch: body.base_branch,
                base_sha: body.base_sha,
                head_sha: body.head_sha,
                pr_title: body.pr_title,
                pr_body: body.pr_body,
                touched_paths: body.touched_paths,
                is_high_risk: body.is_high_risk,
                reviewers,
            })
            .map_err(|err| match err {
                PrGatePrepError::Paused => {
                    HttpError(StatusCode::CONFLICT, "paused", "Host is globally paused")
                }
                PrGatePrepError::NotFound => {
                    HttpError(StatusCode::NOT_FOUND, "not_found", "Not found")
                }
                PrGatePrepError::ScmUnconfigured => HttpError(
                    StatusCode::CONFLICT,
                    "scm_unconfigured",
                    "GitHub credentials are not configured",
                ),
                PrGatePrepError::Invalid(detail) => {
                    HttpError(StatusCode::BAD_REQUEST, "invalid_request", detail)
                }
                PrGatePrepError::Internal => HttpError(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal",
                    "Host operation failed",
                ),
            })
    })
    .await?;
    let gate = execute_pr_gate(&deps, req).await.map_err(map_hub_err)?;
    Ok(Json(json!(gate)))
}

async fn pause_state(State(hub): State<Hub>) -> Reply {
    let state = run_blocking(move || hub.store.pause_state().map_err(HttpError::from)).await?;
    Ok(Json(json!(state)))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct PauseBody {
    reason: Option<String>,
}

async fn pause(State(hub): State<Hub>, body: Option<Json<PauseBody>>) -> Reply {
    let reason = body.and_then(|Json(b)| b.reason);
    let state = hub.pause_all(reason).await.map_err(|err| match err {
        Error::Policy if hub.scm().is_none() => HttpError(
            StatusCode::CONFLICT,
            "scm_unconfigured",
            "Pause persisted, but GitHub credentials are missing to revoke armed auto-merges",
        ),
        other => map_hub_err(other),
    })?;
    Ok(Json(json!(state)))
}

async fn resume(State(hub): State<Hub>) -> Reply {
    let state = run_blocking(move || pr_gate_resume_all(&hub.store).map_err(map_hub_err)).await?;
    Ok(Json(json!(state)))
}
