//! SQLite is owned by this crate; mutation jobs are serialized on one writer.
mod hub;
mod memory;
pub use hub::{ApiHistoryRow, HubCompletion, HubMessage, HubTurn, ThreadDelta, TicketIntent};
mod stage2;
pub mod stage3;
mod writer;

pub use memory::{CandidateMemory, Curator, MemoryAction, MemoryFilter};
use rusqlite::{Connection, OptionalExtension};
use serde::{de::DeserializeOwned, Serialize};
pub use stage2::{CouncilCheckpoint, NewCouncil, NewPrGate, NewPrReview, NewProposal, ScmCursor};
pub use writer::{Changes, Store};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the execution slot is busy")]
    BusyError,
    #[error("state conflicts with the request")]
    ConflictError,
    #[error("ticket budget is exhausted")]
    BudgetExceededError,
    #[error("entity not found")]
    NotFound,
    #[error("invalid request: {0}")]
    Invalid(&'static str),
    #[error("the SQLite writer stopped")]
    Closed,
    #[error("invalid data serialization")]
    Serde(#[from] serde_json::Error),
    #[error("storage operation failed")]
    Sql(rusqlite::Error),
}

impl From<rusqlite::Error> for Error {
    fn from(value: rusqlite::Error) -> Self {
        if let rusqlite::Error::SqliteFailure(code, name) = &value {
            if code.extended_code == 2067 {
                let key = name.as_deref().unwrap_or_default();
                if key.contains("turns.session_id") || key.contains("turns.run_id") {
                    return Self::BusyError;
                }
                return Self::ConflictError;
            }
        }
        Self::Sql(value)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub fn now(db: &Connection) -> Result<String> {
    Ok(
        db.query_row("SELECT strftime('%Y-%m-%dT%H:%M:%fZ', 'now')", [], |r| {
            r.get(0)
        })?,
    )
}

pub fn enum_value<T: Serialize>(value: T) -> Result<String> {
    serde_json::to_value(value)?
        .as_str()
        .map(str::to_owned)
        .ok_or(Error::Invalid("enum must serialize as a string"))
}

pub fn decode_enum<T: DeserializeOwned>(value: &str) -> rusqlite::Result<T> {
    decode_json(serde_json::Value::String(value.to_owned()), 0)
}

pub fn decode_json<T: DeserializeOwned>(
    value: serde_json::Value,
    index: usize,
) -> rusqlite::Result<T> {
    serde_json::from_value(value).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, Box::new(e))
    })
}

pub fn parse_json<T: DeserializeOwned>(value: String, index: usize) -> rusqlite::Result<T> {
    serde_json::from_str(&value).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, Box::new(e))
    })
}

pub fn reserve_ticket(db: &Connection, id: &str, repo: &str) -> Result<()> {
    let row: Option<(String, String, u32, u32)> = db
        .query_row(
            "SELECT status,repo_cwd,runs_used,max_runs FROM tickets WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let Some((status, recorded_repo, used, maximum)) = row else {
        return Err(Error::NotFound);
    };
    if status != "active" || repo != recorded_repo {
        return Err(Error::ConflictError);
    }
    if used >= maximum {
        return Err(Error::BudgetExceededError);
    }
    db.execute("UPDATE tickets SET runs_used=runs_used+1 WHERE id=?1", [id])?;
    Ok(())
}

pub fn new_id(prefix: char) -> String {
    let id = uuid::Uuid::new_v4().simple().to_string();
    format!("{prefix}{}", &id[..8])
}
