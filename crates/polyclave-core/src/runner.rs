use crate::runs::{change, load_attempt, load_run, Result, RunManager, RunWork};
use polyclave_adapters::{
    agy_worker, claude_worker, codex_worker, dsh_worker, parse_agy, parse_claude, parse_codex,
    parse_dsh, tmp_config_path, ClaudeIsolation, WorkerCommand, WorkerContext,
};
use polyclave_os::{
    boot_epoch, canonical_real_path, create_private_dir, hard_stop_excludes_writers,
    spawn_supervised, SpawnSpec,
};
use polyclave_proto::{
    AgentEvent, ChangeTopic, EndReason, RunDto, RunStatus, TurnDto, TurnMode, TurnStatus, Vendor,
    WriterCheck,
};
use polyclave_store::{decode_enum, enum_value, now, Error, Store};
use rusqlite::{params, Connection, OptionalExtension};
use std::{
    collections::HashMap,
    io::{BufRead, Read},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use uuid::Uuid;

type Active = HashMap<String, Arc<AtomicBool>>;
type EventSink = dyn Fn(&str, AgentEvent) + Send + Sync;
#[derive(Clone)]
pub struct TurnRunner {
    pub runs: RunManager,
    pub(crate) state: PathBuf,
    pub(crate) host: String,
    pub(crate) boot: String,
    supervisor: PathBuf,
    mcp_url: String,
    codex: PathBuf,
    claude: PathBuf,
    agy: PathBuf,
    dsh: PathBuf,
    confinement: Option<Arc<dyn ClaudeIsolation>>,
    active: Arc<Mutex<Active>>,
    events: Arc<EventSink>,
}

pub struct RunnerConfig {
    pub supervisor: PathBuf,
    pub mcp_url: String,
    pub codex: PathBuf,
    pub claude: PathBuf,
    pub claude_isolation: Option<Arc<dyn ClaudeIsolation>>,
}

impl TurnRunner {
    pub fn new(
        runs: RunManager,
        state: &Path,
        host: String,
        config: RunnerConfig,
        events: impl Fn(&str, AgentEvent) + Send + Sync + 'static,
    ) -> Result<Self> {
        let state =
            canonical_real_path(state).map_err(|_| Error::Invalid("invalid state directory"))?;
        match create_private_dir(&state.join("tmp")) {
            Ok(()) => (),
            Err(e)
                if e.kind() == std::io::ErrorKind::AlreadyExists
                    && state
                        .join("tmp")
                        .symlink_metadata()
                        .is_ok_and(|m| m.file_type().is_dir()) => {}
            _ => {
                return Err(Error::Invalid(
                    "cannot establish private temporary directory",
                ))
            }
        }
        Ok(Self {
            runs,
            state,
            host,
            boot: boot_epoch().map_err(|_| Error::Invalid("boot identity unavailable"))?,
            supervisor: config.supervisor,
            mcp_url: config.mcp_url,
            codex: config.codex,
            claude: config.claude,
            agy: PathBuf::from("agy"),
            dsh: PathBuf::from("dsh"),
            confinement: config.claude_isolation,
            active: Arc::new(Mutex::new(HashMap::new())),
            events: Arc::new(events),
        })
    }

    pub fn with_agy(mut self, path: PathBuf) -> Self {
        self.agy = path;
        self
    }

    pub fn with_dsh(mut self, path: PathBuf) -> Self {
        self.dsh = path;
        self
    }

    pub fn cancel(&self, id: &str) -> Result<()> {
        let jobs = self.active.lock().map_err(|_| Error::Closed)?;
        jobs.get(id)
            .ok_or(Error::ConflictError)?
            .store(true, Ordering::Release);
        Ok(())
    }

    /// Claims the local slot before returning. The database also enforces the
    /// running-turn lock; this registration covers the scheduling gap.
    pub fn schedule(
        &self,
        id: &str,
        mode: TurnMode,
        text: String,
    ) -> Result<std::thread::JoinHandle<Result<RunDto>>> {
        let mut jobs = self.active.lock().map_err(|_| Error::Closed)?;
        if jobs.contains_key(id) {
            return Err(Error::BusyError);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let work = self.runs.begin(id, mode)?;
        jobs.insert(id.into(), cancel.clone());
        let runner = self.clone();
        let owned = id.to_owned();
        match std::thread::Builder::new()
            .name("polyclave-turn".into())
            .spawn(move || {
                let _slot = Slot {
                    active: runner.active.clone(),
                    id: owned.clone(),
                };
                runner.execute(work, mode, text, &cancel)
            }) {
            Ok(thread) => Ok(thread),
            Err(_) => {
                jobs.remove(id);
                let rid = id.to_owned();
                let _ = self.runs.store.write(move |db| {
                    let changed = db.execute(
                        "UPDATE runs SET status='interrupted',detail='failed to spawn worker thread',updated_at=?2
                         WHERE id=?1 AND status='running'
                         AND NOT EXISTS (SELECT 1 FROM turns WHERE run_id=?1 AND status='running')",
                        params![rid, now(db)?],
                    )?;
                    if changed > 0 {
                        change(db, ChangeTopic::Run, &rid, &load_run(db, &rid)?)?;
                    }
                    Ok(())
                });
                Err(Error::Closed)
            }
        }
    }

    pub fn marker(&self, id: i64) -> PathBuf {
        self.state.join("tmp").join(format!("turn-{id}.released"))
    }

    fn execute(
        &self,
        mut work: RunWork,
        mut mode: TurnMode,
        mut prompt: String,
        cancel: &AtomicBool,
    ) -> Result<RunDto> {
        loop {
            let token = Uuid::new_v4().simple().to_string();
            let refspec = work.id.full_ref();
            let input=format!(
                "Run: {}\nAttempt branch: {}\nOnly git push polyclave-staging {refspec}:{refspec}. Commit with Run-Id: {} in every commit, then use the host submit_result tool.\n\n{prompt}",
                work.run.id,work.attempt.branch,work.run.id);
            let turn = self.prepare(&work, mode, &token)?;
            let step = (|| -> Result<Option<RunDto>> {
                let outcome = self.run_process(&work, turn, &token, input, cancel);
                if let Some(detail) = outcome.detail {
                    let rid = work.run.id.clone();
                    self.runs.store.write(move |db| {
                        change(
                            db,
                            ChangeTopic::Inbox,
                            &rid,
                            &serde_json::json!({"warning":detail}),
                        )
                    })?;
                }
                let released = self.release_present(turn)?;
                let end = if !released {
                    EndReason::NeverStarted
                } else if outcome.cancelled {
                    EndReason::Cancel
                } else if outcome.code.is_some_and(|code| code >= 0) {
                    EndReason::Exit
                } else {
                    EndReason::HostLost
                };
                let writer = if released && end != EndReason::Exit && !hard_stop_excludes_writers()
                {
                    WriterCheck::Possible
                } else {
                    WriterCheck::Excluded
                };
                if end == EndReason::Exit
                    && matches!(
                        mode,
                        TurnMode::Start | TurnMode::Retry | TurnMode::Correction
                    )
                {
                    self.revoke(turn)?;
                    let verdict = self.runs.verdict(&work, writer)?;
                    let retryable =
                        verdict.detail.as_deref() == Some(crate::classify::RETRYABLE_DETAIL);
                    if retryable && work.attempt.corrections < 2 && !cancel.load(Ordering::Acquire)
                    {
                        let id = work.attempt.id;
                        let exit = outcome.code;
                        self.runs.store.write(move |db|{
                            let stamp=now(db)?;
                            db.execute("UPDATE turns SET status=?2,end_reason='exit',writer_check='excluded',exit_code=?3,ended_at=?4 WHERE id=?1",
                                params![turn,if exit==Some(0){"completed"}else{"failed"},exit,stamp])?;
                            db.execute("UPDATE attempts SET corrections=corrections+1 WHERE id=?1",[id])?;
                            change(db,ChangeTopic::Turn,&turn.to_string(),&load_turn(db,turn)?)?;
                            change(db,ChangeTopic::Attempt,&id.to_string(),&load_attempt(db,id)?)
                        })?;
                        self.remove_turn_files(turn)?;
                        return Ok(None);
                    }
                }
                self.finish(turn, Some(&work), end, writer, outcome.code)
                    .map(Some)
            })();
            match step {
                Ok(Some(run)) => return Ok(run),
                Ok(None) => {
                    work = self.runs.old_work(&work.run.id, work.attempt.id)?;
                    mode = TurnMode::Correction;
                    prompt="The host did not see a result. Finish the requested changes, push to your staging branch, and submit the exact commit.".into();
                    continue;
                }
                Err(err) => {
                    let _ = self.revoke(turn);
                    let rid = work.run.id.clone();
                    let aid = work.attempt.id;
                    let _ = self.runs.store.write(move |db| {
                        let stamp = now(db)?;
                        db.execute(
                            "UPDATE turns SET status='interrupted',end_reason='host_lost',writer_check='excluded',mcp_revoked=1,ended_at=?2
                             WHERE id=?1 AND status='running'",
                            params![turn, stamp],
                        )?;
                        change(db, ChangeTopic::Turn, &turn.to_string(), &load_turn(db, turn)?)?;
                        db.execute(
                            "UPDATE attempts SET status='interrupted',ended_at=COALESCE(ended_at,?2) WHERE id=?1 AND status='running'",
                            params![aid, stamp],
                        )?;
                        db.execute(
                            "UPDATE runs SET status='interrupted',detail='host error during turn finalization',updated_at=?2
                             WHERE id=?1 AND status='running'",
                            params![rid, stamp],
                        )?;
                        change(db, ChangeTopic::Run, &rid, &load_run(db, &rid)?)
                    });
                    return Err(err);
                }
            }
        }
    }

    fn prepare(&self, work: &RunWork, mode: TurnMode, token: &str) -> Result<i64> {
        let (instance, boot, token) = (self.host.clone(), self.boot.clone(), token.to_owned());
        let id = work.run.id.clone();
        let session = work.run.worker_session_id.clone();
        let aid = work.attempt.id;
        let number = work.attempt.attempt_no;
        self.runs.store.write(move|db|{
            let r=load_run(db,&id)?;
            if matches!(r.status,RunStatus::Done|RunStatus::Failed)|| r.current_attempt!=number{return Err(Error::ConflictError);}
            db.execute("INSERT INTO turns(session_id,run_id,attempt_id,mode,status,phase,host_instance_id,boot_epoch,mcp_token,started_at)
                VALUES(?1,?2,?3,?4,'running','prepared',?5,?6,?7,?8)",
                params![session,id,aid,enum_value(mode)?,instance,boot,token,now(db)?])?;
            let turn=db.last_insert_rowid();
            db.execute("UPDATE attempts SET status='running',ended_at=NULL WHERE id=?1",[aid])?;
            change(db,ChangeTopic::Attempt,&aid.to_string(),&load_attempt(db,aid)?)?;
            change(db,ChangeTopic::Turn,&turn.to_string(),&load_turn(db,turn)?)?;
            Ok(turn)
        })
    }

    fn run_process(
        &self,
        work: &RunWork,
        turn: i64,
        token: &str,
        prompt: String,
        cancel: &AtomicBool,
    ) -> Exit {
        if cancel.load(Ordering::Acquire) {
            return Exit::cancelled();
        }
        let command=match self.command(work,turn,token){
            Ok(c)=>c,
            Err(polyclave_adapters::Error::ClaudeIsolationUnavailable)=>return Exit::error("Claude worker not started: no verified external filesystem and network sandbox is available"),
            Err(polyclave_adapters::Error::DshModelUnsupported)=>return Exit::error("DeepSeek Harness worker not started: `dsh headless` has no way to choose a model; leave the model unset for dsh"),
            Err(_)=>return Exit::error("worker configuration unavailable"),
        };
        let spec = SpawnSpec {
            supervisor: self.supervisor.clone(),
            worker: command.executable,
            args: command.args,
            cwd: command.cwd,
            env: command.env,
            released: self.marker(turn),
        };
        let mut wrapper = match spawn_supervised(&spec) {
            Ok(s) => s,
            Err(_) => return Exit::error("worker supervisor did not become ready"),
        };
        let identity = wrapper.identity().clone();
        let authorized = self.runs.store.write(move |db| {
            let changed = db.execute(
                "UPDATE turns SET phase='authorized',os_proc_id=?2,proc_started_at=?3
                WHERE id=?1 AND phase='prepared' AND status='running'",
                params![turn, identity.pid.to_string(), identity.started_at],
            )?;
            if changed != 1 {
                return Err(Error::ConflictError);
            }
            change(
                db,
                ChangeTopic::Turn,
                &turn.to_string(),
                &load_turn(db, turn)?,
            )
        });
        if authorized.is_err() || cancel.load(Ordering::Acquire) {
            let _ = wrapper.cancel();
            let _ = wrapper.wait();
            return if authorized.is_err() {
                Exit::error("could not authorize supervisor")
            } else {
                Exit::cancelled()
            };
        }
        if wrapper.go(prompt.as_bytes()).is_err() {
            let _ = wrapper.cancel();
            let _ = wrapper.wait();
            return Exit::error("could not release worker");
        }
        let stdout = wrapper.take_stdout();
        let stderr = wrapper.take_stderr();
        let (events, receiver) = std::sync::mpsc::sync_channel(256);
        let vendor = work.run.vendor;
        let secret = token.to_owned();
        let reader_thread = if let Some(mut stdout) = stdout {
            Some(std::thread::spawn(move || {
                let mut text = String::new();
                loop {
                    text.clear();
                    let read = (&mut stdout).take(1024 * 1024 + 1).read_line(&mut text);
                    match read {
                        Ok(0) | Err(_) => break,
                        Ok(n) if n > 1024 * 1024 && !text.ends_with('\n') => {
                            let mut byte = [0_u8; 1];
                            while matches!(stdout.read(&mut byte), Ok(1)) && byte[0] != b'\n' {}
                            continue;
                        }
                        Ok(_) => {}
                    }
                    if text.contains(&secret) {
                        continue;
                    }
                    let ev = match vendor {
                        Vendor::Codex => parse_codex(&text),
                        Vendor::Claude => parse_claude(&text),
                        Vendor::Agy => parse_agy(&text),
                        Vendor::Dsh => parse_dsh(&text),
                    };
                    if let Some(ev) = ev {
                        if events.send(ev).is_err() {
                            break;
                        }
                    }
                }
            }))
        } else {
            drop(events);
            None
        };
        if let Some(mut stderr) = stderr {
            // CLI diagnostics can contain credentials; do not log or persist them.
            std::thread::spawn(move || {
                let _ = std::io::copy(&mut stderr, &mut std::io::sink());
            });
        }
        loop {
            if cancel.load(Ordering::Acquire) {
                let _ = wrapper.cancel();
                let _ = wrapper.wait();
                return Exit::cancelled();
            }
            match receiver.recv_timeout(Duration::from_millis(15)) {
                Ok(ev) => self.emit(work, &secretless(ev, token)),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    std::thread::park_timeout(Duration::from_millis(10))
                }
                Err(_) => (),
            }
            match wrapper.try_wait() {
                Ok(Some(code)) => {
                    // A full queue can block the reader. Keep receiving until
                    // it closes its sender, but do not wait indefinitely for a
                    // descendant that still holds the stdout pipe open.
                    let deadline = Instant::now() + Duration::from_secs(2);
                    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
                        match receiver.recv_timeout(remaining) {
                            Ok(ev) => self.emit(work, &secretless(ev, token)),
                            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                                if let Some(handle) = reader_thread {
                                    let _ = handle.join();
                                }
                                break;
                            }
                            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                        }
                    }
                    return Exit {
                        code: Some(code),
                        cancelled: false,
                        detail: None,
                    };
                }
                Ok(None) => (),
                Err(_) => {
                    let _ = wrapper.cancel();
                    let _ = wrapper.wait();
                    return Exit::error("worker exit status unavailable");
                }
            }
        }
    }

    fn emit(&self, work: &RunWork, event: &Option<AgentEvent>) {
        let Some(event) = event else {
            return;
        };
        if let AgentEvent::Session { id } = event {
            let (aid, id) = (work.attempt.id, id.clone());
            let _ = self.runs.store.write(move |db| {
                db.execute(
                    "UPDATE attempts SET agent_thread_id=?2 WHERE id=?1",
                    params![aid, id],
                )?;
                change(
                    db,
                    ChangeTopic::Attempt,
                    &aid.to_string(),
                    &load_attempt(db, aid)?,
                )
            });
        }
        (self.events)(&work.run.id, event.clone());
    }

    fn command(
        &self,
        work: &RunWork,
        turn: i64,
        token: &str,
    ) -> std::result::Result<WorkerCommand, polyclave_adapters::Error> {
        let ctx = WorkerContext {
            executable: match work.run.vendor {
                Vendor::Codex => &self.codex,
                Vendor::Claude => &self.claude,
                Vendor::Agy => &self.agy,
                Vendor::Dsh => &self.dsh,
            },
            state_dir: &self.state,
            worktree: &work.workspace.cwd,
            staging: &work.workspace.staging,
            primary_repo: &work.id.run.repo,
            final_mirror: &work.id.run.mirror,
            mcp_url: &self.mcp_url,
            turn_id: turn,
            token,
            model: work.run.model.as_deref(),
            thread: work.attempt.agent_thread_id.as_deref(),
        };
        match work.run.vendor {
            Vendor::Codex => codex_worker(&ctx),
            Vendor::Claude => claude_worker(&ctx, Uuid::new_v4(), self.confinement.as_deref()),
            Vendor::Agy => agy_worker(&ctx),
            Vendor::Dsh => dsh_worker(&ctx),
        }
    }

    pub(crate) fn release_present(&self, turn: i64) -> Result<bool> {
        match self.marker(turn).symlink_metadata() {
            Ok(m) if m.file_type().is_file() => Ok(true),
            Ok(_) => Err(Error::Invalid("invalid supervisor release evidence")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(_) => Err(Error::Invalid("cannot read supervisor release evidence")),
        }
    }

    pub(crate) fn finish(
        &self,
        turn: i64,
        work: Option<&RunWork>,
        end: EndReason,
        writer: WriterCheck,
        exit: Option<i32>,
    ) -> Result<RunDto> {
        let run = self.settle(turn, work, end, writer, exit)?;
        self.remove_turn_files(turn)?;
        if let (Some(work), Some(run)) = (work, &run) {
            if let Some(sha) = run.result_sha.as_deref() {
                self.runs.sync_verified(&work.id, sha)?;
            }
        }
        run.ok_or(Error::NotFound)
    }

    pub(crate) fn settle(
        &self,
        turn: i64,
        work: Option<&RunWork>,
        end: EndReason,
        writer: WriterCheck,
        exit: Option<i32>,
    ) -> Result<Option<RunDto>> {
        // Revoke the token before importing Git state, so an in-flight MCP
        // verification cannot create a late receipt after this snapshot.
        self.revoke(turn)?;
        let mut verdict = work.map(|w| self.runs.verdict(w, writer)).transpose()?;
        if let (Some(w), Some(v)) = (work, &mut verdict) {
            if end == EndReason::Exit
                && w.attempt.corrections >= 2
                && v.detail.as_deref() == Some(crate::classify::RETRYABLE_DETAIL)
                && self
                    .runs
                    .store
                    .read(|db| Ok(load_turn(db, turn)?.mode == TurnMode::Correction))?
            {
                v.status = RunStatus::Failed;
                v.detail = Some("no accepted result after 2 corrections".into());
            }
        }
        let work = work.map(|w| (w.run.id.clone(), w.attempt.id, w.attempt.attempt_no));
        self.runs.store.write(move|db|{
            let stamp=now(db)?;
            let status=match end {
                EndReason::Cancel=>TurnStatus::Cancelled,
                EndReason::Exit if exit==Some(0)=>TurnStatus::Completed,
                EndReason::Exit=>TurnStatus::Failed,
                _=>TurnStatus::Interrupted,
            };
            db.execute("UPDATE turns SET status=?2,end_reason=?3,writer_check=?4,exit_code=?5,ended_at=?6 WHERE id=?1",
                params![turn,enum_value(status)?,enum_value(end)?,enum_value(writer)?,exit,stamp])?;
            change(db,ChangeTopic::Turn,&turn.to_string(),&load_turn(db,turn)?)?;
            let Some(((id,aid,n),verdict))=work.zip(verdict) else {return Ok(None);};
            let current=load_run(db,&id)?;
            if current.current_attempt!=n || matches!(current.status,RunStatus::Done|RunStatus::Failed){return Ok(Some(current));}
            let end_status=if writer==WriterCheck::Possible {"cancelled"} else if verdict.status==RunStatus::Done {"completed"} else {"interrupted"};
            db.execute("UPDATE attempts SET status=?2,ended_at=?3 WHERE id=?1",params![aid,end_status,stamp])?;
            change(db,ChangeTopic::Attempt,&aid.to_string(),&load_attempt(db,aid)?)?;
            let accepted=if verdict.status==RunStatus::Done{Some(aid)}else{None};
            let source=accepted.map(|_|"agent");
            db.execute("UPDATE runs SET status=?2,detail=?3,result_sha=?4,observed_sha=?5,accepted_attempt_id=?6,
                result_source=?7,updated_at=?8 WHERE id=?1",
                params![id,enum_value(verdict.status)?,verdict.detail,verdict.result_sha,verdict.observed_sha,accepted,source,stamp])?;
            if let Some(sha)=&verdict.result_sha{Store::record_run_memory(db,&id,&current.repo_cwd,"Host verified the result",Some(sha))?;}
            let r=load_run(db,&id)?;
            change(db,ChangeTopic::Run,&id,&r)?;
            Ok(Some(r))
        })
    }

    fn revoke(&self, turn: i64) -> Result<()> {
        self.runs.store.write(move |db| {
            db.execute("UPDATE turns SET mcp_revoked=1 WHERE id=?1", [turn])?;
            Ok(())
        })
    }

    pub(crate) fn remove_turn_files(&self, turn: i64) -> Result<()> {
        remove(&tmp_config_path(&self.state, turn))?;
        remove(&self.marker(turn))
    }
}

