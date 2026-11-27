use super::*;
use polyclave_hub::{
    council::{start_council as run_start_council, StartCouncilRequest},
    pr_gate::{run_pr_gate as execute_pr_gate, ReviewerSpec},
};
use polyclave_proto::{CouncilLevel, PlanningPreference};
use polyclave_store::{new_id, Error as StoreError};
use rusqlite::OptionalExtension;

impl Hub {
    pub(super) async fn execute_tool(
        &self,
        caller: ToolScope,
        name: String,
        args: Value,
        stop: Stop,
    ) -> HubResult<Value> {
        stop.check()?;
        if matches!(name.as_str(), "start_council" | "run_pr_gate") {
            let value = tokio::select! {
                biased;
                _ = stop.wait() => return Err(Error::Cancelled),
                res = self.call_tool(&caller, &name, args) => res?,
            };
            stop.check()?;
            return Ok(value);
        }
        let hub = self.clone();
        // Keep ownership of the blocking operation until it actually returns. Cancellation
        // must not release the turn while an admitted dispatch is still taking effect.
        blocking(move || {
            stop.check()?;
            let value = hub.tool(&caller, &name, args)?;
            stop.check()?;
            Ok(value)
        })
        .await
    }

    pub(super) async fn call_tool(
        &self,
        caller: &ToolScope,
        name: &str,
        mut args: Value,
    ) -> HubResult<Value> {
        if !tool_list(caller.role == "hub")
            .iter()
            .any(|t| t["name"] == name)
        {
            return Err(Error::Policy);
        }
        match name {
            "start_council" => {
                let scope = RepoScope::from_registered(FsPath::new(&caller.repo))?;
                if let Some(path) = args
                    .as_object_mut()
                    .ok_or(Error::Policy)?
                    .remove("repo_cwd")
                {
                    scope.matches_requested(FsPath::new(path.as_str().ok_or(Error::Policy)?))?;
                }
                let req_input: StartCouncilTool =
                    serde_json::from_value(args).map_err(|_| Error::Policy)?;
                let hub = self.clone();
                let caller_clone = caller.clone();
                let req = blocking(move || {
                    if hub
                        .store
                        .pause_state()
                        .map_err(|_| Error::Store)?
                        .paused
                    {
                        return Err(Error::Policy);
                    }
                    if let Some(ref ticket_id) = req_input.ticket_id {
                        let valid_ticket: bool = hub
                            .store
                            .read(|db| {
                                db.query_row(
                                    "SELECT EXISTS(SELECT 1 FROM tickets WHERE id=?1 AND hub_session_id=?2 AND repo_cwd=?3)",
                                    params![ticket_id, caller_clone.session, caller_clone.repo],
                                    |r| r.get(0),
                                )
                                .map_err(Into::into)
                            })
                            .map_err(|_| Error::Store)?;
                        if !valid_ticket {
                            return Err(Error::Policy);
                        }
                    }
                    let participants = hub.resolve_council_participants(&caller_clone.repo)?;
                    if participants.is_empty() {
                        return Err(Error::Profile);
                    }
                    Ok(StartCouncilRequest {
                        council_id: req_input.council_id,
                        hub_session_id: Some(caller_clone.session),
                        ticket_id: req_input.ticket_id,
                        repo_cwd: caller_clone.repo,
                        level: req_input.level,
                        preference: req_input.preference.unwrap_or(PlanningPreference::Balanced),
                        prompt: req_input.prompt,
                        participants,
                    })
                })
                .await?;
                let council = run_start_council(&self.store, req).await?;
                serde_json::to_value(council).map_err(|_| Error::Store)
            }
            "run_pr_gate" => {
                let scope = RepoScope::from_registered(FsPath::new(&caller.repo))?;
                if let Some(path) = args
                    .as_object_mut()
                    .ok_or(Error::Policy)?
                    .remove("repo_cwd")
                {
                    scope.matches_requested(FsPath::new(path.as_str().ok_or(Error::Policy)?))?;
                }
                let req_input: RunPrGateTool =
                    serde_json::from_value(args).map_err(|_| Error::Policy)?;
                let reviewers = req_input.reviewers.map(|list| {
                    list.into_iter()
                        .map(|r| ReviewerSpec {
                            vendor: r.vendor,
                            model: r.model,
                        })
                        .collect()
                });
                let hub = self.clone();
                let caller_clone = caller.clone();
                let (req, deps) = blocking(move || {
                    let (req, deps, run) = hub
                        .prepare_pr_gate_run(PrGateRunInputs {
                            target_id: req_input.run_id,
                            hub_session_id: Some(caller_clone.session.clone()),
                            repo_slug: Some(req_input.repo_slug),
                            branch: Some(req_input.branch),
                            base_branch: req_input.base_branch,
                            base_sha: req_input.base_sha,
                            head_sha: req_input.head_sha,
                            pr_title: Some(req_input.pr_title),
                            pr_body: req_input.pr_body,
                            touched_paths: req_input.touched_paths,
                            is_high_risk: req_input.is_high_risk,
                            reviewers,
                        })
                        .map_err(|err| match err {
                            PrGatePrepError::ScmUnconfigured => Error::Credential,
                            PrGatePrepError::Internal => Error::Store,
                            _ => Error::Policy,
                        })?;
                    let same_repo = match (
                        polyclave_os::canonical_real_path(FsPath::new(&run.repo_cwd)),
                        polyclave_os::canonical_real_path(FsPath::new(&caller_clone.repo)),
                    ) {
                        (Ok(a), Ok(b)) => a == b,
                        _ => run.repo_cwd == caller_clone.repo,
                    };
                    if run.hub_session_id != caller_clone.session || !same_repo {
                        return Err(Error::Policy);
                    }
                    Ok((req, deps))
                })
                .await?;
                let gate = execute_pr_gate(&deps, req).await?;
                serde_json::to_value(gate).map_err(|_| Error::Store)
            }
            _ => {
                let hub = self.clone();
                let caller = caller.clone();
                let name = name.to_owned();
                blocking(move || hub.tool(&caller, &name, args)).await
            }
        }
    }
    pub(super) fn tool(&self, caller: &ToolScope, name: &str, mut args: Value) -> HubResult<Value> {
        let session = caller.session.as_str();
        let repo = caller.repo.as_str();
        if !tool_list(caller.role == "hub")
            .iter()
            .any(|t| t["name"] == name)
        {
            return Err(Error::Policy);
        }
        let scope = RepoScope::from_registered(FsPath::new(repo))?;
        if let Some(path) = args
            .as_object_mut()
            .ok_or(Error::Policy)?
            .remove("repo_cwd")
        {
            scope.matches_requested(FsPath::new(path.as_str().ok_or(Error::Policy)?))?;
        }
        let field = |key: &str| args[key].as_str().ok_or(Error::Policy);
        match name {
            "list_repo_files" => Ok(json!(scope.list_repo_files(
                args["dir"].as_str().map(FsPath::new),
                args["glob"].as_str()
            )?)),
            "read_repo_file" => {
                let line = |key: &str| {
                    args.get(key)
                        .map(|v| {
                            v.as_u64()
                                .and_then(|n| usize::try_from(n).ok())
                                .ok_or(Error::Policy)
                        })
                        .transpose()
                };
                Ok(json!(scope.read_repo_file(
                    FsPath::new(field("rel_path")?),
                    line("start_line")?,
                    line("end_line")?
                )?))
            }
            "search_repo" => Ok(json!(
                scope.search_repo(field("pattern")?, args["glob"].as_str())?
            )),
            "get_run_diff" => {
                let (files, diff) = scope.get_run_diff(&self.runs, field("run_id")?)?;
                Ok(json!({"files":files,"diff":diff}))
            }
            "memory_search" | "memory_brief" => {
                polyclave_adapters::mcp::memory_tool_result(&self.store, name, repo, args)
                    .map_err(|_| Error::Policy)
            }
            "memory_propose" => propose_memory(
                &self.store,
                caller,
                serde_json::from_value(args).map_err(|_| Error::Policy)?,
            ),
            "create_or_update_ticket" => self.ticket_tool(
                caller,
                serde_json::from_value(args).map_err(|_| Error::Policy)?,
            ),
            "dispatch_task" => {
                if self.store.pause_state().map_err(|_| Error::Store)?.paused {
                    return Err(Error::Policy);
                }
                args["repo_cwd"] = json!(repo);
                let request: RunRequest =
                    serde_json::from_value(args).map_err(|_| Error::Policy)?;
                if request.hub_session_id != session {
                    return Err(Error::Policy);
                }
                let (run, new) = self.triage.dispatch(request)?;
                if new {
                    let launch = self
                        .runner
                        .schedule(&run.id, TurnMode::Start, run.instructions.clone())
                        .map_err(|_| Error::Store)?;
                    self.track(run.id.clone(), launch);
                }
                Ok(json!(run))
            }
            "memory_commit" | "memory_supersede" | "memory_archive" => {
                let curate =
                    MemoryCurator::for_host(self.store.clone(), repo.to_owned(), Curator::Hub);
                let id = field("id")?;
                Ok(json!(match name {
                    "memory_commit" => curate.memory_commit(id)?,
                    "memory_supersede" => curate.memory_supersede(id, field("prior")?)?,
                    _ => curate.memory_archive(id)?,
                }))
            }
            _ => Err(Error::Policy),
        }
    }

