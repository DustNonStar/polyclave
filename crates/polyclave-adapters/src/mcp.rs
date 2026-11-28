use axum::{
    body::{to_bytes, Body},
    extract::{Request, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use polyclave_proto::MemoryKind;
use polyclave_store::{now, CandidateMemory, Error, MemoryFilter, Store};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttemptScope {
    pub id: i64,
    pub number: u32,
    pub run: String,
    pub repo: String,
    pub branch: String,
    pub mirror: String,
    pub base: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ActiveTurn {
    id: i64,
    mode: String,
    can_stage: bool,
    attempt: Option<AttemptScope>,
}

/// A trusted host implementation checks its own Git mirror; the Worker cannot choose it.
pub trait HostTools: Send + Sync + 'static {
    fn verified_tip(&self, attempt: &AttemptScope, sha: &str) -> Result<bool>;
    fn consult_read_only(&self, attempt: &AttemptScope, question: &str) -> Result<String>;
    fn commit_and_stage(
        &self,
        _attempt: &AttemptScope,
        _turn_id: i64,
        _message: &str,
        _paths: Option<&[String]>,
    ) -> Result<String> {
        Err(Error::Invalid("Host staging is unavailable"))
    }
}

#[derive(Clone)]
pub struct Mcp {
    store: Store,
    host_id: Arc<str>,
    tools: Arc<dyn HostTools>,
}

impl Mcp {
    pub fn new(store: Store, host_id: impl Into<Arc<str>>, tools: Arc<dyn HostTools>) -> Self {
        Self {
            store,
            host_id: host_id.into(),
            tools,
        }
    }

    pub fn router(self) -> Router {
        Router::new().route("/mcp", post(handle)).with_state(self)
    }

    pub async fn listen(
        self,
    ) -> std::io::Result<(
        std::net::SocketAddr,
        tokio::task::JoinHandle<std::io::Result<()>>,
    )> {
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let addr = listener.local_addr()?;
        Ok((
            addr,
            tokio::spawn(async move { axum::serve(listener, self.router()).await }),
        ))
    }

    fn auth(&self, token: &str) -> Result<Option<ActiveTurn>> {
        self.store.read(|db| active(db, token, &self.host_id))
    }

    fn dispatch(&self, request: Value, token: &str) -> Response {
        let turn = match self.auth(token) {
            Ok(Some(turn)) => turn,
            Ok(None) => return StatusCode::UNAUTHORIZED.into_response(),
            Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        };
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        if !(id.is_null()
            || id.is_i64()
            || id.is_u64()
            || id
                .as_str()
                .is_some_and(|v| v.len() <= 128 && !v.contains(token)))
        {
            return rpc_error(Value::Null, -32600, "Invalid request id");
        }
        let method = request["method"].as_str();
        if !request.is_object() || request["jsonrpc"] != "2.0" || method.is_none() {
            return rpc_error(id, -32600, "Invalid request");
        }
        if request.get("id").is_none() {
            return StatusCode::ACCEPTED.into_response();
        }
        let response = match method.unwrap_or_default() {
            "initialize" => {
                let protocol = request["params"]["protocolVersion"]
                    .as_str()
                    .filter(|p| {
                        matches!(
                            *p,
                            "2024-11-05" | "2025-03-26" | "2025-06-18" | "2025-11-25"
                        )
                    })
                    .unwrap_or("2025-06-18");
                json!({"protocolVersion":protocol,"capabilities":{"tools":{}},"serverInfo":{"name":"polyclave-host","version":"0.1.0"}})
            }
            "ping" => json!({}),
            "tools/list" => json!({"tools":definitions(turn.mode == "consult", turn.can_stage)}),
            "tools/call" => {
                let name = request["params"]["name"].as_str().unwrap_or_default();
                if !definitions(turn.mode == "consult", turn.can_stage)
                    .iter()
                    .any(|t| t["name"] == name)
                {
                    return rpc_error(id, -32601, "Unknown or unavailable tool");
                }
                let args = request["params"]
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                let reply = self.call(name, args, token, turn);
                return match reply {
                    Ok(v) => Json(json!({"jsonrpc":"2.0","id":id,"result":v})).into_response(),
                    Err(CallError::Expired) => StatusCode::UNAUTHORIZED.into_response(),
                    Err(CallError::Invalid) => rpc_error(id,-32602,"Invalid arguments"),
                    Err(CallError::Conflict) => Json(json!({"jsonrpc":"2.0","id":id,"result":tool_text("Rejected: request conflicts with current state",true)})).into_response(),
                    Err(CallError::Storage) => rpc_error(id,-32603,"Host could not complete the request"),
                };
            }
            _ => return rpc_error(id, -32601, "Method not found"),
        };
        Json(json!({"jsonrpc":"2.0","id":id,"result":response})).into_response()
    }

    fn call(
        &self,
        name: &str,
        args: Value,
        token: &str,
        turn: ActiveTurn,
    ) -> std::result::Result<Value, CallError> {
        let Some(attempt) = turn.attempt.as_ref() else {
            return Ok(tool_text("Rejected: this session has no active task", true));
        };
        match name {
            "commit_and_stage" => {
                let args: StageCommit = decode(args)?;
                let message = safe_text(args.message, token, 4096)?;
                if args.paths.as_ref().is_some_and(|paths| {
                    paths.is_empty()
                        || paths.len() > 100
                        || paths.iter().any(|p| p.len() > 1024 || p.contains(token))
                }) {
                    return Err(CallError::Invalid);
                }
                let staged =
                    self.tools
                        .commit_and_stage(attempt, turn.id, &message, args.paths.as_deref());
                if self.auth(token)? != Some(turn) {
                    return Err(CallError::Expired);
                }
                match staged {
                    Ok(sha) => {
                        let mut reply = tool_text(
                            &format!("Staged {sha}. Use submit_result with this exact SHA."),
                            false,
                        );
                        reply["structuredContent"] = json!({"sha": sha});
                        Ok(reply)
                    }
                    Err(Error::Invalid(detail)) => Ok(tool_text(detail, true)),
                    Err(error) => Err(error.into()),
                }
            }
            "submit_result" => {
                let Ok(data) = decode::<Submit>(args) else {
                    return Ok(tool_text("Rejected: invalid submission", true));
                };
                if data.run_id != attempt.run {
                    return Ok(tool_text(
                        "Rejected: run is not the active task of this session",
                        true,
                    ));
                }
                if data.branch != attempt.branch {
                    return Ok(tool_text(
                        &format!("Rejected: branch must be {}", attempt.branch),
                        true,
                    ));
                }
                if data.sha.len() != 40
                    || !data
                        .sha
                        .bytes()
                        .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
                {
                    return Ok(tool_text("Rejected: invalid commit id", true));
                }
                let claim = match claim_notes(&data, token) {
                    Ok(claim) => claim,
                    Err(()) => return Ok(tool_text("Rejected: invalid test summary", true)),
                };
                if let Some(accepted) = self.store.read(|db| receipt(db, attempt.id))? {
                    if self.auth(token)? != Some(turn) {
                        return Err(CallError::Expired);
                    }
                    return Ok(if accepted.same_receipt(&data) {
                        tool_text("Already accepted.", false)
                    } else {
                        different_receipt()
                    });
                }
                let valid = self.tools.verified_tip(attempt, &data.sha);
                if self.auth(token)? != Some(turn.clone()) {
                    return Err(CallError::Expired);
                }
                if !valid? {
                    return Ok(tool_text(
                        "Rejected: commit does not match the verified mirror tip",
                        true,
                    ));
                }
                // The claim is recorded for the submitted SHA only, labelled as
                // the Worker's own statement.
                let claim_row = claim.tests.map(|summary| {
                    (
                        data.run_id.clone(),
                        data.sha.clone(),
                        claim.status.unwrap_or(polyclave_proto::TestStatus::Pending),
                        summary,
                    )
                });
                let (id, token, host) = (attempt.id, token.to_owned(), self.host_id.clone());
                let outcome=self.store.write(move |db| {
                    if active(db,&token,&host)? != Some(turn.clone()) {return Ok(None);}
                    if let Some(previous)=receipt(db,id)? {return Ok(Some(if previous.same_receipt(&data) {
                        // A resend that only rewords its notes does not replace the
                        // first receipt, so the same claim is kept once.
                        return Ok(Some(1));
                    } else {2}));}
                    db.execute("INSERT INTO results(run_id,attempt_id,turn_id,accepted,payload_json,created_at) VALUES(?1,?2,?3,1,?4,?5)",
                        params![data.run_id,id,turn.id,serde_json::to_string(&data)?,now(db)?])?;
                    if let Some((run, sha, status, summary)) = claim_row {
                        polyclave_store::record_test_result(
                            db,
                            &polyclave_store::NewTestResult {
                                check_id: String::new(),
                                run_id: run,
                                sha,
                                source: polyclave_proto::TestResultSource::WorkerClaim,
                                name: String::new(),
                                status,
                                summary,
                            },
                        )?;
                    }
                    Ok(Some(0))
                })?;
                match outcome {
                    Some(0) => Ok(tool_text("Accepted and stored by the host.", false)),
                    Some(1) => Ok(tool_text("Already accepted.", false)),
                    Some(_) => Ok(different_receipt()),
                    None => Err(CallError::Expired),
                }
            }
            "report_progress" => {
                let Progress { text } = decode(args)?;
                let text = safe_text(text, token, 8192)?;
                let (token, host) = (token.to_owned(), self.host_id.clone());
                let accepted=self.store.write(move |db| {
                    if active(db,&token,&host)? != Some(turn.clone()) {return Ok(false);}
                    db.execute("INSERT INTO events(turn_id,event_kind,payload_json,created_at) VALUES(?1,'progress',?2,?3)",
                        params![turn.id,json!({"text":text}).to_string(),now(db)?])?;
                    Ok(true)
                })?;
                if !accepted {
                    return Err(CallError::Expired);
                }
                Ok(tool_text("Progress recorded.", false))
            }
            "consult" => {
                let Consult { question } = decode(args)?;
                let question = safe_text(question, token, 8192)?;
                let response = self.tools.consult_read_only(attempt, &question)?;
                if self.auth(token)? != Some(turn) {
                    return Err(CallError::Expired);
                }
                Ok(tool_text(&safe_text(response, token, 16_384)?, false))
            }
            "memory_brief" | "memory_search" => {
                let items = query_memory(&self.store, name, &attempt.repo, args)?;
                if self.auth(token)? != Some(turn) {
                    return Err(CallError::Expired);
                }
                Ok(tool_text(
                    &safe_text(
                        serde_json::to_string(&items).map_err(Error::from)?,
                        token,
                        96_000,
                    )?,
                    false,
                ))
            }
            "memory_propose" => {
                let Proposal {
                    kind,
                    title,
                    content,
                    tags,
                } = decode(args)?;
                let title = safe_text(title, token, 256)?;
                let content = safe_text(content, token, 16_384)?;
                if tags.len() > 24 {
                    return Err(CallError::Invalid);
                }
                let tags = tags
                    .into_iter()
                    .map(|t| safe_text(t, token, 64))
                    .collect::<std::result::Result<_, _>>()?;
                // Store rechecks that the turn is still running and current inside the write.
                let result = self.store.propose_memory(
                    turn.id,
                    CandidateMemory {
                        kind,
                        title,
                        content,
                        tags,
                    },
                );
                match result {
                    Ok(candidate) => Ok(tool_text(&candidate.id, false)),
                    Err(Error::ConflictError) => Err(CallError::Expired),
                    Err(e) => Err(e.into()),
                }
            }
            _ => Err(CallError::Invalid),
        }
    }
}

fn query_memory(
    store: &Store,
    name: &str,
    repo: &str,
    args: Value,
) -> std::result::Result<Vec<polyclave_proto::MemoryItemDto>, CallError> {
    let Search { query } = decode(args)?;
    if name == "memory_brief" && query.is_some() {
        return Err(CallError::Invalid);
    }
    if name == "memory_search" && query.as_deref().is_none_or(|q| q.trim().is_empty()) {
        return Err(CallError::Invalid);
    }
    let memory = store.find_memory(MemoryFilter {
        repo: Some(repo),
        query: query.as_deref(),
        limit: Some(if name == "memory_brief" { 12 } else { 20 }),
        ..Default::default()
    })?;
    let budget = if name == "memory_brief" { 6000 } else { 12_000 };
    let mut kept = Vec::new();
    let mut used = 2usize; // "[]"
    for mut item in memory {
        let sep = usize::from(!kept.is_empty());
        let item_json = serde_json::to_string(&item).map_err(Error::from)?;
        if used + sep + item_json.len() <= budget {
            used += sep + item_json.len();
            kept.push(item);
            continue;
        }
        let mut lo = 0usize;
        let mut hi = item.content.len();
        let mut best = None;
        while lo <= hi {
            let mid = item.content.floor_char_boundary(lo + (hi - lo) / 2);
            let mut candidate = item.clone();
            candidate.content.truncate(mid);
            let candidate_json = serde_json::to_string(&candidate).map_err(Error::from)?;
            if used + sep + candidate_json.len() <= budget {
                best = Some((mid, candidate_json.len()));
                if mid == hi {
                    break;
                }
                lo = item.content.ceil_char_boundary(mid + 1);
            } else if mid == 0 {
                break;
            } else {
                hi = mid - 1;
            }
        }
        if let Some((len, _)) = best.filter(|(len, _)| *len > 0) {
            item.content.truncate(len);
            kept.push(item);
        }
        break;
    }
    Ok(kept)
}

pub fn memory_tool_result(
    store: &Store,
    name: &str,
    repo: &str,
    args: Value,
) -> std::result::Result<Value, Error> {
    let items = query_memory(store, name, repo, args).map_err(|e| match e {
        CallError::Invalid => Error::Invalid("invalid memory tool arguments"),
        CallError::Expired | CallError::Conflict => Error::ConflictError,
        CallError::Storage => Error::Closed,
    })?;
    serde_json::to_value(items).map_err(Error::from)
}

fn active(db: &Connection, token: &str, host: &str) -> Result<Option<ActiveTurn>> {
    Ok(db.query_row(
        "SELECT t.id,t.mode,a.id,r.id,r.repo_cwd,a.branch,r.mirror_remote,r.base_sha,a.attempt_no,
                COALESCE(r.vendor IN ('codex','claude') AND t.mode IN ('start','retry','correction','worker_chat'),0)
         FROM turns t LEFT JOIN runs r ON r.id=t.run_id LEFT JOIN attempts a ON a.id=t.attempt_id AND a.run_id=r.id
         WHERE t.mcp_token=?1 AND t.host_instance_id=?2 AND t.status='running' AND t.phase='authorized' AND t.mcp_revoked=0
           AND (t.run_id IS NULL OR (a.attempt_no=r.current_attempt AND a.status NOT IN ('superseded','cancelled')
                                    AND r.status NOT IN ('done','failed')))",
        [token,host], |r| {
            let attempt=match r.get::<_,Option<i64>>(2)? {
                Some(id)=>Some(AttemptScope{id,run:r.get(3)?,repo:r.get(4)?,branch:r.get(5)?,mirror:r.get(6)?,base:r.get(7)?,number:r.get(8)?}),
                None=>None,
            };
            Ok(ActiveTurn{id:r.get(0)?,mode:r.get(1)?,attempt,can_stage:r.get(9)?})
        }
    ).optional()?)
}

fn receipt(db: &Connection, id: i64) -> Result<Option<Submit>> {
    let s = db
        .query_row(
            "SELECT payload_json FROM results WHERE attempt_id=?1 AND accepted=1",
            [id],
            |r| r.get::<_, String>(0),
        )
        .optional()?;
    Ok(s.map(|s| serde_json::from_str(&s)).transpose()?)
}

#[derive(Debug)]
enum CallError {
    Expired,
    Invalid,
    Conflict,
    Storage,
}
impl From<Error> for CallError {
    fn from(e: Error) -> Self {
        match e {
            Error::ConflictError | Error::BusyError => Self::Conflict,
            Error::Invalid(_) => Self::Invalid,
            _ => Self::Storage,
        }
    }
}
fn decode<T: serde::de::DeserializeOwned>(v: Value) -> std::result::Result<T, CallError> {
    serde_json::from_value(v).map_err(|_| CallError::Invalid)
}
fn safe_text(text: String, token: &str, max: usize) -> std::result::Result<String, CallError> {
    if text.len() > max || text.trim().is_empty() || text.contains(token) {
        Err(CallError::Invalid)
    } else {
        Ok(text)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StageCommit {
    message: String,
    paths: Option<Vec<String>>,
}

#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct Submit {
    run_id: String,
    branch: String,
    sha: String,
    /// The Worker's own test summary. Never a host check; stored labelled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tests: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tests_status: Option<String>,
    /// What the Worker did not finish or could not verify.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    not_done: Option<String>,
}

impl Submit {
    /// The identity of a submission. `tests`, `tests_status` and `not_done` are
    /// the Worker's notes: a resend that words them differently is still the same
    /// receipt, so the first accepted receipt stays authoritative.
    fn same_receipt(&self, other: &Self) -> bool {
        self.run_id == other.run_id && self.branch == other.branch && self.sha == other.sha
    }
}
/// The Worker's optional notes, validated. `None` means "not sent".
#[derive(Default)]
struct ClaimNotes {
    tests: Option<String>,
    status: Option<polyclave_proto::TestStatus>,
}

/// Rejects a note that is too long or carries the turn token, and a claimed
/// status that is not one the approval card knows. A blank note counts as not
/// sent.
fn claim_notes(data: &Submit, token: &str) -> std::result::Result<ClaimNotes, ()> {
    let note = |v: Option<&str>, max: usize| -> std::result::Result<Option<String>, ()> {
        match v.map(str::trim) {
            None | Some("") => Ok(None),
            Some(v) => safe_text(v.to_owned(), token, max)
                .map(Some)
                .map_err(|_| ()),
        }
    };
    let tests = note(data.tests.as_deref(), 2000)?;
    // `not_done` needs no storage of its own: the whole receipt is kept.
    let _ = note(data.not_done.as_deref(), 4000)?;
    let status = match data.tests_status.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(raw) => Some(polyclave_proto::TestStatus::parse(raw).ok_or(())?),
    };
    if status.is_some() && tests.is_none() {
        return Err(());
    }
    Ok(ClaimNotes { tests, status })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Progress {
    text: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Consult {
    question: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Search {
    query: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Proposal {
    kind: MemoryKind,
    title: String,
    content: String,
    #[serde(default)]
    tags: Vec<String>,
}

fn definitions(read_only: bool, can_stage: bool) -> Vec<Value> {
    let str_type = json!({"type":"string"});
    let mut result = vec![];
    let mut tool = |name, description, p: Value, required: Value| {
        result.push(json!({"name":name,"description":description,"inputSchema":{"type":"object","properties":p,"required":required,"additionalProperties":false}}));
    };
    tool(
        "memory_brief",
        "Read verified project memory",
        json!({}),
        json!([]),
    );
    tool(
        "memory_search",
        "Search verified project memory",
        json!({"query":str_type}),
        json!(["query"]),
    );
    if !read_only {
        if can_stage {
            tool(
                "commit_and_stage",
                "Commit changes within this Attempt's Ticket scope and publish to its staging branch. Returns the SHA for submit_result.",
                json!({"message":str_type,"paths":{"type":"array","items":str_type}}),
                json!(["message"]),
            );
        }
        tool(
            "submit_result",
            "Submit a pushed commit for host verification",
            json!({"run_id":str_type,"branch":str_type,"sha":str_type,
                "tests":str_type,"tests_status":str_type,"not_done":str_type}),
            json!(["run_id", "branch", "sha"]),
        );
        tool(
            "report_progress",
            "Report progress without marking a Run complete",
            json!({"text":str_type}),
            json!(["text"]),
        );
        tool(
            "consult",
            "Ask a depth-one read-only advisor",
            json!({"question":str_type}),
            json!(["question"]),
        );
        tool(
            "memory_propose",
            "Propose candidate memory for later review",
            json!({"kind":{"type":"string","enum":["decision","constraint","progress","codebase_fact","pitfall","preference","receipt"]},"title":str_type,"content":str_type,"tags":{"type":"array","items":str_type}}),
            json!(["kind", "title", "content"]),
        );
    }
    result
}
fn different_receipt() -> Value {
    tool_text(
        "Rejected: a different result was already accepted for this task",
        true,
    )
}

fn tool_text(text: &str, is_error: bool) -> Value {
    json!({"content":[{"type":"text","text":text}],"isError":is_error})
}
fn rpc_error(id: Value, code: i32, message: &str) -> Response {
    Json(json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})).into_response()
}

async fn handle(State(mcp): State<Mcp>, req: Request<Body>) -> Response {
    if req.headers().contains_key(header::ORIGIN) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let token = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .filter(|s| !s.is_empty() && s.len() <= 256);
    let Some(token) = token.map(str::to_owned) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let bytes = match to_bytes(req.into_body(), 65536).await {
        Ok(b) => b,
        Err(_) => return StatusCode::PAYLOAD_TOO_LARGE.into_response(),
    };
    let request = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => {
            return match tokio::task::spawn_blocking(move || mcp.auth(&token)).await {
                Ok(Ok(Some(_))) => rpc_error(Value::Null, -32700, "Parse error"),
                Ok(Ok(None)) => StatusCode::UNAUTHORIZED.into_response(),
                _ => StatusCode::SERVICE_UNAVAILABLE.into_response(),
            }
        }
    };
    match tokio::task::spawn_blocking(move || mcp.dispatch(request, &token)).await {
        Ok(response) => response,
        Err(_) => StatusCode::SERVICE_UNAVAILABLE.into_response(),
    }
}