struct Slot {
    active: Arc<Mutex<Active>>,
    id: String,
}
impl Drop for Slot {
    fn drop(&mut self) {
        if let Ok(mut a) = self.active.lock() {
            a.remove(&self.id);
        }
    }
}
struct Exit {
    code: Option<i32>,
    cancelled: bool,
    detail: Option<&'static str>,
}
impl Exit {
    fn cancelled() -> Self {
        Self {
            code: None,
            cancelled: true,
            detail: None,
        }
    }
    fn error(detail: &'static str) -> Self {
        Self {
            code: None,
            cancelled: false,
            detail: Some(detail),
        }
    }
}
fn secretless(e: AgentEvent, secret: &str) -> Option<AgentEvent> {
    let contains = match &e {
        AgentEvent::Session { id } => id.contains(secret),
        AgentEvent::TextDelta { text } | AgentEvent::ReasoningDelta { text } => {
            text.contains(secret)
        }
        AgentEvent::Finished { .. } => false,
    };
    if contains {
        None
    } else {
        Some(e)
    }
}

pub(crate) fn remove(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(Error::Invalid("could not remove a temporary turn file")),
    }
}

pub(crate) fn load_turn(db: &Connection, id: i64) -> Result<TurnDto> {
    db.query_row("SELECT id,session_id,run_id,attempt_id,parent_turn_id,mode,status,phase,exit_code,end_reason,writer_check,started_at,ended_at FROM turns WHERE id=?1",[id],
        |r|Ok(TurnDto{id:r.get(0)?,session_id:r.get(1)?,run_id:r.get(2)?,attempt_id:r.get(3)?,parent_turn_id:r.get(4)?,
            mode:decode_enum(&r.get::<_,String>(5)?)?,status:decode_enum(&r.get::<_,String>(6)?)?,phase:decode_enum(&r.get::<_,String>(7)?)?,
            exit_code:r.get(8)?,end_reason:r.get::<_,Option<String>>(9)?.as_deref().map(decode_enum).transpose()?,
            writer_check:r.get::<_,Option<String>>(10)?.as_deref().map(decode_enum).transpose()?,started_at:r.get(11)?,ended_at:r.get(12)?}))
       .optional()?.ok_or(Error::NotFound)
}
