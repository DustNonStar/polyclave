//! Primary-side ledger of mesh `worker_run` dispatches.
//!
//! A remote Run is charged to the primary Ticket that dispatched it. The key is
//! (Hub session, `request_id`): the first dispatch reserves one `runs_used` on
//! the Ticket and writes the row in the same `BEGIN IMMEDIATE` transaction; a
//! resend with the same key finds the row and is not charged again.
//!
//! Refund rule: a reservation is never refunded. When the remote call fails
//! after the reservation, the outcome is recorded on the row and the Hub
//! resends with the same `request_id`, which costs nothing and reaches the Run
//! the receiver may already have admitted (the receiver is idempotent on
//! `request_id`). A refund could not tell "never arrived" from "arrived, reply
//! lost", so refunding would let a retry admit a Run the Ticket never paid for.
use crate::{hub::publish_ticket, now, reserve_ticket, Error, Result, Store};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;

/// Additive schema step, safe to run on every open.
pub(crate) fn upgrade(db: &Connection) -> Result<()> {
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS remote_dispatches(
           hub_session_id TEXT NOT NULL REFERENCES sessions(id),
           request_id     TEXT NOT NULL,
           node_id        TEXT NOT NULL,
           ticket_id      TEXT NOT NULL REFERENCES tickets(id),
           remote_run_id  TEXT,
           outcome        TEXT NOT NULL CHECK (outcome IN
                            ('reserved','admitted','at_limit','refused','unreachable')),
           detail         TEXT,
           created_at     TEXT NOT NULL,
           updated_at     TEXT NOT NULL,
           PRIMARY KEY(hub_session_id, request_id));",
    )?;
    Ok(())
}

/// What happened to a dispatch after its reservation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteOutcome {
    /// Charged; no answer from the receiver recorded yet.
    Reserved,
    /// The receiver returned a Run (new or the one it already had).
    Admitted,
    /// The receiver was at its per-node concurrency limit (HTTP 429).
    AtLimit,
    /// The receiver answered with another error status.
    Refused,
    /// No usable answer; the receiver may or may not have admitted the Run.
    Unreachable,
}

