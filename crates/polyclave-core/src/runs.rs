use crate::{
    classify::{classify_run, BranchCheck, Verdict},
    mirror::{AttemptGit, MirrorManager, RunGit},
    worktree::Workspace,
};
use polyclave_adapters::mcp::{AttemptScope, HostTools};
use polyclave_os::canonical_real_path;
use polyclave_proto::{
    AttemptDto, ChangeTopic, ResultSource, RunDto, RunEvidenceDto, RunStatus, TestStatus, TurnMode,
    Vendor, WriterCheck,
};
use polyclave_store::{
    decode_enum, enum_value, insert_run_origin_snapshot, new_id, now, parse_json, reserve_ticket,
    Error, Store,
};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

pub type Result<T> = std::result::Result<T, Error>;

/// An extra condition on admitting a new Run, such as a per-node cap.
pub trait AdmissionGuard: Send + Sync + 'static {
    /// Read-only early refusal, before any mirror work is done.
    fn check(&self, db: &Connection) -> Result<()>;
    /// Runs inside the admission transaction after the Run row is written.
    /// An error rolls the whole admission back.
    fn admitted(&self, db: &Connection, run_id: &str) -> Result<()>;
}

/// `(sha, tests, tests_status, not_done)` as `submit_result` stored them.
type ReceiptRow = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

