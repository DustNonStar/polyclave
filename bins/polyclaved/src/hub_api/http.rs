use super::*;
use crate::routes::{HttpError, Reply};
use polyclave_hub::{
    council::{start_council as run_start_council, StartCouncilRequest},
    evolution::{
        build_redacted_community_draft, distill_local_suggestions, sync_skill_to_isolated_homes,
    },
    notify::{
        validate_private_endpoint, HttpPrivateNotifier, NotificationMetadata, PrivateNotifier,
    },
    pr_gate::{resume_all as pr_gate_resume_all, ReviewerSpec},
};
use polyclave_proto::{
    ClientKind, CouncilLevel, EvolutionStatus, MeshNodeStatus, NotificationChannelKind,
    PlanningPreference, Vendor,
};
use polyclave_store::stage3::{NewMeshNode, NewNotificationChannel};
use polyclave_store::stage4::NewLocalSkill;

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
        .route("/pr-gates/{id}/diff", axum::routing::get(pr_gate_diff))
        .route("/pr-gates/{id}/approve", post(approve_pr_gate))
        .route("/attention", axum::routing::get(attention_items))
        .route("/pause", axum::routing::get(pause_state).post(pause))
        .route("/resume", post(resume))
        .route(
            "/pairing/tickets",
            axum::routing::get(pairing_tickets).post(create_pairing_ticket),
        )
        .route("/devices", axum::routing::get(devices))
        .route("/devices/{id}/revoke", post(revoke_device))
        .route(
            "/mesh/nodes",
            axum::routing::get(mesh_nodes).post(upsert_mesh_node),
        )
        .route("/mesh/nodes/{id}/revoke", post(revoke_mesh_node))
        .route("/mesh/limits", axum::routing::get(mesh_limits))
        .route("/mesh/nodes/{id}/concurrency", post(set_mesh_concurrency))
        .route(
            "/notifications/channels",
            axum::routing::get(notification_channels).post(upsert_notification_channel),
        )
        .route(
            "/notifications/channels/{id}",
            axum::routing::delete(delete_notification_channel),
        )
        .route("/notifications/test", post(test_notifications))
        .route("/cua", axum::routing::get(cua_state))
        .route("/cua/acquire", post(cua_acquire))
        .route("/cua/release", post(cua_release))
        .route("/cua/reclaim", post(cua_reclaim))
        .route(
            "/evolution/suggestions",
            axum::routing::get(evolution_suggestions),
        )
        .route("/evolution/distill", post(evolution_distill))
        .route("/evolution/suggestions/{id}/apply", post(evolution_apply))
        .route(
            "/evolution/suggestions/{id}/rollback",
            post(evolution_rollback),
        )
        .route(
            "/skills",
            axum::routing::get(local_skills).post(save_local_skill),
        )
        .route("/skills/{id}/rollback", post(rollback_local_skill))
        .route("/community/draft", axum::routing::get(community_draft))
        .route("/community/opt-in", post(community_opt_in))
        .route("/community/submit", post(community_submit))
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
        Error::Policy | Error::Profile | Error::Publish => HttpError(
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

async fn attention_items(State(hub): State<Hub>) -> Reply {
    let items = run_blocking(move || {
        use polyclave_proto::attention::{
            council_needs_attention, pr_gate_needs_attention, run_needs_attention,
        };
        let mut out = Vec::new();
        for run in hub.runs.list().map_err(HttpError::from)? {
            if let Some(title) = run_needs_attention(run.status) {
                out.push(json!({"topic": "run", "id": run.id, "title": title}));
            }
        }
        for gate in hub.store.list_pr_gates().map_err(HttpError::from)? {
            if let Some(title) = pr_gate_needs_attention(gate.status) {
                out.push(json!({"topic": "pr_gate", "id": gate.id, "title": title}));
            }
        }
        for council in hub.store.list_councils().map_err(HttpError::from)? {
            if let Some(title) = council_needs_attention(council.status) {
                out.push(json!({"topic": "council", "id": council.id, "title": title}));
            }
        }
        for ticket in hub.store.hub_tickets().map_err(HttpError::from)? {
            if ticket.status == "blocked_on_user" {
                out.push(json!({
                    "topic": "ticket",
                    "id": ticket.id,
                    "title": "Ticket awaiting your input",
                }));
            }
        }
        Ok::<_, HttpError>(out)
    })
    .await?;
    Ok(Json(json!(items)))
}

/// The mirror diff for exactly the gate's `base_sha..head_sha`. A change bigger
/// than the page budget is cut and marked `truncated`, never silently shortened.
const MAX_PAGE_DIFF_BYTES: usize = 65_536;

/// Maps a [\`Hub\`] error to one HTTP error, so a refused or missing gate is a 4xx
/// rather than an opaque 500.
fn map_page_err(err: super::Error) -> HttpError {
    match err {
        super::Error::Policy => HttpError(
            StatusCode::CONFLICT,
            "policy",
            "The host refused this operation",
        ),
        super::Error::Credential => HttpError(
            StatusCode::CONFLICT,
            "scm_unconfigured",
            "GitHub credentials are not configured",
        ),
        super::Error::Http { status, detail } => {
            let code = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
            HttpError(code, "github_error", detail)
        }
        _ => HttpError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "Host operation failed",
        ),
    }
}

