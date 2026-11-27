//! The repository `origin` snapshot taken when a Run is admitted.
//!
//! Only the post-R1 publish step of the PR gate reads it. Admission, Worker
//! push and R1 verification never consult it: their only target stays the
//! host-owned bare mirror.
use crate::{now, Result, Store};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS run_publish_origins(
    run_id TEXT PRIMARY KEY REFERENCES runs(id),
    urls_json TEXT NOT NULL,
    push_urls_json TEXT NOT NULL,
    captured_at TEXT NOT NULL);";

/// `remote.origin.url` and `remote.origin.pushurl` values of the repository,
/// as read once at admission. Credentials embedded in a URL are removed before
/// the snapshot is built, so none reach the database.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct OriginSnapshot {
    pub urls: Vec<String>,
    pub push_urls: Vec<String>,
}

/// Records the snapshot for `run_id`. The first snapshot wins, so a later
/// change to `origin` can never redirect the publish of an admitted Run.
pub fn insert_run_origin_snapshot(
    db: &Connection,
    run_id: &str,
    snapshot: &OriginSnapshot,
) -> Result<()> {
    let stamp = now(db)?;
    db.execute(
        "INSERT OR IGNORE INTO run_publish_origins(run_id,urls_json,push_urls_json,captured_at)
         VALUES(?1,?2,?3,?4)",
        params![
            run_id,
            serde_json::to_string(&snapshot.urls)?,
            serde_json::to_string(&snapshot.push_urls)?,
            stamp
        ],
    )?;
    Ok(())
}

impl Store {
    /// The `origin` snapshot taken when `run_id` was admitted, or `None` for a
    /// Run admitted before snapshots existed.
    pub fn run_origin_snapshot(&self, run_id: &str) -> Result<Option<OriginSnapshot>> {
        let run_id = run_id.to_owned();
        self.read(move |db| {
            let row: Option<(String, String)> = db
                .query_row(
                    "SELECT urls_json,push_urls_json FROM run_publish_origins WHERE run_id=?1",
                    [&run_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            row.map(|(urls, push_urls)| {
                Ok(OriginSnapshot {
                    urls: serde_json::from_str(&urls)?,
                    push_urls: serde_json::from_str(&push_urls)?,
                })
            })
            .transpose()
        })
    }

    /// Records the snapshot for `run_id` (see [`insert_run_origin_snapshot`]).
    pub fn record_run_origin_snapshot(&self, run_id: &str, snapshot: OriginSnapshot) -> Result<()> {
        let run_id = run_id.to_owned();
        self.write(move |db| insert_run_origin_snapshot(db, &run_id, &snapshot))
    }
}
