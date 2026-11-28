//! Test results recorded per `(run, sha, source)`.
//!
//! The approval card may only show a result for the exact SHA it is about, so
//! every row is keyed by SHA and read back with an equality filter. `ci` rows are
//! written by the SCM observer from check-run conclusions; `worker_claim` rows are
//! the Worker's own claim, submitted with `submit_result`, and are always shown
//! labelled as a claim. `host_run` is reserved for a host-run test engine that
//! does not exist yet.
use crate::{now, Error, Result, Store};
use polyclave_proto::{TestResultDto, TestResultSource, TestStatus};
use rusqlite::{params, Connection, OptionalExtension};

/// Additive schema step, safe to run on every open.
///
/// The key is `(run_id, sha, source, name, check_id)`. `check_id` is GitHub's
/// stable check-run id for `ci` rows and empty for a Worker's claim, so two
/// checks that share a name but come from different apps or suites keep their
/// own rows instead of overwriting each other. A ledger written before the
/// column existed is rebuilt in place with empty ids, which leaves every old
/// row's meaning unchanged.
pub(crate) fn upgrade(db: &Connection) -> Result<()> {
    let exists: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('test_results'))",
        [],
        |r| r.get(0),
    )?;
    let has_check_id: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM pragma_table_info('test_results') WHERE name='check_id')",
        [],
        |r| r.get(0),
    )?;
    if exists && !has_check_id {
        db.execute_batch(
            "ALTER TABLE test_results RENAME TO test_results_before_check_id;
             CREATE TABLE test_results(
               run_id      TEXT NOT NULL REFERENCES runs(id),
               sha         TEXT NOT NULL,
               source      TEXT NOT NULL CHECK (source IN ('worker_claim','host_run','ci')),
               name        TEXT NOT NULL DEFAULT '',
               check_id    TEXT NOT NULL DEFAULT '',
               status      TEXT NOT NULL CHECK (status IN ('passed','failed','pending')),
               summary     TEXT NOT NULL,
               recorded_at TEXT NOT NULL,
               PRIMARY KEY(run_id, sha, source, name, check_id));
             INSERT INTO test_results(run_id,sha,source,name,check_id,status,summary,recorded_at)
               SELECT run_id,sha,source,name,'',status,summary,recorded_at
               FROM test_results_before_check_id;
             DROP TABLE test_results_before_check_id;",
        )?;
    }
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS test_results(
           run_id      TEXT NOT NULL REFERENCES runs(id),
           sha         TEXT NOT NULL,
           source      TEXT NOT NULL CHECK (source IN ('worker_claim','host_run','ci')),
           name        TEXT NOT NULL DEFAULT '',
           check_id    TEXT NOT NULL DEFAULT '',
           status      TEXT NOT NULL CHECK (status IN ('passed','failed','pending')),
           summary     TEXT NOT NULL,
           recorded_at TEXT NOT NULL,
           PRIMARY KEY(run_id, sha, source, name, check_id));
         CREATE INDEX IF NOT EXISTS test_results_by_sha ON test_results(run_id, sha);",
    )?;
    Ok(())
}

/// One result to record. `name` is the check-run name for `ci` and empty for a
/// claim; `check_id` is GitHub's stable check-run id (empty for a claim).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewTestResult {
    pub run_id: String,
    pub sha: String,
    pub source: TestResultSource,
    pub name: String,
    pub check_id: String,
    pub status: TestStatus,
    pub summary: String,
}

/// A row that really changed, so a caller can tell the UI about it. `from` is
/// `None` for a first sighting.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub struct TestResultChange {
    pub source: TestResultSource,
    pub name: String,
    pub check_id: String,
    pub from: Option<TestStatus>,
    pub to: TestStatus,
}