impl RemoteOutcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Reserved => "reserved",
            Self::Admitted => "admitted",
            Self::AtLimit => "at_limit",
            Self::Refused => "refused",
            Self::Unreachable => "unreachable",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RemoteDispatch {
    pub hub_session_id: String,
    pub request_id: String,
    pub node_id: String,
    pub ticket_id: String,
    pub remote_run_id: Option<String>,
    pub outcome: String,
    pub detail: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Result of [`Store::charge_remote_dispatch`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RemoteCharge {
    /// First dispatch for this key: one run was reserved on the Ticket.
    Charged(RemoteDispatch),
    /// A resend: the earlier row was found and nothing was charged.
    Existing(RemoteDispatch),
}

fn load(db: &Connection, hub: &str, request_id: &str) -> Result<Option<RemoteDispatch>> {
    Ok(db
        .query_row(
            "SELECT hub_session_id,request_id,node_id,ticket_id,remote_run_id,outcome,detail,
                    created_at,updated_at
             FROM remote_dispatches WHERE hub_session_id=?1 AND request_id=?2",
            params![hub, request_id],
            |r| {
                Ok(RemoteDispatch {
                    hub_session_id: r.get(0)?,
                    request_id: r.get(1)?,
                    node_id: r.get(2)?,
                    ticket_id: r.get(3)?,
                    remote_run_id: r.get(4)?,
                    outcome: r.get(5)?,
                    detail: r.get(6)?,
                    created_at: r.get(7)?,
                    updated_at: r.get(8)?,
                })
            },
        )
        .optional()?)
}

impl Store {
    pub fn remote_dispatch(&self, hub: &str, request_id: &str) -> Result<Option<RemoteDispatch>> {
        self.read(|db| load(db, hub, request_id))
    }

    /// Charges a mesh `worker_run` to `ticket` once per (`hub`, `request_id`).
    ///
    /// In one write transaction: a row for the key means a resend, returned as
    /// [`RemoteCharge::Existing`] without a charge, provided it names the same
    /// node and Ticket and the Ticket is not closed. Otherwise the Ticket must
    /// belong to `hub`, be `active` and be recorded for `repo`; one run is
    /// reserved with [`reserve_ticket`] and the row is written. An exhausted
    /// Ticket moves to `blocked_on_user` and the call returns
    /// [`Error::BudgetExceededError`].
    pub fn charge_remote_dispatch(
        &self,
        hub: &str,
        request_id: &str,
        node_id: &str,
        ticket: &str,
        repo: &str,
    ) -> Result<RemoteCharge> {
        if request_id.trim().is_empty() || request_id.len() > 256 {
            return Err(Error::Invalid("missing or oversized request_id"));
        }
        let (hub, request_id, node_id, ticket, repo) = (
            hub.to_owned(),
            request_id.to_owned(),
            node_id.to_owned(),
            ticket.to_owned(),
            repo.to_owned(),
        );
        let charged = self.write(move |db| {
            if let Some(row) = load(db, &hub, &request_id)? {
                let status: String = db.query_row(
                    "SELECT status FROM tickets WHERE id=?1",
                    [&row.ticket_id],
                    |r| r.get(0),
                )?;
                if row.node_id != node_id
                    || row.ticket_id != ticket
                    || matches!(status.as_str(), "done" | "cancelled")
                {
                    return Err(Error::ConflictError);
                }
                return Ok(Some(RemoteCharge::Existing(row)));
            }
            let owner: Option<String> = db
                .query_row(
                    "SELECT hub_session_id FROM tickets WHERE id=?1",
                    [&ticket],
                    |r| r.get(0),
                )
                .optional()?;
            match owner {
                None => return Err(Error::NotFound),
                Some(owner) if owner != hub => return Err(Error::ConflictError),
                Some(_) => {}
            }
            match reserve_ticket(db, &ticket, &repo) {
                Ok(()) => {}
                Err(Error::BudgetExceededError) => {
                    // Same as dispatch_task: an exhausted Ticket waits on the user.
                    db.execute(
                        "UPDATE tickets SET status='blocked_on_user',updated_at=?2
                         WHERE id=?1 AND status='active'",
                        params![ticket, now(db)?],
                    )?;
                    publish_ticket(db, &ticket)?;
                    return Ok(None);
                }
                Err(err) => return Err(err),
            }
            let stamp = now(db)?;
            db.execute(
                "INSERT INTO remote_dispatches(hub_session_id,request_id,node_id,ticket_id,outcome,
                    created_at,updated_at) VALUES(?1,?2,?3,?4,'reserved',?5,?5)",
                params![hub, request_id, node_id, ticket, stamp],
            )?;
            db.execute(
                "UPDATE tickets SET updated_at=?2 WHERE id=?1",
                params![ticket, stamp],
            )?;
            publish_ticket(db, &ticket)?;
            let row = load(db, &hub, &request_id)?.ok_or(Error::NotFound)?;
            Ok(Some(RemoteCharge::Charged(row)))
        })?;
        charged.ok_or(Error::BudgetExceededError)
    }

    /// Records what the receiver answered. The reservation is kept whatever
    /// the outcome; a resend with the same key is never charged.
    pub fn record_remote_dispatch_outcome(
        &self,
        hub: &str,
        request_id: &str,
        outcome: RemoteOutcome,
        remote_run_id: Option<String>,
        detail: Option<String>,
    ) -> Result<RemoteDispatch> {
        let (hub, request_id) = (hub.to_owned(), request_id.to_owned());
        self.write(move |db| {
            let changed = db.execute(
                "UPDATE remote_dispatches SET outcome=?3,
                    remote_run_id=COALESCE(remote_run_id,?4),detail=?5,updated_at=?6
                 WHERE hub_session_id=?1 AND request_id=?2",
                params![
                    hub,
                    request_id,
                    outcome.as_str(),
                    remote_run_id,
                    detail,
                    now(db)?
                ],
            )?;
            if changed == 0 {
                return Err(Error::NotFound);
            }
            load(db, &hub, &request_id)?.ok_or(Error::NotFound)
        })
    }
}
