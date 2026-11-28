//! Additive schema step for the Attempt ledger.
use crate::{Result, Store};
use rusqlite::{Connection, OptionalExtension};

/// Records the commit a revision Attempt was branched from.
///
/// `NULL` means the Attempt started at the Run's immutable baseline, which is
/// what every pre-revision Attempt means. A PR-gate revision records the prior
/// Attempt's R1-verified SHA here, so the new Attempt continues that work while
/// the Run's own `base_sha` (the reviewed baseline) never moves.
///
/// Additive and idempotent, checked against the live schema, so `user_version`
/// stays 10.
pub(crate) fn upgrade(db: &Connection) -> Result<()> {
    let present: Option<i64> = db
        .query_row(
            "SELECT 1 FROM pragma_table_info('attempts') WHERE name='from_sha'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    if present.is_none() {
        db.execute_batch("ALTER TABLE attempts ADD COLUMN from_sha TEXT;")?;
    }
    Ok(())
}

impl Store {
    /// The commit this Attempt was branched from: the revision base, or `None`
    /// for an Attempt that started at the Run's baseline.
    pub fn attempt_from_sha(&self, attempt_id: i64) -> Result<Option<String>> {
        self.read(move |db| {
            Ok(db.query_row(
                "SELECT from_sha FROM attempts WHERE id=?1",
                [attempt_id],
                |r| r.get::<_, Option<String>>(0),
            )?)
        })
    }
}
