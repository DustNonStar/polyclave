//! Crash recovery never restarts a Worker or discards release evidence early.
use crate::{
    runner::{remove, TurnRunner},
    runs::Result,
};
use polyclave_os::{exclude_old_supervisor, hard_stop_excludes_writers, ProcessIdentity};
use polyclave_proto::{EndReason, WriterCheck};
use polyclave_store::Error;

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
            if id.is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())) {
                remove(&entry.path())?;
            }
        }
        Ok(stale.len())
    }
}