fn is_full_oid(sha: &str) -> bool {
    sha.len() == 40
        && sha
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

/// Records or updates one result on an existing transaction and reports the
/// change when the status really moved.
///
/// The key is `(run, sha, source, name, check_id)`, so a check that moves from
/// queued to completed replaces its own row, two same-named checks from
/// different sources keep separate rows, and a later push (a different SHA) can
/// never touch these rows.
pub fn record_test_result(
    db: &Connection,
    row: &NewTestResult,
) -> Result<Option<TestResultChange>> {
    if !is_full_oid(&row.sha) {
        return Err(Error::Invalid("test result sha must be a full commit id"));
    }
    if row.name.len() > 200 || row.summary.trim().is_empty() || row.summary.len() > 2000 {
        return Err(Error::Invalid("invalid test result"));
    }
    if row.check_id.len() > 64
        || !row
            .check_id
            .bytes()
            .all(|c| c.is_ascii_digit() || c.is_ascii_lowercase() || c == b'-')
    {
        return Err(Error::Invalid("invalid check identifier"));
    }
    let known: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runs WHERE id=?1)",
        [&row.run_id],
        |r| r.get(0),
    )?;
    if !known {
        return Err(Error::NotFound);
    }
    let previous: Option<String> = db
        .query_row(
            "SELECT status FROM test_results
             WHERE run_id=?1 AND sha=?2 AND source=?3 AND name=?4 AND check_id=?5",
            params![
                row.run_id,
                row.sha,
                row.source.as_str(),
                row.name,
                row.check_id
            ],
            |r| r.get(0),
        )
        .optional()?;
    let from = previous.as_deref().and_then(TestStatus::parse);
    db.execute(
        "INSERT INTO test_results(run_id,sha,source,name,check_id,status,summary,recorded_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
         ON CONFLICT(run_id,sha,source,name,check_id) DO UPDATE SET
            status=excluded.status,summary=excluded.summary,recorded_at=excluded.recorded_at",
        params![
            row.run_id,
            row.sha,
            row.source.as_str(),
            row.name,
            row.check_id,
            row.status.as_str(),
            row.summary,
            now(db)?
        ],
    )?;
    if from == Some(row.status) {
        return Ok(None);
    }
    Ok(Some(TestResultChange {
        source: row.source,
        name: row.name.clone(),
        check_id: row.check_id.clone(),
        from,
        to: row.status,
    }))
}

/// Rows for exactly `sha`, claims last so a host check leads.
pub fn test_results_for_sha(db: &Connection, run: &str, sha: &str) -> Result<Vec<TestResultDto>> {
    let mut q = db.prepare(
        "SELECT sha,source,status,name,summary,recorded_at FROM test_results
         WHERE run_id=?1 AND sha=?2
         ORDER BY (source='worker_claim'), name, recorded_at",
    )?;
    let rows = q.query_map(params![run, sha], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, String>(5)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (sha, source, status, name, summary, recorded_at) = row?;
        out.push(TestResultDto {
            sha,
            source: TestResultSource::parse(&source)
                .ok_or(Error::Invalid("unknown test result source"))?,
            status: TestStatus::parse(&status).ok_or(Error::Invalid("unknown test status"))?,
            name,
            summary,
            recorded_at,
        });
    }
    Ok(out)
}

impl Store {
    /// Records results in one write transaction and returns the rows whose status
    /// actually moved, so the caller can publish a change event for them.
    pub fn record_test_results(&self, rows: Vec<NewTestResult>) -> Result<Vec<TestResultChange>> {
        if rows.is_empty() {
            return Ok(Vec::new());
        }
        self.write(move |db| {
            let mut changes = Vec::new();
            for row in &rows {
                changes.extend(record_test_result(db, row)?);
            }
            Ok(changes)
        })
    }

    pub fn record_test_result(&self, row: NewTestResult) -> Result<Vec<TestResultChange>> {
        self.record_test_results(vec![row])
    }

    /// Removes exactly one recorded row. Used when a partial check-run list is
    /// replaced by a complete one, so the "incomplete" marker disappears.
    pub fn clear_test_result(
        &self,
        run: &str,
        sha: &str,
        source: TestResultSource,
        name: &str,
        check_id: &str,
    ) -> Result<()> {
        let (run, sha, name, check_id) = (
            run.to_owned(),
            sha.to_owned(),
            name.to_owned(),
            check_id.to_owned(),
        );
        self.write(move |db| {
            db.execute(
                "DELETE FROM test_results
                 WHERE run_id=?1 AND sha=?2 AND source=?3 AND name=?4 AND check_id=?5",
                params![run, sha, source.as_str(), name, check_id],
            )?;
            Ok(())
        })
    }

    pub fn test_results_for_sha(&self, run: &str, sha: &str) -> Result<Vec<TestResultDto>> {
        let (run, sha) = (run.to_owned(), sha.to_owned());
        self.read(move |db| test_results_for_sha(db, &run, &sha))
    }

    /// The SHA test results are recorded for: the R1-verified `result_sha`, or
    /// the `observed_sha` while nothing is verified. An `observed_sha` row is a
    /// reference only; nothing may call it verified.
    pub fn run_test_sha(&self, run: &str) -> Result<Option<String>> {
        let run = run.to_owned();
        self.read(move |db| {
            Ok(db
                .query_row(
                    "SELECT COALESCE(result_sha, observed_sha) FROM runs WHERE id=?1",
                    [&run],
                    |r| r.get::<_, Option<String>>(0),
                )
                .optional()?
                .flatten())
        })
    }
}
