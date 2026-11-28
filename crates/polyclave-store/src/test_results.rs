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
pub(crate) fn upgrade(db: &Connection) -> Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS test_results(
           run_id      TEXT NOT NULL REFERENCES runs(id),
           sha         TEXT NOT NULL,
           source      TEXT NOT NULL CHECK (source IN ('worker_claim','host_run','ci')),
           name        TEXT NOT NULL DEFAULT '',
           status      TEXT NOT NULL CHECK (status IN ('passed','failed','pending')),
           summary     TEXT NOT NULL,
           recorded_at TEXT NOT NULL,
           PRIMARY KEY(run_id, sha, source, name));
         CREATE INDEX IF NOT EXISTS test_results_by_sha ON test_results(run_id, sha);",
    )?;
    Ok(())
}

/// One result to record. `name` is the check-run name for `ci` and empty for a claim.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NewTestResult {
    pub run_id: String,
    pub sha: String,
    pub source: TestResultSource,
    pub name: String,
    pub status: TestStatus,
    pub summary: String,
}

fn is_full_oid(sha: &str) -> bool {
    sha.len() == 40
        && sha
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

/// Records or updates one result on an existing transaction.
///
/// The key is `(run, sha, source, name)`, so a check that moves from queued to
/// completed replaces its own row and a later push (a different SHA) can never
/// touch these rows.
pub fn record_test_result(db: &Connection, row: &NewTestResult) -> Result<()> {
    if !is_full_oid(&row.sha) {
        return Err(Error::Invalid("test result sha must be a full commit id"));
    }
    if row.name.len() > 200 || row.summary.trim().is_empty() || row.summary.len() > 2000 {
        return Err(Error::Invalid("invalid test result"));
    }
    let known: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runs WHERE id=?1)",
        [&row.run_id],
        |r| r.get(0),
    )?;
    if !known {
        return Err(Error::NotFound);
    }
    db.execute(
        "INSERT INTO test_results(run_id,sha,source,name,status,summary,recorded_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7)
         ON CONFLICT(run_id,sha,source,name) DO UPDATE SET
            status=excluded.status,summary=excluded.summary,recorded_at=excluded.recorded_at",
        params![
            row.run_id,
            row.sha,
            row.source.as_str(),
            row.name,
            row.status.as_str(),
            row.summary,
            now(db)?
        ],
    )?;
    Ok(())
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
    /// Records results in one write transaction.
    pub fn record_test_results(&self, rows: Vec<NewTestResult>) -> Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        self.write(move |db| {
            for row in &rows {
                record_test_result(db, row)?;
            }
            Ok(())
        })
    }

    pub fn record_test_result(&self, row: NewTestResult) -> Result<()> {
        self.record_test_results(vec![row])
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
