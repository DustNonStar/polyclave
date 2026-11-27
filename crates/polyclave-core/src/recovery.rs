//! Crash recovery never restarts a Worker or discards release evidence early.
use crate::{
    runner::{remove, TurnRunner},
    runs::{change, load_run, Result},
};
use polyclave_os::{exclude_old_supervisor, hard_stop_excludes_writers, ProcessIdentity};
use polyclave_proto::{ChangeTopic, EndReason, WriterCheck};
use polyclave_store::{now, Error};

struct Stale {
    id: i64,
    run: Option<String>,
    attempt: Option<i64>,
    boot: String,
    pid: Option<String>,
    started: Option<String>,
}

impl TurnRunner {
    pub fn recover(&self) -> Result<usize> {
        let stale = self.runs.store.read(|db| {
            let mut stmt = db.prepare(
                "SELECT id,run_id,attempt_id,boot_epoch,os_proc_id,proc_started_at
                FROM turns WHERE status='running' AND host_instance_id!=?1 ORDER BY id DESC",
            )?;
            let rows = stmt
                .query_map([&self.host], |r| {
                    Ok(Stale {
                        id: r.get(0)?,
                        run: r.get(1)?,
                        attempt: r.get(2)?,
                        boot: r.get(3)?,
                        pid: r.get(4)?,
                        started: r.get(5)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })?;
        for old in &stale {
            let same_boot = old.boot == self.boot;
            if same_boot {
                if let (Some(pid), Some(started)) = (&old.pid, &old.started) {
                    let pid = pid
                        .parse()
                        .map_err(|_| Error::Invalid("invalid supervisor identity"))?;
                    exclude_old_supervisor(&ProcessIdentity{pid,started_at:started.into()})
                        .map_err(|_|Error::Invalid("old supervisor is not yet excluded; recovery did not clear its evidence"))?;
                }
            }
            let released = self.release_present(old.id)?;
            let end = if released {
                EndReason::HostLost
            } else {
                EndReason::NeverStarted
            };
            let writer = if released && same_boot && !hard_stop_excludes_writers() {
                WriterCheck::Possible
            } else {
                WriterCheck::Excluded
            };
            let work = match (&old.run, old.attempt) {
                (Some(run), Some(attempt)) => Some(self.runs.old_work(run, attempt)?),
                _ => None,
            };
            let run = self.settle(old.id, work.as_ref(), end, writer, None)?;
            if let (Some(work), Some(run)) = (&work, &run) {
                if let Some(sha) = &run.result_sha {
                    self.runs.sync_verified(&work.id, sha)?;
                }
            }
        }
        self.runs.store.write(|db|{
            // A persisted admission is not permission to replay a Worker on startup.
            let mut q=db.prepare("SELECT id FROM runs WHERE status='pending' AND current_attempt=0")?;
            let ids=q.query_map([],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
            for id in ids {
                db.execute("UPDATE runs SET status='interrupted',detail='host stopped before dispatch; safe to retry',
                    updated_at=?2 WHERE id=?1",rusqlite::params![id,now(db)?])?;
                change(db,ChangeTopic::Run,&id,&load_run(db,&id)?)?;
            }
            Ok(())
        })?;
        let zombies = self.runs.store.read(|db| {
            let mut q = db.prepare(
                "SELECT id, current_attempt FROM runs r
                 WHERE r.status='running'
                 AND NOT EXISTS (SELECT 1 FROM turns t WHERE t.run_id=r.id AND t.status='running')",
            )?;
            let rows = q
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u32>(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })?;
        for (id, attempt_no) in zombies {
            if attempt_no == 0 {
                let rid = id.clone();
                self.runs.store.write(move |db| {
                    db.execute(
                        "UPDATE runs SET status='interrupted',detail='host stopped before dispatch; safe to retry',
                         updated_at=?2 WHERE id=?1 AND status='running'",
                        rusqlite::params![rid, now(db)?],
                    )?;
                    change(db, ChangeTopic::Run, &rid, &load_run(db, &rid)?)
                })?;
            } else {
                let attempt_id = self.runs.store.read(|db| {
                    Ok(db.query_row(
                        "SELECT id FROM attempts WHERE run_id=?1 AND attempt_no=?2",
                        rusqlite::params![&id, attempt_no],
                        |r| r.get::<_, i64>(0),
                    )?)
                })?;
                let work = self.runs.old_work(&id, attempt_id)?;
                let verdict = self.runs.verdict(&work, WriterCheck::Excluded)?;
                let rid = id.clone();
                let sync_sha = verdict.result_sha.clone();
                self.runs.store.write(move |db| {
                    let stamp = now(db)?;
                    let end_status = if verdict.status == polyclave_proto::RunStatus::Done {
                        "completed"
                    } else {
                        "interrupted"
                    };
                    db.execute(
                        "UPDATE attempts SET status=?2,ended_at=COALESCE(ended_at,?3) WHERE id=?1",
                        rusqlite::params![attempt_id, end_status, stamp],
                    )?;
                    change(
                        db,
                        ChangeTopic::Attempt,
                        &attempt_id.to_string(),
                        &crate::runs::load_attempt(db, attempt_id)?,
                    )?;
                    let accepted = if verdict.status == polyclave_proto::RunStatus::Done {
                        Some(attempt_id)
                    } else {
                        None
                    };
                    let source = accepted.map(|_| "agent");
                    db.execute(
                        "UPDATE runs SET status=?2,detail=?3,result_sha=?4,observed_sha=?5,
                         accepted_attempt_id=?6,result_source=?7,updated_at=?8 WHERE id=?1",
                        rusqlite::params![
                            rid,
                            polyclave_store::enum_value(verdict.status)?,
                            verdict.detail,
                            verdict.result_sha,
                            verdict.observed_sha,
                            accepted,
                            source,
                            stamp,
                        ],
                    )?;
                    if let Some(sha) = &verdict.result_sha {
                        polyclave_store::Store::record_run_memory(
                            db,
                            &rid,
                            &work.run.repo_cwd,
                            "Host verified the result",
                            Some(sha),
                        )?;
                    }
                    change(db, ChangeTopic::Run, &rid, &load_run(db, &rid)?)
                })?;
                if let Some(sha) = sync_sha {
                    self.runs.sync_verified(&work.id, &sha)?;
                }
            }
        }
        let current = self.runs.store.read(|db| {
            let mut q = db.prepare("SELECT id FROM turns WHERE status='running'")?;
            let ids = q
                .query_map([], |r| r.get::<_, i64>(0))?
                .collect::<rusqlite::Result<std::collections::HashSet<_>>>()?;
            Ok(ids)
        })?;
        // Do not erase any release or MCP evidence until every old turn has
        // been classified. Only files with typed, managed names are removed.
        for entry in std::fs::read_dir(self.state.join("tmp"))
            .map_err(|_| Error::Invalid("cannot open temporary directory"))?
        {
            let entry =
                entry.map_err(|_| Error::Invalid("cannot enumerate temporary directory"))?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let id = name
                .strip_prefix("turn-")
                .and_then(|s| s.strip_suffix(".released"))
                .or_else(|| {
                    name.strip_prefix("mcp-turn-")
                        .and_then(|s| s.strip_suffix(".json"))
                });
            if id
                .and_then(|n| n.parse::<i64>().ok())
                .is_some_and(|id| id >= 0 && !current.contains(&id))
            {
                remove(&entry.path())?;
            }
        }
        Ok(stale.len())
    }
}