    fn ticket_tool(&self, caller: &ToolScope, req: TicketTool) -> HubResult<Value> {
        if let Some(id) = req.ticket_id {
            let caller = caller.clone();
            return self.store.write(move |db| {
                let (basis, title, mandate, budget, status):(Option<i64>,String,String,u32,String) = db.query_row(
                    "SELECT basis_msg_id,title,mandate_json,max_runs,status FROM tickets
                     WHERE id=?1 AND hub_session_id=?2 AND repo_cwd=?3",
                    params![id,caller.session,caller.repo], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?)))
                    .optional()?.ok_or(StoreError::NotFound)?;
                let saved_mandate:Value = serde_json::from_str(&mandate)?;
                if req.message_id.is_some_and(|b| Some(b)!=basis) || req.max_runs.is_some_and(|v|v!=budget)
                    || req.mandate.as_ref().is_some_and(|v|json!(v)!=saved_mandate) {
                    return Err(StoreError::ConflictError);
                }
                let next = req.status.unwrap_or_else(||status.clone());
                let title = req.title.unwrap_or(title);
                if title.trim().is_empty() || title.len()>256 || (status!=next &&
                    (status!="active" || !matches!(next.as_str(),"done"|"cancelled"|"blocked_on_user"))) {
                    return Err(StoreError::ConflictError);
                }
                db.execute("UPDATE tickets SET title=?2,status=?3,updated_at=?4 WHERE id=?1",
                    params![id,title,next,now(db)?])?;
                let value=json!({"id":id,"hub":caller.session,"repo":caller.repo,"title":title,"mandate":saved_mandate,
                    "status":next,"limit":budget,"used":db.query_row("SELECT runs_used FROM tickets WHERE id=?1",[&id],|r|r.get::<_,u32>(0))?});
                Store::append_change(db,ChangeTopic::Ticket,&id,&value)?;
                Ok(value)
            }).map_err(|_|Error::Policy);
        }
        // Only the explicit user input claimed by this turn can authorize a new budget.
        let basis: i64 = self
            .store
            .read(|db| {
                Ok(db.query_row(
                    "SELECT m.id FROM messages m JOIN turns t ON t.id=m.turn_id
             WHERE t.id=?1 AND t.status='running' AND t.phase='authorized' AND t.mcp_revoked=0
             AND m.session_id=?2 AND m.source='user'",
                    params![caller.turn, caller.session],
                    |r| r.get(0),
                )?)
            })
            .map_err(|_| Error::Policy)?;
        if req.message_id.is_some_and(|id| id != basis)
            || req.status.as_deref().is_some_and(|s| s != "active")
        {
            return Err(Error::Policy);
        }
        let ticket = self.triage.explicit(
            &caller.session,
            basis,
            &req.title.ok_or(Error::Policy)?,
            req.mandate.ok_or(Error::Policy)?,
            req.max_runs.unwrap_or(4),
        )?;
        Ok(json!(ticket))
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartCouncilTool {
    council_id: Option<String>,
    ticket_id: Option<String>,
    level: Option<CouncilLevel>,
    preference: Option<PlanningPreference>,
    prompt: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunPrGateTool {
    run_id: String,
    repo_slug: String,
    branch: String,
    base_branch: Option<String>,
    base_sha: Option<String>,
    head_sha: Option<String>,
    pr_title: String,
    pr_body: Option<String>,
    touched_paths: Option<Vec<String>>,
    is_high_risk: Option<bool>,
    reviewers: Option<Vec<super::http::ReviewerInput>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TicketTool {
    ticket_id: Option<String>,
    #[serde(alias = "basis_msg_id")]
    message_id: Option<i64>,
    title: Option<String>,
    mandate: Option<Mandate>,
    max_runs: Option<u32>,
    status: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Proposal {
    title: String,
    content: String,
    kind: MemoryKind,
    #[serde(default)]
    tags: Vec<String>,
}
fn propose_memory(store: &Store, caller: &ToolScope, proposal: Proposal) -> HubResult<Value> {
    let Proposal {
        title,
        content,
        kind,
        tags,
    } = proposal;
    if title.trim().is_empty()
        || title.len() > 256
        || content.trim().is_empty()
        || content.len() > 16_384
        || tags.len() > 24
        || tags
            .iter()
            .any(|tag| tag.trim().is_empty() || tag.len() > 64)
    {
        return Err(Error::Policy);
    }
    let turn = caller.turn;
    store.write(move |db| {
        let id=new_id('m');
        let changed=db.execute("INSERT INTO memory_items(id,scope,repo_cwd,kind,title,content,tags_json,status,created_by,source_turn_id,created_at,updated_at)
            SELECT ?2,'project',s.repo_cwd,?3,?4,?5,?6,'candidate',s.role,t.id,?7,?7 FROM turns t JOIN sessions s ON s.id=t.session_id
            WHERE t.id=?1 AND t.phase='authorized' AND t.status='running' AND t.mcp_revoked=0 AND t.mode=s.role AND s.role IN ('hub','manager')",
            params![turn,id,serde_json::to_value(kind)?.as_str(),title,content,json!(tags).to_string(),now(db)?])?;
        if changed!=1 { return Err(StoreError::ConflictError); }
        let value=json!({"id":id,"status":"candidate","source_turn_id":turn});
        Store::append_change(db,ChangeTopic::Memory,&id,&value)?;
        Ok(value)
    }).map_err(|_|Error::Policy)
}

pub(super) fn tool_list(hub: bool) -> Vec<Value> {
    let string = json!({"type":"string"});
    let strings = json!({"type":"array","items":string});
    let line = json!({"type":"integer","minimum":1});
    fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
        json!({"name":name,"description":description,"inputSchema":{"type":"object","properties":properties,"required":required,"additionalProperties":false}})
    }
    let mut tools = vec![
        tool(
            "list_repo_files",
            "List safe project files",
            json!({"dir":string,"glob":string}),
            &[],
        ),
        tool(
            "read_repo_file",
            "Read safe text by inclusive line range",
            json!({"rel_path":string,"start_line":line,"end_line":line}),
            &["rel_path"],
        ),
        tool(
            "search_repo",
            "Search project text",
            json!({"pattern":string,"glob":string}),
            &["pattern"],
        ),
        tool(
            "get_run_diff",
            "Read verified mirror evidence",
            json!({"run_id":string}),
            &["run_id"],
        ),
        tool("memory_brief", "Read verified memory", json!({}), &[]),
        tool(
            "memory_search",
            "Search verified memory",
            json!({"query":string}),
            &["query"],
        ),
        tool(
            "memory_propose",
            "Propose a candidate project memory",
            json!({"title":string,"content":string,"kind":string,"tags":strings}),
            &["kind", "title", "content"],
        ),
    ];
    if hub {
        let mandate = json!({"type":"object","properties":{"goal":string,"allowed_paths":strings,"forbidden_actions":strings,"acceptance_criteria":strings},
            "required":["goal","allowed_paths","forbidden_actions","acceptance_criteria"],"additionalProperties":false});
        tools.extend([
            tool("create_or_update_ticket","Create a ticket from this turn's explicit user request with title and mandate. Supply ticket_id to update its title or close it within its original budget.",
                json!({"ticket_id":string,"message_id":line,"title":string,"mandate":mandate,"max_runs":{"type":"integer","minimum":1,"maximum":20},
                    "status":{"enum":["active","blocked_on_user","done","cancelled"]}}),&[]),
            tool("dispatch_task","Dispatch only under an approved Ticket",json!({"ticket_id":string,"hub_session_id":string,"request_id":string,"vendor":string,
                "model":string,"repo_cwd":string,"base_branch":string,"instructions":string}),&["ticket_id","hub_session_id","request_id","vendor","repo_cwd","base_branch","instructions"]),
            tool("memory_commit","Promote an eligible candidate",json!({"id":string}),&["id"]),
            tool("memory_supersede","Version a verified project rule",json!({"id":string,"prior":string}),&["id","prior"]),
            tool("memory_archive","Archive an eligible rule",json!({"id":string}),&["id"]),
            tool(
                "start_council",
                "Run a blind multi-model architecture council. Direction-level councils always wait for explicit user choice and never auto-start dev.",
                json!({
                    "council_id": string,
                    "ticket_id": string,
                    "level": {"enum": ["direction", "detail"]},
                    "preference": {"enum": ["thorough", "balanced", "conservative"]},
                    "prompt": string,
                    "repo_cwd": string
                }),
                &["prompt"],
            ),
            tool(
                "run_pr_gate",
                "Run preflight, 3-layer touched_paths check, PR creation, and heterogeneous disposable worktree review for a completed run.",
                json!({
                    "run_id": string,
                    "repo_slug": string,
                    "branch": string,
                    "base_branch": string,
                    "base_sha": string,
                    "head_sha": string,
                    "pr_title": string,
                    "pr_body": string,
                    "touched_paths": strings,
                    "is_high_risk": {"type": "boolean"},
                    "reviewers": {"type": "array", "items": {"type": "object", "properties": {"vendor": string, "model": string}, "required": ["vendor", "model"], "additionalProperties": false}},
                    "repo_cwd": string
                }),
                &["run_id", "repo_slug", "branch", "pr_title"],
            ),
        ]);
    }
    tools
}
