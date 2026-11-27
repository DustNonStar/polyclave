//! A second observer cursor for pull-request reviews.
//!
//! GitHub numbers reviews and issue comments in two independent sequences, so
//! one shared cursor makes whichever sequence has the smaller ids invisible
//! forever: a review id of 900 would hide every later comment below 900. The
//! review cursor therefore lives here, in its own table, while
//! `scm_observer_cursors.last_comment_id` keeps tracking issue comments.
use crate::{now, Error, Result, Store};
use rusqlite::{params, Connection, OptionalExtension};

/// Additive schema step, safe to run on every open.
pub(crate) fn upgrade(db: &Connection) -> Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS scm_review_cursors(
           repo_slug      TEXT NOT NULL,
           pr_number      INTEGER NOT NULL,
           last_review_id INTEGER NOT NULL,
           updated_at     TEXT NOT NULL,
           PRIMARY KEY(repo_slug, pr_number));",
    )?;
    Ok(())
}

impl Store {
    /// The highest review id already handled for this pull request.
    pub fn scm_review_cursor(&self, repo_slug: &str, pr_number: u32) -> Result<Option<i64>> {
        let (repo_slug, pr_number) = (repo_slug.to_owned(), pr_number);
        self.read(move |db| {
            Ok(db
                .query_row(
                    "SELECT last_review_id FROM scm_review_cursors
                     WHERE repo_slug=?1 AND pr_number=?2",
                    params![repo_slug, pr_number],
                    |r| r.get(0),
                )
                .optional()?)
        })
    }

    pub fn save_scm_review_cursor(
        &self,
        repo_slug: &str,
        pr_number: u32,
        last_review_id: i64,
    ) -> Result<()> {
        if repo_slug.is_empty() || last_review_id < 0 {
            return Err(Error::Invalid("invalid review cursor"));
        }
        let repo_slug = repo_slug.to_owned();
        self.write(move |db| {
            db.execute(
                "INSERT INTO scm_review_cursors(repo_slug,pr_number,last_review_id,updated_at)
                 VALUES(?1,?2,?3,?4)
                 ON CONFLICT(repo_slug,pr_number) DO UPDATE SET
                    last_review_id=excluded.last_review_id,updated_at=excluded.updated_at",
                params![repo_slug, pr_number, last_review_id, now(db)?],
            )?;
            Ok(())
        })
    }
}