#[derive(Clone)]
pub struct RunManager {
    pub store: Store,
    pub git: Arc<MirrorManager>,
    gate: Arc<Mutex<()>>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunRequest {
    pub hub_session_id: String,
    pub request_id: String,
    pub vendor: Vendor,
    pub model: Option<String>,
    pub repo_cwd: PathBuf,
    pub instructions: String,
    pub base_branch: String,
    pub ticket_id: Option<String>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Resolution {
    Accept,
    Fail,
    AllowRetry,
}

#[derive(Clone)]
pub struct RunWork {
    pub run: RunDto,
    pub attempt: AttemptDto,
    pub id: AttemptGit,
    pub workspace: Workspace,
}

impl RunManager {
    pub fn new(store: Store, git: Arc<MirrorManager>) -> Self {
        Self {
            store,
            git,
            gate: Arc::new(Mutex::new(())),
        }
    }
    pub fn get(&self, id: &str) -> Result<RunDto> {
        self.store.read(|db| load_run(db, id))
    }
    pub fn list(&self) -> Result<Vec<RunDto>> {
        self.store.read(|db| {
            let mut s = db.prepare(&format!(
                "SELECT {RUN_COLUMNS} FROM runs ORDER BY created_at,id"
            ))?;
            let rows = s.query_map([], row_run)?.collect::<rusqlite::Result<_>>()?;
            Ok(rows)
        })
    }

    /// Returns the original Run when a client repeats the same admission key.
    pub fn accept(&self, request: RunRequest) -> Result<(RunDto, bool)> {
        self.accept_guarded(request, None)
    }

    /// Like [`RunManager::accept`], with an extra admission check. A repeated
    /// admission key returns the original Run before the guard is consulted.
    /// For a new Run the guard is checked before the mirror baseline is made
    /// and again inside the transaction that writes the Run, under the same
    /// admission gate, so two concurrent admissions cannot both pass it.
    pub fn accept_guarded(
        &self,
        request: RunRequest,
        guard: Option<Arc<dyn AdmissionGuard>>,
    ) -> Result<(RunDto, bool)> {
        self.accept_inner(request, guard, false)
    }

    /// Admission for the local direct-dispatch endpoint.
    pub fn accept_local_worker(&self, request: RunRequest) -> Result<(RunDto, bool)> {
        self.accept_inner(request, None, true)
    }

    fn accept_inner(
        &self,
        request: RunRequest,
        guard: Option<Arc<dyn AdmissionGuard>>,
        require_staging_scope: bool,
    ) -> Result<(RunDto, bool)> {
        let _guard = self.gate.lock().map_err(|_| Error::Closed)?;
        if request.request_id.trim().is_empty()
            || request.request_id.len() > 256
            || request.instructions.trim().is_empty()
            || request.instructions.len() > 1024 * 1024
        {
            return Err(Error::Invalid("missing or oversized admission fields"));
        }
        let existing = self.store.read(|db| {
            Ok(db
                .query_row(
                    "SELECT id FROM runs WHERE hub_session_id=?1 AND request_id=?2",
                    [&request.hub_session_id, &request.request_id],
                    |r| r.get::<_, String>(0),
                )
                .optional()?)
        })?;
        if let Some(id) = existing {
            let saved = self.get(&id)?;
            let same_repo = Path::new(&saved.repo_cwd) == request.repo_cwd
                || canonical_real_path(&request.repo_cwd)
                    .is_ok_and(|path| path == Path::new(&saved.repo_cwd));
            if !same_repo
                || saved.vendor != request.vendor
                || saved.model != request.model
                || saved.instructions != request.instructions
                || saved.base_branch != request.base_branch
                || request
                    .ticket_id
                    .as_ref()
                    .is_some_and(|ticket| ticket != &saved.ticket_id)
            {
                return Err(Error::ConflictError);
            }
            return Ok((saved, false));
        }
        if require_staging_scope {
            self.check_local_worker_scope(
                request.vendor,
                &request.hub_session_id,
                request.ticket_id.as_deref(),
            )?;
        }
        if let Some(guard) = &guard {
            self.store.read(|db| guard.check(db))?;
        }
        let repo = canonical_real_path(&request.repo_cwd)
            .map_err(|_| Error::Invalid("repository is unavailable"))?;
        let repo_text = path_text(&repo)?;
        let id = new_id('r');
        self.store.read(|db| {
            let hub: Option<(String, String)> = db
                .query_row(
                    "SELECT role,repo_cwd FROM sessions WHERE id=?1",
                    [&request.hub_session_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            if hub.as_ref().map(|(a, b)| (a.as_str(), b.as_str()))
                != Some(("hub", repo_text.as_str()))
            {
                return Err(Error::ConflictError);
            }
            Ok(())
        })?;
        let run_git = self
            .git
            .admit(&repo, &id, &request.base_branch)
            .map_err(|_| Error::Invalid("cannot initialize immutable repository baseline"))?;
        // Snapshot origin for the post-R1 publish step only; it never affects
        // admission, dispatch or verification.
        let origin = self.git.origin_snapshot(&repo);
        self.store.write(move |db|{
            let stamp=now(db)?;
            let ticket=if let Some(ticket)=request.ticket_id {
                let owner:Option<String>=db.query_row("SELECT hub_session_id FROM tickets WHERE id=?1",[&ticket],|r|r.get(0)).optional()?;
                if owner.as_deref()!=Some(&request.hub_session_id) {return Err(Error::ConflictError);}
                ticket
            } else {
                let ticket=new_id('t');
                db.execute("INSERT INTO tickets(id,hub_session_id,repo_cwd,title,mandate_json,source_kind,max_runs,status,created_at,updated_at)
                    VALUES(?1,?2,?3,'Direct request','{}','explicit_user',1,'active',?4,?4)",
                    params![ticket,request.hub_session_id,repo_text,stamp])?;
                ticket
            };
            reserve_ticket(db,&ticket,&repo_text)?;
            let worker=new_id('s');
            let vendor=enum_value(request.vendor)?;
            let profile=format!("builtin-{vendor}");
            db.execute("INSERT OR IGNORE INTO provider_profiles(id,display_name,kind,models_json,created_at)
                VALUES(?1,?2,?3,'[]',?4)",params![profile,vendor,format!("{vendor}_cli"),stamp])?;
            db.execute("INSERT INTO sessions(id,name,role,vendor,repo_cwd,profile_id,provider_model,created_at)
                VALUES(?1,?2,'worker',?3,?4,?5,?6,?7)",
                params![worker,format!("Worker {id}"),vendor,repo_text,profile,request.model.as_deref().unwrap_or("default"),stamp])?;
            db.execute("INSERT INTO runs(id,ticket_id,hub_session_id,worker_session_id,request_id,repo_cwd,vendor,model,instructions,
                mirror_remote,base_branch,base_sha,status,created_at,updated_at)
                VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,'pending',?13,?13)",
                params![id,ticket,request.hub_session_id,worker,request.request_id,repo_text,vendor,
                        request.model,request.instructions,path_text(&run_git.mirror)?,request.base_branch,run_git.base,stamp])?;
            insert_run_origin_snapshot(db,&id,&origin)?;
            if let Some(guard)=&guard { guard.admitted(db,&id)?; }
            change(db,ChangeTopic::Ticket,&ticket,&ticket_state(db,&ticket)?)?;
            change(db,ChangeTopic::Run,&id,&load_run(db,&id)?)?;
            Ok((load_run(db,&id)?,true))
        })
    }

    /// Reused Runs from before host staging may still have an implicit Ticket.
    pub fn check_local_worker_scope(
        &self,
        vendor: Vendor,
        hub: &str,
        ticket: Option<&str>,
    ) -> Result<()> {
        if uses_host_staging(vendor) {
            let (saved, mandate) = self
                .store
                .hub_ticket_mandate(ticket.ok_or(STAGING_SCOPE_REQUIRED)?)?;
            if saved.hub != hub {
                return Err(Error::ConflictError);
            }
            staging_allowed_paths(&mandate)?;
        }
        Ok(())
    }

    pub fn begin(&self, id: &str, mode: TurnMode) -> Result<RunWork> {
        let _guard = self.gate.lock().map_err(|_| Error::Closed)?;
        let run = self.get(id)?;
        if matches!(run.status, RunStatus::Done | RunStatus::Failed) {
            return Err(Error::ConflictError);
        }
        let occupied=self.store.read(|db|Ok(db.query_row(
            "SELECT EXISTS(SELECT 1 FROM turns WHERE run_id=?1 AND status='running' AND mode!='consult')",
            [id],|r|r.get::<_,bool>(0))?))?;
        if occupied {
            return Err(Error::BusyError);
        }
        let previous = self.store.read(|db| current_attempt(db, &run))?;
        let new = match mode {
            TurnMode::Start if run.status == RunStatus::Pending && previous.is_none() => true,
            TurnMode::Retry
                if run.status == RunStatus::Interrupted
                    || run.resolution.as_deref() == Some("allow_retry") =>
            {
                if run.resolution.as_deref() != Some("allow_retry") {
                    if let Some(prev) = &previous {
                        if self.attempt_has_turn(prev.id)?
                            && self
                                .git
                                .reconcile(&self.attempt_git(&run, prev.attempt_no)?)
                                != BranchCheck::Absent
                        {
                            return Err(Error::ConflictError);
                        }
                    }
                }
                true
            }
            TurnMode::WorkerChat
                if matches!(
                    run.status,
                    RunStatus::Running | RunStatus::Interrupted | RunStatus::NeedsReview
                ) =>
            {
                previous.as_ref().is_some_and(|a| {
                    matches!(
                        a.status,
                        polyclave_proto::AttemptStatus::Cancelled
                            | polyclave_proto::AttemptStatus::Superseded
                    )
                })
            }
            TurnMode::Correction if run.status == RunStatus::Running => false,
            _ => return Err(Error::ConflictError),
        };
        if !new {
            let attempt = previous.ok_or(Error::ConflictError)?;
            let git = self.attempt_git(&run, attempt.attempt_no)?;
            if mode == TurnMode::WorkerChat
                && uses_host_staging(run.vendor)
                && !self.git.has_commit_stage_dir(&git)
            {
                return Err(Error::Invalid(
                    "This Attempt has no host staging directory. Choose Allow retry, then Retry, to create a fresh Attempt.",
                ));
            }
            let workspace = Workspace {
                cwd: PathBuf::from(&attempt.worktree_path),
                staging: self.git.staging_path(&git),
                branch: attempt.branch.clone(),
            };
            let rid = run.id.clone();
            let run = self.store.write(move |db| {
                let changed = db.execute(
                    "UPDATE runs SET status='running',updated_at=?2 WHERE id=?1 AND status!='running'",
                    params![rid, now(db)?],
                )?;
                let updated = load_run(db, &rid)?;
                if changed > 0 {
                    change(db, ChangeTopic::Run, &rid, &updated)?;
                }
                Ok(updated)
            })?;
            return Ok(RunWork {
                run,
                attempt,
                id: git,
                workspace,
            });
        }
        let (attempt, git) = self.create_attempt(&run, previous, None, false)?;
        let workspace = match self.git.prepare_attempt(&git) {
            Ok(workspace) => workspace,
            Err(_) => {
                self.interrupt_unstarted_attempt(attempt)?;
                return Err(Error::Invalid("cannot prepare isolated Attempt"));
            }
        };
        let run = self.get(&run.id)?;
        Ok(RunWork {
            run,
            attempt,
            id: git,
            workspace,
        })
    }

    /// Creates the next Attempt and moves the Run to `running`. `from_sha` is
    /// the commit a revision continues from; `clear_result` drops the Run's own
    /// verified result so a revision can never inherit it.
    fn create_attempt(
        &self,
        run: &RunDto,
        previous: Option<AttemptDto>,
        from_sha: Option<&str>,
        clear_result: bool,
    ) -> Result<(AttemptDto, AttemptGit)> {
        let n = run
            .current_attempt
            .checked_add(1)
            .filter(|n| *n <= run.max_attempts)
            .ok_or(Error::BudgetExceededError)?;
        let git = self.attempt_git(run, n)?;
        let reserved = self.git.attempt_workspace(&git);
        let path = path_text(&reserved.cwd)?;
        let branch = reserved.branch;
        let id = run.id.clone();
        let from_sha = from_sha.map(str::to_owned);
        let attempt = self.store.write(move |db| {
            let stamp = now(db)?;
            if let Some(previous) = previous {
                db.execute(
                    "UPDATE attempts SET status='superseded',ended_at=COALESCE(ended_at,?2) WHERE id=?1",
                    params![previous.id, stamp],
                )?;
                change(
                    db,
                    ChangeTopic::Attempt,
                    &previous.id.to_string(),
                    &load_attempt(db, previous.id)?,
                )?;
            }
            db.execute(
                "INSERT INTO attempts(run_id,attempt_no,from_sha,branch,worktree_path,status,created_at)
                 VALUES(?1,?2,?3,?4,?5,'prepared',?6)",
                params![id, n, from_sha, branch, path, stamp],
            )?;
            let attempt = load_attempt(db, db.last_insert_rowid())?;
            db.execute(
                "UPDATE runs SET current_attempt=?2,status='running',detail=NULL,resolution=NULL,
                   result_sha=CASE WHEN ?4 THEN NULL ELSE result_sha END,
                   observed_sha=CASE WHEN ?4 THEN NULL ELSE observed_sha END,
                   result_source=CASE WHEN ?4 THEN NULL ELSE result_source END,
                   accepted_attempt_id=CASE WHEN ?4 THEN NULL ELSE accepted_attempt_id END,
                   updated_at=?3
                 WHERE id=?1",
                params![id, n, stamp, clear_result],
            )?;
            change(db, ChangeTopic::Attempt, &attempt.id.to_string(), &attempt)?;
            change(db, ChangeTopic::Run, &id, &load_run(db, &id)?)?;
            Ok(attempt)
        })?;
        Ok((attempt, git))
    }

    /// Starts a PR-gate revision: a fresh Attempt branched from the previous
    /// Attempt's R1-verified SHA, with the Run's immutable baseline untouched.
    ///
    /// The Run's own verified result is cleared first, so the revised commit
    /// needs its own accepted receipt: a missing one leaves the Run in
    /// `needs_review` with no `result_sha`, which the PR gate refuses. The prior
    /// SHA and any approval of it are never reused.
    pub fn begin_revision(&self, id: &str) -> Result<RunWork> {
        let _guard = self.gate.lock().map_err(|_| Error::Closed)?;
        let run = self.get(id)?;
        let Some(prior) = self.r1_verified_sha(&run)? else {
            return Err(Error::ConflictError);
        };
        let occupied = self.store.read(|db| {
            Ok(db.query_row(
                "SELECT EXISTS(SELECT 1 FROM turns WHERE run_id=?1 AND status='running' AND mode!='consult')",
                [id],
                |r| r.get::<_, bool>(0),
            )?)
        })?;
        if occupied {
            return Err(Error::BusyError);
        }
        let previous = self.store.read(|db| current_attempt(db, &run))?;
        let (attempt, git) = self.create_attempt(&run, previous, Some(&prior), true)?;
        let workspace = match self.git.prepare_revision_attempt(&git, &prior) {
            Ok(workspace) => workspace,
            Err(_) => {
                self.interrupt_unstarted_attempt(attempt)?;
                return Err(Error::Invalid("cannot prepare the revision Attempt"));
            }
        };
        let run = self.get(&run.id)?;
        Ok(RunWork {
            run,
            attempt,
            id: git,
            workspace,
        })
    }

    pub(crate) fn attempt_has_turn(&self, attempt_id: i64) -> Result<bool> {
        self.store.read(|db| {
            Ok(db.query_row(
                "SELECT EXISTS(SELECT 1 FROM turns WHERE attempt_id=?1)",
                [attempt_id],
                |row| row.get(0),
            )?)
        })
    }

    /// The next dispatch takes a fresh number. The abandoned files remain
    /// isolated even if their Git layout was only partially created.
    pub(crate) fn interrupt_unstarted_attempt(&self, attempt: AttemptDto) -> Result<()> {
        self.store.write(move |db| {
            let stamp = now(db)?;
            let changed = db.execute(
                "UPDATE attempts SET status='superseded',ended_at=?2 WHERE id=?1 AND status='prepared'
                 AND NOT EXISTS (SELECT 1 FROM turns WHERE attempt_id=?1)",
                params![attempt.id, stamp],
            )?;
            if changed != 1 {
                return Err(Error::ConflictError);
            }
            let changed = db.execute(
                "UPDATE runs SET status='interrupted',detail='Attempt did not start; safe to retry',
                 result_sha=NULL,observed_sha=NULL,accepted_attempt_id=NULL,result_source=NULL,updated_at=?3
                 WHERE id=?1 AND current_attempt=?2 AND status='running'",
                params![attempt.run_id, attempt.attempt_no, stamp],
            )?;
            if changed != 1 {
                return Err(Error::ConflictError);
            }
            change(db, ChangeTopic::Attempt, &attempt.id.to_string(), &load_attempt(db, attempt.id)?)?;
            change(db, ChangeTopic::Run, &attempt.run_id, &load_run(db, &attempt.run_id)?)
        })
    }

    pub fn commit_for_run(&self, run: &RunDto) -> Result<Option<crate::mirror::CommitSummary>> {
        let Some(sha) = run.result_sha.as_ref().or(run.observed_sha.as_ref()) else {
            return Ok(None);
        };
        self.git
            .commit_summary(&self.attempt_git(run, run.current_attempt)?, sha)
            .map(Some)
            .map_err(|_| Error::Invalid("mirror is unavailable for the selected commit"))
    }

    /// The Run's `result_sha` only when the host itself verified it.
    ///
    /// Provenance is read from the ledger, not from the message: R1 must have
    /// classified the Run (`result_source='agent'`, a completed Run, no open
    /// reason in `detail`) and the accepted Attempt must still hold an
    /// accepted `submit_result` receipt for exactly that SHA. A human accept
    /// keeps `result_source='user'` and stores no receipt, so it can close the
    /// Run for a person but never feeds auto-publish, review or merge.
    pub fn r1_verified_sha(&self, run: &RunDto) -> Result<Option<String>> {
        let Some(sha) = run.result_sha.clone().filter(|s| !s.trim().is_empty()) else {
            return Ok(None);
        };
        if run.result_source != Some(ResultSource::Agent)
            || run.status != RunStatus::Done
            || run.detail.is_some()
        {
            return Ok(None);
        }
        let Some(attempt) = run.accepted_attempt_id else {
            return Ok(None);
        };
        let receipt: Option<String> = self.store.read(move |db| {
            Ok(db
                .query_row(
                    "SELECT json_extract(payload_json,'$.sha') FROM results
                     WHERE attempt_id=?1 AND accepted=1",
                    [attempt],
                    |r| r.get::<_, Option<String>>(0),
                )
                .optional()?
                .flatten())
        })?;
        Ok((receipt.as_deref() == Some(sha.as_str())).then_some(sha))
    }

    /// Approval evidence for the Run's current Attempt only: the accepted
    /// `submit_result` receipt (with the Worker's own notes), the latest
    /// `report_progress` claim, and every test result recorded for exactly one
    /// SHA. Nothing from an older Attempt, and no result from another SHA, is
    /// returned.
    pub fn evidence_for_run(&self, run: &RunDto) -> Result<RunEvidenceDto> {
        let (id, number) = (run.id.clone(), run.current_attempt);
        self.store.read(move |db| {
            // The SHA the test rows belong to: verified result first, else the
            // observed tip. Both come from the Run, never from a message.
            let test_sha: Option<String> = db
                .query_row(
                    "SELECT COALESCE(result_sha, observed_sha) FROM runs WHERE id=?1",
                    [&id],
                    |r| r.get::<_, Option<String>>(0),
                )
                .optional()?
                .flatten();
            let test_results = match &test_sha {
                Some(sha) => polyclave_store::test_results_for_sha(db, &id, sha)?,
                None => Vec::new(),
            };
            let attempt: Option<i64> = db
                .query_row(
                    "SELECT id FROM attempts WHERE run_id=?1 AND attempt_no=?2",
                    params![id, number],
                    |r| r.get(0),
                )
                .optional()?;
            let Some(attempt) = attempt else {
                return Ok(RunEvidenceDto {
                    attempt_no: number,
                    receipt_sha: None,
                    worker_claim: None,
                    test_sha,
                    receipt_tests: None,
                    receipt_tests_status: None,
                    receipt_not_done: None,
                    test_results,
                });
            };
            let receipt: Option<ReceiptRow> = db
                .query_row(
                    "SELECT json_extract(payload_json,'$.sha'),
                            json_extract(payload_json,'$.tests'),
                            json_extract(payload_json,'$.tests_status'),
                            json_extract(payload_json,'$.not_done')
                     FROM results WHERE attempt_id=?1 AND accepted=1
                     ORDER BY id DESC LIMIT 1",
                    [attempt],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()?;
            let (receipt_sha, receipt_tests, receipt_tests_status, receipt_not_done) = match receipt
            {
                Some((sha, tests, status, not_done)) => (
                    sha,
                    tests,
                    status.as_deref().and_then(TestStatus::parse),
                    not_done,
                ),
                None => (None, None, None, None),
            };
            let worker_claim = db
                .query_row(
                    "SELECT json_extract(e.payload_json,'$.text') FROM events e
                     JOIN turns t ON t.id=e.turn_id
                     WHERE t.attempt_id=?1 AND e.event_kind='progress'
                     ORDER BY e.id DESC LIMIT 1",
                    [attempt],
                    |r| r.get::<_, Option<String>>(0),
                )
                .optional()?
                .flatten();
            Ok(RunEvidenceDto {
                attempt_no: number,
                receipt_sha,
                worker_claim,
                test_sha,
                receipt_tests,
                receipt_tests_status,
                receipt_not_done,
                test_results,
            })
        })
    }

    pub(crate) fn attempt_git(&self, run: &RunDto, number: u32) -> Result<AttemptGit> {
        let mirror = self.store.read(|db| {
            Ok(db.query_row(
                "SELECT mirror_remote FROM runs WHERE id=?1",
                [&run.id],
                |r| r.get::<_, String>(0),
            )?)
        })?;
        Ok(AttemptGit {
            run: RunGit {
                repo: run.repo_cwd.clone().into(),
                mirror: mirror.into(),
                id: run.id.clone(),
                base: run.base_sha.clone(),
            },
            number,
        })
    }

    pub fn resolve(&self, id: &str, decision: Resolution, note: &str) -> Result<RunDto> {
        self.resolve_seen(id, decision, note, None)
    }
    pub fn resolve_seen(
        &self,
        id: &str,
        decision: Resolution,
        note: &str,
        seen_sha: Option<&str>,
    ) -> Result<RunDto> {
        if note.len() > 4096 || note.trim().is_empty() {
            return Err(Error::Invalid("resolution requires a note"));
        }
        let _guard = self.gate.lock().map_err(|_| Error::Closed)?;
        let run = self.get(id)?;
        if !matches!(run.status, RunStatus::NeedsReview | RunStatus::Interrupted) {
            return Err(Error::ConflictError);
        }
        let busy = self.store.read(|db| {
            Ok(db.query_row(
                "SELECT EXISTS(SELECT 1 FROM turns WHERE run_id=?1 AND status='running')",
                [id],
                |r| r.get::<_, bool>(0),
            )?)
        })?;
        if busy {
            return Err(Error::BusyError);
        }
        let a = self.attempt_git(&run, run.current_attempt)?;
        let tip = match decision {
            Resolution::Accept => match self.git.reconcile(&a) {
                BranchCheck::Present { tip, .. } if seen_sha == Some(tip.as_str()) => Some(tip),
                _ => return Err(Error::ConflictError),
            },
            _ => None,
        };
        let id = id.to_owned();
        let note = note.to_owned();
        let completed=self.store.write(move|db|{
            let r=load_run(db,&id)?;
            if !matches!(r.status,RunStatus::NeedsReview|RunStatus::Interrupted) {
                return Err(Error::ConflictError);
            }
            let stamp=now(db)?;
            let attempt=current_attempt(db,&r)?;
            if db.query_row("SELECT EXISTS(SELECT 1 FROM turns WHERE run_id=?1 AND status='running')",[&id],|r|r.get::<_,bool>(0))? {
                return Err(Error::BusyError);
            }
            let (status,resolution,source,accepted_id)=match decision {
                Resolution::Accept=>{
                    if let Some(a)=&attempt {
                        db.execute("UPDATE attempts SET status='completed',ended_at=COALESCE(ended_at,?2) WHERE id=?1",params![a.id,stamp])?;
                        change(db,ChangeTopic::Attempt,&a.id.to_string(),&load_attempt(db,a.id)?)?;
                    }
                    ("done","accept",Some("user"),attempt.as_ref().map(|a|a.id))
                }
                Resolution::Fail=>("failed","fail",None,None),
                Resolution::AllowRetry=>{
                    if r.current_attempt>=r.max_attempts{return Err(Error::BudgetExceededError);}
                    if let Some(a)=&attempt {
                        db.execute("UPDATE attempts SET status='superseded',ended_at=COALESCE(ended_at,?2) WHERE id=?1",params![a.id,stamp])?;
                        change(db,ChangeTopic::Attempt,&a.id.to_string(),&load_attempt(db,a.id)?)?;
                    }
                    ("interrupted","allow_retry",None,None)
                }
            };
            db.execute("UPDATE runs SET status=?2,result_sha=?3,result_source=?4,resolution=?5,accepted_attempt_id=COALESCE(?6,accepted_attempt_id),detail=NULL,updated_at=?7 WHERE id=?1",
                params![id,status,tip,source,resolution,accepted_id,stamp])?;
            Store::record_run_memory(db,&id,&r.repo_cwd,&note,tip.as_deref())?;
            let result=load_run(db,&id)?;
            change(db,ChangeTopic::Run,&id,&result)?;
            Ok(result)
        })?;
        if let Some(sha) = completed.result_sha.as_deref() {
            self.sync_verified(&a, sha)?;
        }
        Ok(completed)
    }

    /// Terminal state is durable before any pullback. Failure only records a warning.
    pub(crate) fn sync_verified(&self, a: &AttemptGit, sha: &str) -> Result<()> {
        if self.git.sync_done(a, sha).is_err() {
            let id = a.run.id.clone();
            self.store.write(move |db| {
                change(
                    db,
                    ChangeTopic::HubReceipt,
                    &id,
                    &json!({"warning":"verified result could not be pulled back; resync is safe"}),
                )
            })?;
        }
        Ok(())
    }

    pub(crate) fn verdict(&self, work: &RunWork, writer: WriterCheck) -> Result<Verdict> {
        let current=self.store.read(|db|Ok(db.query_row(
            "SELECT json_extract(payload_json,'$.sha') FROM results WHERE attempt_id=?1 AND accepted=1",[work.attempt.id],|r|r.get::<_,String>(0)).optional()?))?;
        Ok(classify_run(
            writer,
            current.as_deref(),
            self.git.reconcile(&work.id),
        ))
    }

    pub(crate) fn old_work(&self, id: &str, attempt_id: i64) -> Result<RunWork> {
        let run = self.get(id)?;
        let attempt = self.store.read(|db| load_attempt(db, attempt_id))?;
        if attempt.run_id != run.id {
            return Err(Error::ConflictError);
        }
        let a = self.attempt_git(&run, attempt.attempt_no)?;
        let workspace = Workspace {
            cwd: attempt.worktree_path.clone().into(),
            staging: self.git.staging_path(&a),
            branch: attempt.branch.clone(),
        };
        Ok(RunWork {
            run,
            attempt,
            id: a,
            workspace,
        })
    }
}

impl HostTools for RunManager {
    fn commit_and_stage(
        &self,
        scope: &AttemptScope,
        turn_id: i64,
        message: &str,
        paths: Option<&[String]>,
    ) -> Result<String> {
        let work = self.old_work(&scope.run, scope.id)?;
        if work.run.current_attempt != scope.number
            || work.attempt.attempt_no != scope.number
            || work.attempt.branch != scope.branch
            || work.run.base_sha != scope.base
            || work.run.repo_cwd != scope.repo
            || !uses_host_staging(work.run.vendor)
        {
            return Err(Error::ConflictError);
        }
        let (_, mandate) = self.store.hub_ticket_mandate(&work.run.ticket_id)?;
        let allowed = staging_allowed_paths(&mandate)?;
        let store = self.store.clone();
        let attempt_id = scope.id;
        self.git
            .commit_and_stage(&work.id, message, paths, &allowed, move || {
                store
                    .read(|db| {
                        Ok(db.query_row(
                            "SELECT EXISTS(SELECT 1 FROM turns t JOIN runs r ON r.id=t.run_id
                             JOIN attempts a ON a.id=t.attempt_id AND a.run_id=r.id
                             WHERE t.id=?1 AND t.attempt_id=?2 AND t.status='running'
                               AND t.phase='authorized' AND t.mcp_revoked=0
                               AND t.mode IN ('start','retry','correction','worker_chat')
                               AND a.status='running' AND a.attempt_no=r.current_attempt
                               AND r.status='running' AND NOT EXISTS
                                   (SELECT 1 FROM results WHERE attempt_id=a.id AND accepted=1))",
                            params![turn_id, attempt_id],
                            |row| row.get(0),
                        )?)
                    })
                    .unwrap_or(false)
            })
            .map_err(|error| match error {
                crate::GitError::Invalid(reason) => Error::Invalid(reason),
                _ => Error::Invalid("Host could not stage this Attempt"),
            })
    }

    fn verified_tip(&self, scope: &AttemptScope, sha: &str) -> Result<bool> {
        let work = self.old_work(&scope.run, scope.id)?;
        if work.attempt.attempt_no != scope.number
            || work.attempt.branch != scope.branch
            || work.run.base_sha != scope.base
        {
            return Err(Error::ConflictError);
        }
        match self.git.reconcile(&work.id) {
            BranchCheck::Unreadable => Err(Error::Invalid("mirror read failed")),
            BranchCheck::Present { tip, problem: None } => Ok(tip == sha),
            _ => Ok(false),
        }
    }
    fn consult_read_only(&self, scope: &AttemptScope, _question: &str) -> Result<String> {
        let work = self.old_work(&scope.run, scope.id)?;
        // The phase 1A local advisor surface exposes only host-owned, read-only facts.
        Ok(format!(
            "Run {} uses base {} and Attempt {}.",
            work.run.id, work.run.base_sha, work.attempt.attempt_no
        ))
    }
}

fn uses_host_staging(vendor: Vendor) -> bool {
    matches!(vendor, Vendor::Codex | Vendor::Claude)
}

const STAGING_SCOPE_REQUIRED: Error = Error::Invalid(
    "Host staging requires a Ticket with allowed paths. Create a scoped Ticket and dispatch from it.",
);

fn staging_allowed_paths(mandate: &serde_json::Value) -> Result<Vec<String>> {
    mandate
        .get("allowed_paths")
        .cloned()
        .and_then(|paths| serde_json::from_value(paths).ok())
        .filter(|paths: &Vec<String>| {
            !paths.is_empty() && paths.iter().all(|path| !path.trim().is_empty())
        })
        .ok_or(STAGING_SCOPE_REQUIRED)
}

pub(crate) fn path_text(p: &Path) -> Result<String> {
    p.to_str()
        .map(str::to_owned)
        .ok_or(Error::Invalid("host paths must be valid Unicode"))
}
pub(crate) fn change<T: Serialize>(
    db: &Connection,
    topic: ChangeTopic,
    id: &str,
    t: &T,
) -> Result<()> {
    Store::append_change(db, topic, id, &serde_json::to_value(t)?)?;
    Ok(())
}
fn ticket_state(db: &Connection, id: &str) -> Result<serde_json::Value> {
    Ok(db.query_row("SELECT status,max_runs,runs_used FROM tickets WHERE id=?1",[id],
        |r|Ok(json!({"id":id,"status":r.get::<_,String>(0)?,"max_runs":r.get::<_,u32>(1)?,"runs_used":r.get::<_,u32>(2)?})))?)
}

const RUN_COLUMNS:&str="id,ticket_id,hub_session_id,worker_session_id,request_id,repo_cwd,vendor,model,instructions,context_refs_json,
    base_branch,base_sha,status,current_attempt,max_attempts,accepted_attempt_id,detail,result_sha,observed_sha,result_source,resolution,created_at,updated_at";
pub(crate) fn load_run(db: &Connection, id: &str) -> Result<RunDto> {
    db.query_row(
        &format!("SELECT {RUN_COLUMNS} FROM runs WHERE id=?1"),
        [id],
        row_run,
    )
    .optional()?
    .ok_or(Error::NotFound)
}
fn row_run(r: &Row<'_>) -> rusqlite::Result<RunDto> {
    Ok(RunDto {
        id: r.get(0)?,
        ticket_id: r.get(1)?,
        hub_session_id: r.get(2)?,
        worker_session_id: r.get(3)?,
        request_id: r.get(4)?,
        repo_cwd: r.get(5)?,
        vendor: decode_enum(&r.get::<_, String>(6)?)?,
        model: r.get(7)?,
        instructions: r.get(8)?,
        context_refs_json: r
            .get::<_, Option<String>>(9)?
            .map(|s| parse_json(s, 9))
            .transpose()?,
        base_branch: r.get(10)?,
        base_sha: r.get(11)?,
        status: decode_enum(&r.get::<_, String>(12)?)?,
        current_attempt: r.get(13)?,
        max_attempts: r.get(14)?,
        accepted_attempt_id: r.get(15)?,
        detail: r.get(16)?,
        result_sha: r.get(17)?,
        observed_sha: r.get(18)?,
        result_source: r
            .get::<_, Option<String>>(19)?
            .as_deref()
            .map(decode_enum)
            .transpose()?,
        resolution: r.get(20)?,
        created_at: r.get(21)?,
        updated_at: r.get(22)?,
    })
}
pub(crate) fn load_attempt(db: &Connection, id: i64) -> Result<AttemptDto> {
    db.query_row("SELECT id,run_id,attempt_no,branch,worktree_path,agent_thread_id,corrections,status,created_at,ended_at FROM attempts WHERE id=?1",
        [id],|r|Ok(AttemptDto{id:r.get(0)?,run_id:r.get(1)?,attempt_no:r.get(2)?,branch:r.get(3)?,worktree_path:r.get(4)?,
            agent_thread_id:r.get(5)?,corrections:r.get(6)?,status:decode_enum(&r.get::<_,String>(7)?)?,created_at:r.get(8)?,ended_at:r.get(9)?}))
        .optional()?.ok_or(Error::NotFound)
}
fn current_attempt(db: &Connection, r: &RunDto) -> Result<Option<AttemptDto>> {
    let id = db
        .query_row(
            "SELECT id FROM attempts WHERE run_id=?1 AND attempt_no=?2",
            params![r.id, r.current_attempt],
            |r| r.get(0),
        )
        .optional()?;
    id.map(|a| load_attempt(db, a)).transpose()
}

impl RunManager {
    pub fn read_review_diff(
        &self,
        id: &str,
        scope: &Path,
        allow_path: impl Fn(&str) -> bool,
    ) -> Result<(Vec<String>, String)> {
        let r = self.get(id)?;
        if Path::new(&r.repo_cwd) != scope {
            return Err(Error::ConflictError);
        }
        let tip = r
            .result_sha
            .as_ref()
            .or(r.observed_sha.as_ref())
            .ok_or(Error::ConflictError)?;
        let git = self.attempt_git(&r, r.current_attempt)?;
        self.git
            .review_diff(&git, tip, allow_path)
            .map_err(|_| Error::ConflictError)
    }
}