async fn pr_gate_diff(State(hub): State<Hub>, Path(id): Path<String>) -> Reply {
    let diff = run_blocking(move || {
        hub.pr_gate_diff(&id, MAX_PAGE_DIFF_BYTES)
            .map_err(map_page_err)
    })
    .await?;
    Ok(Json(json!(diff)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApprovePrGateBody {
    /// The SHA the person saw. `seen_sha` is the same field under its older name.
    #[serde(alias = "seen_sha")]
    expected_sha: String,
    hub_session_id: Option<String>,
}

/// A person approving one exact SHA. Nothing is armed before this call, and this
/// call arms only that SHA.
async fn approve_pr_gate(
    State(hub): State<Hub>,
    Path(id): Path<String>,
    Json(body): Json<ApprovePrGateBody>,
) -> Reply {
    if body.expected_sha.len() != 40
        || !body
            .expected_sha
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        return Err(HttpError(
            StatusCode::BAD_REQUEST,
            "invalid_sha",
            "expected_sha must be a full commit id",
        ));
    }
    let gate = hub
        .approve_pr_gate(&id, &body.expected_sha, body.hub_session_id)
        .await
        .map_err(map_page_err)?;
    Ok(Json(json!(gate)))
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
    head_sha: Option<String>,
    pr_title: Option<String>,
    pr_body: Option<String>,
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
                head_sha: body.head_sha,
                pr_title: body.pr_title,
                pr_body: body.pr_body,
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
    let gate = hub
        .start_pr_gate_async(deps, req)
        .await
        .map_err(map_hub_err)?;
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

async fn pairing_tickets(State(hub): State<Hub>) -> Reply {
    let list =
        run_blocking(move || hub.store.list_pairing_tickets().map_err(HttpError::from)).await?;
    Ok(Json(json!(list)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreatePairingTicketBody {
    client_kind: ClientKind,
    device_label: String,
    ttl_secs: Option<i64>,
}

async fn create_pairing_ticket(
    State(hub): State<Hub>,
    Json(body): Json<CreatePairingTicketBody>,
) -> Reply {
    let ttl = body.ttl_secs.unwrap_or(600);
    let origin = hub.pairing_origin();
    let store = hub.store.clone();
    let (ticket, secret) = run_blocking(move || {
        store
            .create_pairing_ticket(body.client_kind, body.device_label, ttl)
            .map_err(HttpError::from)
    })
    .await?;
    // The phone scans this and opens it in a browser, so the link must name the
    // origin that browser can actually reach: the configured Tailscale HTTPS
    // origin. A `polyclave://` URI is undecodable by a phone browser that has no
    // handler installed, which is exactly the dead end the audit found. With no
    // origin configured there is no link to hand out, and the caller is told so
    // rather than being given one that cannot work.
    let qr_uri = origin
        .as_ref()
        .map(|origin| format!("{origin}/#pair={secret}"));
    Ok(Json(json!({
        "ticket": ticket,
        "secret": secret,
        "origin": origin,
        "qr_uri": qr_uri,
    })))
}

async fn devices(State(hub): State<Hub>) -> Reply {
    let list =
        run_blocking(move || hub.store.list_paired_devices().map_err(HttpError::from)).await?;
    Ok(Json(json!(list)))
}

async fn revoke_device(State(hub): State<Hub>, Path(id): Path<String>) -> Reply {
    let device =
        run_blocking(move || hub.store.revoke_paired_device(&id).map_err(HttpError::from)).await?;
    let _ = hub
        .stream
        .send((device.id.clone(), super::StreamEvent::DeviceRevoked));
    Ok(Json(json!(device)))
}

async fn mesh_nodes(State(hub): State<Hub>) -> Reply {
    let list = run_blocking(move || hub.store.list_mesh_nodes().map_err(HttpError::from)).await?;
    Ok(Json(json!(list)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpsertMeshNodeBody {
    id: String,
    name: String,
    endpoint: String,
    os_family: String,
    allowed_repos: Vec<String>,
    allowed_actions: Vec<String>,
    status: Option<MeshNodeStatus>,
}

async fn upsert_mesh_node(State(hub): State<Hub>, Json(body): Json<UpsertMeshNodeBody>) -> Reply {
    if validate_private_endpoint(&body.endpoint).is_err() {
        return Err(HttpError(
            StatusCode::CONFLICT,
            "policy_violation",
            "Mesh node endpoint must be loopback, private RFC1918, or Tailscale",
        ));
    }
    let node = run_blocking(move || {
        hub.store
            .upsert_mesh_node(NewMeshNode {
                id: body.id,
                name: body.name,
                endpoint: body.endpoint,
                os_family: body.os_family,
                allowed_repos: body.allowed_repos,
                allowed_actions: body.allowed_actions,
                status: body.status.unwrap_or(MeshNodeStatus::Online),
            })
            .map_err(HttpError::from)
    })
    .await?;
    Ok(Json(json!(node)))
}

async fn revoke_mesh_node(State(hub): State<Hub>, Path(id): Path<String>) -> Reply {
    let node =
        run_blocking(move || hub.store.revoke_mesh_node(&id).map_err(HttpError::from)).await?;
    Ok(Json(json!(node)))
}

async fn mesh_limits(State(hub): State<Hub>) -> Reply {
    let list = run_blocking(move || hub.store.mesh_node_limits().map_err(HttpError::from)).await?;
    Ok(Json(json!(list)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MeshConcurrencyBody {
    max_concurrent_runs: u32,
}

async fn set_mesh_concurrency(
    State(hub): State<Hub>,
    Path(id): Path<String>,
    Json(body): Json<MeshConcurrencyBody>,
) -> Reply {
    let limit = run_blocking(move || {
        hub.store
            .set_mesh_node_concurrency(&id, body.max_concurrent_runs)
            .map_err(HttpError::from)
    })
    .await?;
    Ok(Json(json!(limit)))
}

async fn notification_channels(State(hub): State<Hub>) -> Reply {
    let list = run_blocking(move || {
        hub.store
            .list_notification_channels()
            .map_err(HttpError::from)
    })
    .await?;
    Ok(Json(json!(list)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpsertNotificationChannelBody {
    id: Option<String>,
    kind: NotificationChannelKind,
    label: String,
    endpoint: String,
    topic: String,
    enabled: Option<bool>,
}

async fn upsert_notification_channel(
    State(hub): State<Hub>,
    Json(body): Json<UpsertNotificationChannelBody>,
) -> Reply {
    if validate_private_endpoint(&body.endpoint).is_err() {
        return Err(HttpError(
            StatusCode::CONFLICT,
            "policy_violation",
            "Notification endpoint must be private, loopback, or Tailscale",
        ));
    }
    let ch = run_blocking(move || {
        hub.store
            .upsert_notification_channel(NewNotificationChannel {
                id: body.id,
                kind: body.kind,
                label: body.label,
                endpoint: body.endpoint,
                topic: body.topic,
                enabled: body.enabled.unwrap_or(true),
            })
            .map_err(HttpError::from)
    })
    .await?;
    Ok(Json(json!(ch)))
}

async fn delete_notification_channel(State(hub): State<Hub>, Path(id): Path<String>) -> Reply {
    run_blocking(move || {
        hub.store
            .delete_notification_channel(&id)
            .map_err(HttpError::from)
    })
    .await?;
    Ok(Json(json!({ "deleted": true })))
}

async fn test_notifications(State(hub): State<Hub>) -> Reply {
    let channels = run_blocking(move || {
        hub.store
            .list_notification_channels()
            .map_err(HttpError::from)
    })
    .await?;
    let notifier = HttpPrivateNotifier::new().map_err(map_hub_err)?;
    let meta = NotificationMetadata::sanitized("test", "host", "Polyclave wakeup test", 0);
    let mut delivered = 0usize;
    for ch in channels.into_iter().filter(|c| c.enabled) {
        notifier.notify(&ch, &meta).await.map_err(map_hub_err)?;
        delivered += 1;
    }
    Ok(Json(json!({ "delivered": delivered })))
}

async fn cua_state(State(hub): State<Hub>) -> Reply {
    let state = run_blocking(move || hub.store.cua_state().map_err(HttpError::from)).await?;
    Ok(Json(json!(state)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CuaAcquireBody {
    holder_run_id: String,
    target_window: String,
    observed_idle_secs: Option<u32>,
}

async fn cua_acquire(State(hub): State<Hub>, Json(body): Json<CuaAcquireBody>) -> Reply {
    let state = run_blocking(move || {
        let idle = body
            .observed_idle_secs
            .unwrap_or_else(|| polyclave_os::desktop_idle_secs().unwrap_or(0));
        hub.store
            .cua_acquire(body.holder_run_id, body.target_window, idle)
            .map_err(HttpError::from)
    })
    .await?;
    Ok(Json(json!(state)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CuaReleaseBody {
    holder_run_id: String,
}

async fn cua_release(State(hub): State<Hub>, Json(body): Json<CuaReleaseBody>) -> Reply {
    let state = run_blocking(move || {
        hub.store
            .cua_release(&body.holder_run_id)
            .map_err(HttpError::from)
    })
    .await?;
    Ok(Json(json!(state)))
}

async fn cua_reclaim(State(hub): State<Hub>) -> Reply {
    let state = run_blocking(move || hub.store.cua_reclaim().map_err(HttpError::from)).await?;
    Ok(Json(json!(state)))
}

async fn evolution_suggestions(State(hub): State<Hub>) -> Reply {
    let list = run_blocking(move || {
        hub.store
            .list_evolution_suggestions()
            .map_err(HttpError::from)
    })
    .await?;
    Ok(Json(json!(list)))
}

async fn evolution_distill(State(hub): State<Hub>) -> Reply {
    let created =
        run_blocking(move || distill_local_suggestions(&hub.store).map_err(map_hub_err)).await?;
    Ok(Json(json!(created)))
}

async fn evolution_apply(State(hub): State<Hub>, Path(id): Path<String>) -> Reply {
    let updated = run_blocking(move || {
        hub.store
            .set_evolution_status(&id, EvolutionStatus::Applied)
            .map_err(HttpError::from)
    })
    .await?;
    Ok(Json(json!(updated)))
}

async fn evolution_rollback(State(hub): State<Hub>, Path(id): Path<String>) -> Reply {
    let updated = run_blocking(move || {
        hub.store
            .set_evolution_status(&id, EvolutionStatus::RolledBack)
            .map_err(HttpError::from)
    })
    .await?;
    Ok(Json(json!(updated)))
}

async fn local_skills(State(hub): State<Hub>) -> Reply {
    let list = run_blocking(move || hub.store.list_local_skills().map_err(HttpError::from)).await?;
    Ok(Json(json!(list)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveSkillBody {
    name: String,
    description: String,
    body_md: String,
    synced_vendors: Option<Vec<Vendor>>,
}

async fn save_local_skill(State(hub): State<Hub>, Json(body): Json<SaveSkillBody>) -> Reply {
    let vendors = body
        .synced_vendors
        .unwrap_or_else(|| vec![Vendor::Codex, Vendor::Claude, Vendor::Agy, Vendor::Dsh]);
    let saved = run_blocking(move || {
        let skill = hub
            .store
            .save_local_skill(NewLocalSkill {
                name: body.name,
                description: body.description,
                body_md: body.body_md,
                synced_vendors: vendors,
            })
            .map_err(HttpError::from)?;
        sync_skill_to_isolated_homes(&hub.state, &skill).map_err(map_hub_err)?;
        Ok(skill)
    })
    .await?;
    Ok(Json(json!(saved)))
}

async fn rollback_local_skill(State(hub): State<Hub>, Path(id): Path<String>) -> Reply {
    let rolled = run_blocking(move || {
        let skill = hub
            .store
            .rollback_local_skill(&id)
            .map_err(HttpError::from)?;
        sync_skill_to_isolated_homes(&hub.state, &skill).map_err(map_hub_err)?;
        Ok(skill)
    })
    .await?;
    Ok(Json(json!(rolled)))
}

async fn community_draft(State(hub): State<Hub>) -> Reply {
    let draft =
        run_blocking(move || build_redacted_community_draft(&hub.store).map_err(map_hub_err))
            .await?;
    Ok(Json(json!(draft)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CommunityOptInBody {
    opt_in: bool,
}

async fn community_opt_in(State(hub): State<Hub>, Json(body): Json<CommunityOptInBody>) -> Reply {
    let opt_in = run_blocking(move || {
        hub.store
            .set_community_opt_in(body.opt_in)
            .map_err(HttpError::from)
    })
    .await?;
    Ok(Json(json!({ "opt_in": opt_in })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CommunitySubmitBody {
    repo_slug: Option<String>,
    title: String,
    body: String,
    confirmed: bool,
}

async fn community_submit(State(hub): State<Hub>, Json(body): Json<CommunitySubmitBody>) -> Reply {
    let opted_in = run_blocking({
        let store = hub.store.clone();
        move || store.community_opt_in().map_err(HttpError::from)
    })
    .await?;
    if !opted_in || !body.confirmed {
        return Err(HttpError(
            StatusCode::CONFLICT,
            "policy",
            "Community opt-in and explicit confirmation are required",
        ));
    }
    let token = run_blocking(|| {
        polyclave_os::read_provider_credential("Polyclave.GitHub", "bot_token").map_err(|_| {
            HttpError(
                StatusCode::CONFLICT,
                "credential_missing",
                "GitHub credential is not configured",
            )
        })
    })
    .await?;
    let api_base = std::env::var("POLYCLAVE_COMMUNITY_API_BASE")
        .unwrap_or_else(|_| "https://api.github.com".to_owned());
    let result = polyclave_hub::evolution::submit_redacted_community_draft(
        &hub.store,
        &api_base,
        &token,
        polyclave_hub::evolution::CommunitySubmitRequest {
            repo_slug: body
                .repo_slug
                .unwrap_or_else(|| "DustNonStar/polyclave".to_owned()),
            title: body.title,
            body: body.body,
            confirmed: body.confirmed,
        },
    )
    .await
    .map_err(map_hub_err)?;
    Ok(Json(json!(result)))
}

pub fn mesh_router(hub: Hub) -> Router {
    Router::new()
        .route("/health", axum::routing::get(mesh_health))
        .route("/dispatch", axum::routing::post(mesh_dispatch))
        .with_state(hub)
}

async fn mesh_health(State(hub): State<Hub>) -> Json<polyclave_mesh::RemoteHealth> {
    Json(polyclave_mesh::RemoteHealth {
        node_id: hub.host.clone(),
        ok: true,
    })
}

async fn mesh_dispatch(
    State(hub): State<Hub>,
    Json(req): Json<polyclave_mesh::RemoteDispatchRequest>,
) -> Response {
    let paused = hub.store.pause_state().map(|p| p.paused).unwrap_or(true);
    if paused {
        return StatusCode::CONFLICT.into_response();
    }
    let Some(first_scope) = req.token.chain.first() else {
        return StatusCode::FORBIDDEN.into_response();
    };
    let Ok(node) = hub.store.get_mesh_node(&first_scope.node_id) else {
        return StatusCode::FORBIDDEN.into_response();
    };
    let policy = hub.mesh_policy();
    if policy
        .authorize(
            &req.token,
            &node,
            &req.repo_cwd,
            &req.action,
            polyclave_mesh::current_epoch_secs(),
        )
        .is_err()
    {
        return StatusCode::FORBIDDEN.into_response();
    }
    match req.action.as_str() {
        "ping" => Json(json!({
            "ok": true,
            "node_id": node.id,
            "action": "ping"
        }))
        .into_response(),
        "list_repo_files" => match RepoScope::from_registered(FsPath::new(&req.repo_cwd))
            .and_then(|scope| scope.list_repo_files(None, None))
        {
            Ok(files) => Json(json!({ "ok": true, "files": files })).into_response(),
            Err(_) => StatusCode::FORBIDDEN.into_response(),
        },
        "search_repo" => match RepoScope::from_registered(FsPath::new(&req.repo_cwd))
            .and_then(|scope| scope.search_repo(&req.instructions, None))
        {
            Ok(hits) => Json(json!({ "ok": true, "matches": hits })).into_response(),
            Err(_) => StatusCode::FORBIDDEN.into_response(),
        },
        "worker_run" => mesh_worker_run(hub, node.id, req).await,
        "run_status" => mesh_run_status(hub, node.id, req).await,
        _ => StatusCode::FORBIDDEN.into_response(),
    }
}

/// Name of the per-repository Hub session that owns Runs admitted over the mesh.
/// Keeping them apart from the user's own Hub chat means a remote Run never wakes
/// a local Hub turn, and `request_id` repeats map to one stable admission key.
const MESH_HUB_SESSION_NAME: &str = "Mesh remote";

fn remote_worker_result(node_id: String, run: polyclave_proto::RunDto) -> Response {
    let status = serde_json::to_value(run.status)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default();
    Json(polyclave_mesh::RemoteWorkerResult {
        ok: !matches!(
            run.status,
            polyclave_proto::RunStatus::Failed | polyclave_proto::RunStatus::Interrupted
        ),
        node_id,
        run_id: run.id,
        status,
        result_sha: run.result_sha,
        observed_sha: run.observed_sha,
        detail: run.detail,
    })
    .into_response()
}

/// Finds or creates the mesh Hub session for one canonical repository in a single
/// write, so concurrent admissions agree on the same idempotency scope.
fn mesh_hub_session(store: &Store, repo_text: String) -> polyclave_store::Result<String> {
    store.write(move |db| {
        let existing: Option<String> = db
            .query_row(
                "SELECT id FROM sessions WHERE role='hub' AND repo_cwd=?1 AND name=?2
                 ORDER BY created_at,id LIMIT 1",
                params![repo_text, MESH_HUB_SESSION_NAME],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = existing {
            return Ok(id);
        }
        let stamp = now(db)?;
        db.execute(
            "INSERT OR IGNORE INTO provider_profiles(id,display_name,kind,models_json,created_at)
             VALUES('builtin-hub','Codex CLI','codex_cli','[\"default\"]',?1)",
            [&stamp],
        )?;
        let id = format!("mesh-{}", uuid::Uuid::new_v4().simple());
        db.execute(
            "INSERT INTO sessions(id,name,role,repo_cwd,profile_id,provider_model,created_at)
             VALUES(?1,?2,'hub',?3,'builtin-hub','default',?4)",
            params![id, MESH_HUB_SESSION_NAME, repo_text, stamp],
        )?;
        Ok(id)
    })
}

async fn mesh_worker_run(
    hub: Hub,
    node_id: String,
    req: polyclave_mesh::RemoteDispatchRequest,
) -> Response {
    if hub.is_closed() {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    // The sender's request_id is the only idempotency key: a retried dispatch
    // must map to the Run it already admitted, so a missing one is refused
    // rather than replaced by a fresh random id that would admit a second Run.
    let Some(request_id) = req.request_id.clone().filter(|id| !id.trim().is_empty()) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let Ok(vendor) = serde_json::from_value::<Vendor>(json!(req.vendor_or_default())) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let Ok(repo) = polyclave_os::canonical_real_path(FsPath::new(&req.repo_cwd)) else {
        return StatusCode::FORBIDDEN.into_response();
    };
    let Some(repo_text) = repo.to_str().map(str::to_owned) else {
        return StatusCode::FORBIDDEN.into_response();
    };
    let model = req.model.clone().filter(|m| !m.trim().is_empty());
    let base_branch = req.base_branch_or_default().to_owned();
    let node = node_id.clone();
    let admitted = run_blocking(move || {
        Ok((move || {
            let session = mesh_hub_session(&hub.store, repo_text)
                .map_err(|e| MeshAdmission::Refused(HttpError::from(e)))?;
            let request = RunRequest {
                hub_session_id: session,
                request_id,
                vendor,
                model,
                repo_cwd: repo,
                instructions: req.instructions,
                base_branch,
                ticket_id: None,
            };
            // At most N Runs from this node may be pending or running here; a resend
            // of an admitted request_id still returns its Run, even at the limit.
            let guard: Arc<dyn polyclave_core::runs::AdmissionGuard> =
                Arc::new(MeshNodeCapacity(node));
            let (run, new) = match hub.runs.accept_guarded(request, Some(guard)) {
                Ok(admitted) => admitted,
                Err(polyclave_store::Error::ConcurrencyLimit(limit)) => {
                    return Err(MeshAdmission::AtLimit(limit))
                }
                Err(err) => return Err(MeshAdmission::Refused(HttpError::from(err))),
            };
            if !new {
                return Ok(run);
            }
            let worker = hub
                .runner
                .schedule(&run.id, TurnMode::Start, run.instructions.clone())
                .map_err(|e| MeshAdmission::Refused(HttpError::from(e)))?;
            // The Worker thread finishes on its own and records its outcome in this
            // node's ledger. No local Hub turn is woken: the primary polls `run_status`
            // and keeps R1 verification of the result for itself.
            drop(worker);
            hub.runs
                .get(&run.id)
                .map_err(|e| MeshAdmission::Refused(HttpError::from(e)))
        })())
    })
    .await;
    match admitted {
        Ok(Ok(run)) => remote_worker_result(node_id, run),
        Ok(Err(MeshAdmission::AtLimit(limit))) => mesh_at_limit(limit),
        Ok(Err(MeshAdmission::Refused(err))) | Err(err) => err.into_response(),
    }
}

/// Why a mesh admission was not made.
enum MeshAdmission {
    AtLimit(u32),
    Refused(HttpError),
}

/// Retryable refusal: nothing was admitted or queued, so the sender may resend
/// the same `request_id` once a slot frees.
fn mesh_at_limit(limit: u32) -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        [(axum::http::header::RETRY_AFTER, "30")],
        Json(json!({
            "code": "mesh_concurrency_limit",
            "message": "This node already has its maximum number of active mesh Runs; retry the same request_id later.",
            "limit": limit,
            "retryable": true,
        })),
    )
        .into_response()
}

/// Per-node concurrency cap, checked and recorded inside the admission.
struct MeshNodeCapacity(String);

impl polyclave_core::runs::AdmissionGuard for MeshNodeCapacity {
    fn check(&self, db: &rusqlite::Connection) -> polyclave_store::Result<()> {
        polyclave_store::mesh_limits::check_mesh_capacity(db, &self.0)
    }
    fn admitted(&self, db: &rusqlite::Connection, run_id: &str) -> polyclave_store::Result<()> {
        polyclave_store::mesh_limits::record_mesh_admission(db, &self.0, run_id)
    }
}

async fn mesh_run_status(
    hub: Hub,
    node_id: String,
    req: polyclave_mesh::RemoteDispatchRequest,
) -> Response {
    let Some(run_id) = req.run_id.clone().filter(|id| !id.trim().is_empty()) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let Ok(repo) = polyclave_os::canonical_real_path(FsPath::new(&req.repo_cwd)) else {
        return StatusCode::FORBIDDEN.into_response();
    };
    let found = run_blocking(move || match hub.runs.get(&run_id) {
        Ok(run) => Ok(Some(run)),
        Err(polyclave_store::Error::NotFound) => Ok(None),
        Err(err) => Err(HttpError::from(err)),
    })
    .await;
    match found {
        Ok(Some(run)) if FsPath::new(&run.repo_cwd) == repo => remote_worker_result(node_id, run),
        // A token for one repository must not read Runs of another. A missing Run
        // gets the same answer, so a probe cannot tell the two apart.
        Ok(_) => StatusCode::FORBIDDEN.into_response(),
        Err(err) => err.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    #[tokio::test]
    async fn map_page_err_preserves_dynamic_github_error_without_leaking() {
        let resp = map_page_err(super::super::Error::Http {
            status: 422,
            detail: "custom upstream error".to_owned(),
        })
        .into_response();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = to_bytes(resp.into_body(), 4096).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["code"], "github_error");
        assert_eq!(parsed["message"], "custom upstream error");
    }
}
