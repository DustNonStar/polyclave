//! Receiver-side concurrency cap for Runs admitted over the Agent Mesh.
//!
//! Each `mesh_nodes` row carries `max_concurrent_runs` (default 3, 1..=16). A
//! Run admitted for a node is recorded in `mesh_admissions` in the same
//! transaction that writes the Run, so the count and the admission cannot
//! drift apart. A Run occupies a slot while it is `pending` or `running`, the
//! states in which a Worker is queued or spends vendor quota. `needs_review`,
//! `interrupted`, `done` and `failed` hold no Worker process and free the slot.
use crate::{now, Error, Result, Store};
use polyclave_proto::MeshNodeLimitDto;
use rusqlite::{params, Connection, OptionalExtension};

/// Default number of concurrent mesh Runs one node may have on this host.
pub const MESH_CONCURRENCY_DEFAULT: u32 = 3;
/// Highest value the owner may choose.
pub const MESH_CONCURRENCY_MAX: u32 = 16;

/// Additive schema step, safe to run on every open: existing nodes take the
/// default of 3. It checks the live schema rather than `user_version`.
pub(crate) fn upgrade(db: &Connection) -> Result<()> {
    let has_column: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('mesh_nodes') WHERE name='max_concurrent_runs')",
        [],
        |r| r.get(0),
    )?;
    if !has_column {
        db.execute_batch(
            "ALTER TABLE mesh_nodes ADD COLUMN max_concurrent_runs INTEGER NOT NULL DEFAULT 3
               CHECK (max_concurrent_runs BETWEEN 1 AND 16);",
        )?;
    }
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS mesh_admissions(
           run_id     TEXT PRIMARY KEY REFERENCES runs(id),
           node_id    TEXT NOT NULL,
           created_at TEXT NOT NULL);
         CREATE INDEX IF NOT EXISTS mesh_admissions_node ON mesh_admissions(node_id);",
    )?;
    Ok(())
}

/// Run states that hold a concurrency slot.
const ACTIVE_SQL: &str = "SELECT COUNT(*) FROM mesh_admissions a JOIN runs r ON r.id=a.run_id
     WHERE a.node_id=?1 AND r.status IN ('pending','running')";

fn read_limit(db: &Connection, node_id: &str) -> Result<MeshNodeLimitDto> {
    let limit: u32 = db
        .query_row(
            "SELECT max_concurrent_runs FROM mesh_nodes WHERE id=?1",
            [node_id],
            |r| r.get(0),
        )
        .optional()?
        .ok_or(Error::NotFound)?;
    let active: u32 = db.query_row(ACTIVE_SQL, [node_id], |r| r.get(0))?;
    Ok(MeshNodeLimitDto {
        node_id: node_id.to_owned(),
        max_concurrent_runs: limit,
        active_runs: active,
    })
}

/// Refuses with [`Error::ConcurrencyLimit`] when the node already has its
/// maximum number of active mesh Runs. Call it inside the admission
/// transaction (or under the admission gate) so the answer stays true until
/// the Run is written.
pub fn check_mesh_capacity(db: &Connection, node_id: &str) -> Result<()> {
    let state = read_limit(db, node_id)?;
    if state.active_runs >= state.max_concurrent_runs {
        return Err(Error::ConcurrencyLimit(state.max_concurrent_runs));
    }
    Ok(())
}

/// Checks capacity and records `run_id` as admitted for `node_id`. Must run in
/// the same write transaction that inserted the Run, so a refusal rolls the
/// admission back.
pub fn record_mesh_admission(db: &Connection, node_id: &str, run_id: &str) -> Result<()> {
    check_mesh_capacity(db, node_id)?;
    db.execute(
        "INSERT INTO mesh_admissions(run_id,node_id,created_at) VALUES(?1,?2,?3)",
        params![run_id, node_id, now(db)?],
    )?;
    Ok(())
}

impl Store {
    /// The concurrency setting and current active count of every mesh node.
    pub fn mesh_node_limits(&self) -> Result<Vec<MeshNodeLimitDto>> {
        self.read(|db| {
            let ids = db
                .prepare("SELECT id FROM mesh_nodes ORDER BY rowid DESC")?
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ids.iter().map(|id| read_limit(db, id)).collect()
        })
    }

    pub fn mesh_node_limit(&self, node_id: &str) -> Result<MeshNodeLimitDto> {
        let id = node_id.to_owned();
        self.read(move |db| read_limit(db, &id))
    }

    /// Sets the per-node concurrency cap. It applies to the next admission;
    /// Runs already admitted are never stopped by lowering it.
    pub fn set_mesh_node_concurrency(
        &self,
        node_id: &str,
        max_concurrent_runs: u32,
    ) -> Result<MeshNodeLimitDto> {
        if !(1..=MESH_CONCURRENCY_MAX).contains(&max_concurrent_runs) {
            return Err(Error::Invalid(
                "max_concurrent_runs must be between 1 and 16",
            ));
        }
        let id = node_id.to_owned();
        self.write(move |db| {
            let changed = db.execute(
                "UPDATE mesh_nodes SET max_concurrent_runs=?2 WHERE id=?1",
                params![id, max_concurrent_runs],
            )?;
            if changed == 0 {
                return Err(Error::NotFound);
            }
            read_limit(db, &id)
        })
    }
}
