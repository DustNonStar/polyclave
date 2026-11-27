//! Stage 2 durable state: blind councils, PR gates, SCM observer cursors and
//! the global Pause flag. Every mutation runs on the single writer and appends
//! its public projection to `change_log` in the same transaction.
use crate::{decode_enum, enum_value, new_id, now, parse_json, Error, Result, Store};
use polyclave_proto::{
    ChangeTopic, CouncilDto, CouncilLevel, CouncilProposalDto, CouncilStatus, PauseStateDto,
    PlanningPreference, PrGateDto, PrGateStatus, PrReviewDto, ProposalPayload, ReviewVerdict,
    Vendor,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

/// Additive, idempotent Stage 2 schema.
pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS councils (
  id                      TEXT PRIMARY KEY,
  ticket_id               TEXT REFERENCES tickets(id),
  repo_cwd                TEXT NOT NULL,
  level                   TEXT NOT NULL CHECK (level IN ('direction', 'detail')),
  status                  TEXT NOT NULL CHECK (status IN ('running', 'awaiting_user', 'decided', 'cancelled', 'failed')),
  prompt                  TEXT NOT NULL,
  preference              TEXT NOT NULL CHECK (preference IN ('thorough', 'balanced', 'conservative')),
  escalation_reasons_json TEXT NOT NULL DEFAULT '[]',
  chosen_label            TEXT,
  decision_note           TEXT,
  created_at              TEXT NOT NULL,
  decided_at              TEXT,
  CHECK ((status = 'decided') = (chosen_label IS NOT NULL AND decided_at IS NOT NULL))
);
CREATE TABLE IF NOT EXISTS council_proposals (
  id            TEXT PRIMARY KEY,
  council_id    TEXT NOT NULL REFERENCES councils(id),
  label         TEXT NOT NULL,
  vendor        TEXT NOT NULL CHECK (vendor IN ('codex', 'claude', 'agy', 'dsh')),
  model         TEXT NOT NULL,
  payload_json  TEXT NOT NULL,
  input_hash    TEXT NOT NULL,
  created_at    TEXT NOT NULL,
  UNIQUE (council_id, label)
);
CREATE TABLE IF NOT EXISTS council_checkpoints (
  council_id    TEXT NOT NULL REFERENCES councils(id),
  step_key      TEXT NOT NULL,
  input_hash    TEXT NOT NULL,
  output_json   TEXT NOT NULL,
  created_at    TEXT NOT NULL,
  PRIMARY KEY (council_id, step_key)
);
CREATE TABLE IF NOT EXISTS pr_gates (
  id                    TEXT PRIMARY KEY,
  run_id                TEXT NOT NULL REFERENCES runs(id),
  ticket_id             TEXT NOT NULL REFERENCES tickets(id),
  repo_slug             TEXT NOT NULL,
  branch                TEXT NOT NULL,
  base_sha              TEXT NOT NULL,
  head_sha              TEXT NOT NULL,
  pr_number             INTEGER,
  pr_url                TEXT,
  status                TEXT NOT NULL CHECK (status IN ('pending', 'scope_violation', 'reviewing', 'revise_requested',
                          'approved', 'auto_merge_armed', 'merged', 'escalated', 'paused', 'failed')),
  touched_paths_json    TEXT NOT NULL DEFAULT '[]',
  scope_violations_json TEXT NOT NULL DEFAULT '[]',
  revise_rounds         INTEGER NOT NULL DEFAULT 0 CHECK (revise_rounds >= 0),
  auto_merge_armed      INTEGER NOT NULL DEFAULT 0 CHECK (auto_merge_armed IN (0, 1)),
  created_at            TEXT NOT NULL,
  updated_at            TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS pr_gates_armed ON pr_gates(auto_merge_armed) WHERE auto_merge_armed = 1;
CREATE TABLE IF NOT EXISTS pr_reviews (
  id                TEXT PRIMARY KEY,
  pr_gate_id        TEXT NOT NULL REFERENCES pr_gates(id),
  reviewer_vendor   TEXT NOT NULL CHECK (reviewer_vendor IN ('codex', 'claude', 'agy', 'dsh')),
  reviewer_model    TEXT NOT NULL,
  reviewed_sha      TEXT NOT NULL,
  verdict           TEXT NOT NULL CHECK (verdict IN ('approve', 'revise')),
  comments          TEXT NOT NULL,
  github_review_id  INTEGER,
  created_at        TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS scm_observer_cursors (
  repo_slug        TEXT NOT NULL,
  pr_number        INTEGER NOT NULL,
  last_event_etag  TEXT,
  last_check_sha   TEXT,
  last_comment_id  INTEGER,
  updated_at       TEXT NOT NULL,
  PRIMARY KEY (repo_slug, pr_number)
);
CREATE TABLE IF NOT EXISTS host_pause_state (
  singleton   INTEGER PRIMARY KEY CHECK (singleton = 1),
  paused      INTEGER NOT NULL CHECK (paused IN (0, 1)),
  reason      TEXT,
  updated_at  TEXT NOT NULL
);
INSERT OR IGNORE INTO host_pause_state(singleton,paused,reason,updated_at)
  VALUES(1,0,NULL,strftime('%Y-%m-%dT%H:%M:%fZ','now'));
";

pub struct NewCouncil {
    pub ticket_id: Option<String>,
    pub repo_cwd: String,
    pub level: CouncilLevel,
    pub preference: PlanningPreference,
    pub prompt: String,
}

/// Host-recorded attribution for one proposal. `vendor`/`model` come from the
/// host launch record and are never exposed until the council is decided.
pub struct NewProposal {
    pub label: String,
    pub vendor: Vendor,
    pub model: String,
    pub payload: ProposalPayload,
    pub input_hash: String,
}

pub struct NewPrGate {
    pub run_id: String,
    pub repo_slug: String,
    pub branch: String,
    pub base_sha: String,
    pub head_sha: String,
    pub touched_paths: Vec<String>,
}

pub struct NewPrReview {
    pub reviewer_vendor: Vendor,
    pub reviewer_model: String,
    pub reviewed_sha: String,
    pub verdict: ReviewVerdict,
    pub comments: String,
    pub github_review_id: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScmCursor {
    pub repo_slug: String,
    pub pr_number: u32,
    pub last_event_etag: Option<String>,
    pub last_check_sha: Option<String>,
    pub last_comment_id: Option<i64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CouncilCheckpoint {
    pub input_hash: String,
    pub output: Value,
}

fn ensure(cond: bool, message: &'static str) -> Result<()> {
    if cond {
        Ok(())
    } else {
        Err(Error::Invalid(message))
    }
}

fn load_council(db: &Connection, id: &str) -> Result<CouncilDto> {
    let row = db
        .query_row(
            "SELECT id,ticket_id,repo_cwd,level,status,prompt,escalation_reasons_json,chosen_label,
                    decision_note,created_at,decided_at FROM councils WHERE id=?1",
            [id],
            |r| {
                Ok(CouncilDto {
                    id: r.get(0)?,
                    ticket_id: r.get(1)?,
                    repo_cwd: r.get(2)?,
                    level: decode_enum(&r.get::<_, String>(3)?)?,
                    status: decode_enum(&r.get::<_, String>(4)?)?,
                    prompt: r.get(5)?,
                    escalation_reasons: parse_json(r.get(6)?, 6)?,
                    chosen_label: r.get(7)?,
                    decision_note: r.get(8)?,
                    proposals: Vec::new(),
                    created_at: r.get(9)?,
                    decided_at: r.get(10)?,
                })
            },
        )
        .optional()?;
    let Some(mut council) = row else {
        return Err(Error::NotFound);
    };
    // Blind projection: attribution is stripped here, in Rust, unless decided.
    let reveal = council.status == CouncilStatus::Decided;
    let mut stmt = db.prepare(
        "SELECT label,payload_json,vendor,model FROM council_proposals
         WHERE council_id=?1 ORDER BY rowid",
    )?;
    let mut rows = stmt.query([id])?;
    while let Some(r) = rows.next()? {
        council.proposals.push(CouncilProposalDto {
            label: r.get(0)?,
            payload: parse_json(r.get(1)?, 1)?,
            vendor: if reveal {
                Some(decode_enum(&r.get::<_, String>(2)?)?)
            } else {
                None
            },
            model: if reveal { Some(r.get(3)?) } else { None },
        });
    }
    Ok(council)
}

fn publish_council(db: &Connection, id: &str) -> Result<CouncilDto> {
    let council = load_council(db, id)?;
    Store::append_change(
        db,
        ChangeTopic::Council,
        id,
        &serde_json::to_value(&council)?,
    )?;
    Ok(council)
}

fn council_status(db: &Connection, id: &str) -> Result<CouncilStatus> {
    let status: Option<String> = db
        .query_row("SELECT status FROM councils WHERE id=?1", [id], |r| {
            r.get(0)
        })
        .optional()?;
    Ok(decode_enum(&status.ok_or(Error::NotFound)?)?)
}

fn open_council(status: CouncilStatus) -> bool {
    matches!(status, CouncilStatus::Running | CouncilStatus::AwaitingUser)
}

fn load_pr_gate(db: &Connection, id: &str) -> Result<PrGateDto> {
    let row = db
        .query_row(
            "SELECT id,run_id,ticket_id,repo_slug,branch,base_sha,head_sha,pr_number,pr_url,status,
                    scope_violations_json,revise_rounds,auto_merge_armed,updated_at
             FROM pr_gates WHERE id=?1",
            [id],
            |r| {
                Ok(PrGateDto {
                    id: r.get(0)?,
                    run_id: r.get(1)?,
                    ticket_id: r.get(2)?,
                    repo_slug: r.get(3)?,
                    branch: r.get(4)?,
                    base_sha: r.get(5)?,
                    head_sha: r.get(6)?,
                    pr_number: r.get(7)?,
                    pr_url: r.get(8)?,
                    status: decode_enum(&r.get::<_, String>(9)?)?,
                    scope_violations: parse_json(r.get(10)?, 10)?,
                    revise_rounds: r.get(11)?,
                    auto_merge_armed: r.get(12)?,
                    reviews: Vec::new(),
                    updated_at: r.get(13)?,
                })
            },
        )
        .optional()?;
    let Some(mut gate) = row else {
        return Err(Error::NotFound);
    };
    let mut stmt = db.prepare(
        "SELECT id,pr_gate_id,reviewer_vendor,reviewer_model,reviewed_sha,verdict,comments,
                github_review_id,created_at FROM pr_reviews WHERE pr_gate_id=?1 ORDER BY rowid",
    )?;
    let mut rows = stmt.query([id])?;
    while let Some(r) = rows.next()? {
        gate.reviews.push(PrReviewDto {
            id: r.get(0)?,
            pr_gate_id: r.get(1)?,
            reviewer_vendor: decode_enum(&r.get::<_, String>(2)?)?,
            reviewer_model: r.get(3)?,
            reviewed_sha: r.get(4)?,
            verdict: decode_enum(&r.get::<_, String>(5)?)?,
            comments: r.get(6)?,
            github_review_id: r.get(7)?,
            created_at: r.get(8)?,
        });
    }
    Ok(gate)
}

fn publish_pr_gate(db: &Connection, id: &str) -> Result<PrGateDto> {
    db.execute(
        "UPDATE pr_gates SET updated_at=?2 WHERE id=?1",
        params![id, now(db)?],
    )?;
    let gate = load_pr_gate(db, id)?;
    Store::append_change(db, ChangeTopic::PrGate, id, &serde_json::to_value(&gate)?)?;
    Ok(gate)
}

fn gate_state(db: &Connection, id: &str) -> Result<(PrGateStatus, bool, String)> {
    let row: Option<(String, bool, String)> = db
        .query_row(
            "SELECT status,auto_merge_armed,head_sha FROM pr_gates WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let (status, armed, head) = row.ok_or(Error::NotFound)?;
    Ok((decode_enum(&status)?, armed, head))
}

fn load_pause(db: &Connection) -> Result<PauseStateDto> {
    Ok(db.query_row(
        "SELECT paused,reason,updated_at FROM host_pause_state WHERE singleton=1",
        [],
        |r| {
            Ok(PauseStateDto {
                paused: r.get(0)?,
                reason: r.get(1)?,
                updated_at: r.get(2)?,
            })
        },
    )?)
}

fn ids(db: &Connection, sql: &str) -> Result<Vec<String>> {
    let mut stmt = db.prepare(sql)?;
    let rows = stmt.query_map([], |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

impl Store {
    // ---- Council ----------------------------------------------------------

    pub fn create_council(&self, input: NewCouncil) -> Result<CouncilDto> {
        ensure(
            !input.prompt.trim().is_empty() && !input.repo_cwd.is_empty(),
            "council prompt and repository are required",
        )?;
        self.write(move |db| {
            let id = new_id('c');
            db.execute(
                "INSERT INTO councils(id,ticket_id,repo_cwd,level,status,prompt,preference,created_at)
                 VALUES(?1,?2,?3,?4,'running',?5,?6,?7)",
                params![
                    id,
                    input.ticket_id,
                    input.repo_cwd,
                    enum_value(input.level)?,
                    input.prompt,
                    enum_value(input.preference)?,
                    now(db)?
                ],
            )?;
            publish_council(db, &id)
        })
    }

    /// Idempotent on `(council_id, label)` with the same `input_hash`; a
    /// different hash for an existing label is a conflict.
    pub fn add_council_proposal(&self, council_id: &str, input: NewProposal) -> Result<CouncilDto> {
        ensure(
            !input.label.is_empty() && !input.input_hash.is_empty(),
            "proposal label and input hash are required",
        )?;
        let council_id = council_id.to_owned();
        self.write(move |db| {
            if council_status(db, &council_id)? != CouncilStatus::Running {
                return Err(Error::ConflictError);
            }
            let existing: Option<String> = db
                .query_row(
                    "SELECT input_hash FROM council_proposals WHERE council_id=?1 AND label=?2",
                    params![council_id, input.label],
                    |r| r.get(0),
                )
                .optional()?;
            match existing {
                Some(hash) if hash == input.input_hash => return load_council(db, &council_id),
                Some(_) => return Err(Error::ConflictError),
                None => {}
            }
            db.execute(
                "INSERT INTO council_proposals(id,council_id,label,vendor,model,payload_json,input_hash,created_at)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                params![
                    new_id('p'),
                    council_id,
                    input.label,
                    enum_value(input.vendor)?,
                    input.model,
                    serde_json::to_string(&input.payload)?,
                    input.input_hash,
                    now(db)?
                ],
            )?;
            publish_council(db, &council_id)
        })
    }

    pub fn council_checkpoint(
        &self,
        council_id: &str,
        step_key: &str,
    ) -> Result<Option<CouncilCheckpoint>> {
        self.read(|db| {
            let row: Option<(String, String)> = db
                .query_row(
                    "SELECT input_hash,output_json FROM council_checkpoints
                     WHERE council_id=?1 AND step_key=?2",
                    params![council_id, step_key],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            row.map(|(input_hash, output)| {
                Ok(CouncilCheckpoint {
                    input_hash,
                    output: serde_json::from_str(&output)?,
                })
            })
            .transpose()
        })
    }

    /// First write wins: replaying a step with the same input hash returns the
    /// stored output unchanged; a different input hash is a conflict.
    pub fn record_council_checkpoint(
        &self,
        council_id: &str,
        step_key: &str,
        input_hash: &str,
        output: Value,
    ) -> Result<Value> {
        ensure(
            !step_key.is_empty() && !input_hash.is_empty(),
            "checkpoint key and input hash are required",
        )?;
        let (council_id, step_key, input_hash) = (
            council_id.to_owned(),
            step_key.to_owned(),
            input_hash.to_owned(),
        );
        self.write(move |db| {
            council_status(db, &council_id)?;
            let existing: Option<(String, String)> = db
                .query_row(
                    "SELECT input_hash,output_json FROM council_checkpoints
                     WHERE council_id=?1 AND step_key=?2",
                    params![council_id, step_key],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            if let Some((hash, stored)) = existing {
                return if hash == input_hash {
                    Ok(serde_json::from_str(&stored)?)
                } else {
                    Err(Error::ConflictError)
                };
            }
            db.execute(
                "INSERT INTO council_checkpoints(council_id,step_key,input_hash,output_json,created_at)
                 VALUES(?1,?2,?3,?4,?5)",
                params![
                    council_id,
                    step_key,
                    input_hash,
                    serde_json::to_string(&output)?,
                    now(db)?
                ],
            )?;
            Ok(output)
        })
    }

    /// Moves an open council to `Running`, `AwaitingUser`, `Cancelled` or
    /// `Failed`. `Decided` is only reachable through [`Store::resolve_council`].
    pub fn update_council_status(
        &self,
        council_id: &str,
        status: CouncilStatus,
        escalation_reasons: Option<Vec<String>>,
    ) -> Result<CouncilDto> {
        ensure(
            status != CouncilStatus::Decided,
            "use resolve_council to decide a council",
        )?;
        let council_id = council_id.to_owned();
        self.write(move |db| {
            if !open_council(council_status(db, &council_id)?) {
                return Err(Error::ConflictError);
            }
            db.execute(
                "UPDATE councils SET status=?2 WHERE id=?1",
                params![council_id, enum_value(status)?],
            )?;
            if let Some(reasons) = escalation_reasons {
                db.execute(
                    "UPDATE councils SET escalation_reasons_json=?2 WHERE id=?1",
                    params![council_id, serde_json::to_string(&reasons)?],
                )?;
            }
            publish_council(db, &council_id)
        })
    }

    /// Records the chosen label and reveals proposal attribution.
    pub fn resolve_council(
        &self,
        council_id: &str,
        chosen_label: &str,
        decision_note: Option<String>,
    ) -> Result<CouncilDto> {
        let (council_id, chosen_label) = (council_id.to_owned(), chosen_label.to_owned());
        self.write(move |db| {
            if !open_council(council_status(db, &council_id)?) {
                return Err(Error::ConflictError);
            }
            let known: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM council_proposals WHERE council_id=?1 AND label=?2)",
                params![council_id, chosen_label],
                |r| r.get(0),
            )?;
            if !known {
                return Err(Error::Invalid("unknown proposal label"));
            }
            db.execute(
                "UPDATE councils SET status='decided',chosen_label=?2,decision_note=?3,decided_at=?4
                 WHERE id=?1",
                params![council_id, chosen_label, decision_note, now(db)?],
            )?;
            publish_council(db, &council_id)
        })
    }

    pub fn council(&self, council_id: &str) -> Result<CouncilDto> {
        self.read(|db| load_council(db, council_id))
    }

    pub fn list_councils(&self) -> Result<Vec<CouncilDto>> {
        self.read(|db| {
            ids(
                db,
                "SELECT id FROM councils ORDER BY created_at DESC, rowid DESC",
            )?
            .iter()
            .map(|id| load_council(db, id))
            .collect()
        })
    }

    // ---- PR gate ----------------------------------------------------------

    pub fn create_pr_gate(&self, input: NewPrGate) -> Result<PrGateDto> {
        ensure(
            !input.repo_slug.is_empty()
                && !input.branch.is_empty()
                && !input.base_sha.is_empty()
                && !input.head_sha.is_empty(),
            "pr gate repository, branch and commits are required",
        )?;
        self.write(move |db| {
            let ticket: Option<String> = db
                .query_row(
                    "SELECT ticket_id FROM runs WHERE id=?1",
                    [&input.run_id],
                    |r| r.get(0),
                )
                .optional()?;
            let ticket = ticket.ok_or(Error::NotFound)?;
            let id = new_id('g');
            let at = now(db)?;
            db.execute(
                "INSERT INTO pr_gates(id,run_id,ticket_id,repo_slug,branch,base_sha,head_sha,status,
                    touched_paths_json,created_at,updated_at)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,'pending',?8,?9,?9)",
                params![
                    id,
                    input.run_id,
                    ticket,
                    input.repo_slug,
                    input.branch,
                    input.base_sha,
                    input.head_sha,
                    serde_json::to_string(&input.touched_paths)?,
                    at
                ],
            )?;
            publish_pr_gate(db, &id)
        })
    }

    /// Generic status transition. Arming goes through [`Store::arm_auto_merge`]
    /// and an armed gate must be disarmed through [`Store::disarm_auto_merge`].
    pub fn set_pr_gate_status(&self, gate_id: &str, status: PrGateStatus) -> Result<PrGateDto> {
        ensure(
            status != PrGateStatus::AutoMergeArmed,
            "use arm_auto_merge to arm a pr gate",
        )?;
        let gate_id = gate_id.to_owned();
        self.write(move |db| {
            let (_, armed, _) = gate_state(db, &gate_id)?;
            if armed && status != PrGateStatus::Merged {
                return Err(Error::ConflictError);
            }
            db.execute(
                "UPDATE pr_gates SET status=?2,
                    auto_merge_armed=CASE WHEN ?2='merged' THEN 0 ELSE auto_merge_armed END WHERE id=?1",
                params![gate_id, enum_value(status)?],
            )?;
            publish_pr_gate(db, &gate_id)
        })
    }

    pub fn record_scope_violations(
        &self,
        gate_id: &str,
        violations: Vec<String>,
    ) -> Result<PrGateDto> {
        let gate_id = gate_id.to_owned();
        self.write(move |db| {
            if gate_state(db, &gate_id)?.1 {
                return Err(Error::ConflictError);
            }
            let status = if violations.is_empty() {
                PrGateStatus::Pending
            } else {
                PrGateStatus::ScopeViolation
            };
            db.execute(
                "UPDATE pr_gates SET scope_violations_json=?2,status=?3 WHERE id=?1",
                params![
                    gate_id,
                    serde_json::to_string(&violations)?,
                    enum_value(status)?
                ],
            )?;
            publish_pr_gate(db, &gate_id)
        })
    }

    pub fn record_pr_opened(&self, gate_id: &str, number: u32, url: &str) -> Result<PrGateDto> {
        let (gate_id, url) = (gate_id.to_owned(), url.to_owned());
        self.write(move |db| {
            gate_state(db, &gate_id)?;
            db.execute(
                "UPDATE pr_gates SET pr_number=?2,pr_url=?3 WHERE id=?1",
                params![gate_id, number, url],
            )?;
            publish_pr_gate(db, &gate_id)
        })
    }

    /// Records a new head after a revise push. Refused while auto-merge is armed.
    pub fn set_pr_head(&self, gate_id: &str, head_sha: &str) -> Result<PrGateDto> {
        ensure(!head_sha.is_empty(), "head sha is required")?;
        let (gate_id, head_sha) = (gate_id.to_owned(), head_sha.to_owned());
        self.write(move |db| {
            if gate_state(db, &gate_id)?.1 {
                return Err(Error::ConflictError);
            }
            db.execute(
                "UPDATE pr_gates SET head_sha=?2 WHERE id=?1",
                params![gate_id, head_sha],
            )?;
            publish_pr_gate(db, &gate_id)
        })
    }

    /// Reviews are pinned to the gate's current `head_sha`.
    pub fn record_pr_review(&self, gate_id: &str, input: NewPrReview) -> Result<PrGateDto> {
        let gate_id = gate_id.to_owned();
        self.write(move |db| {
            let (_, _, head) = gate_state(db, &gate_id)?;
            if head != input.reviewed_sha {
                return Err(Error::ConflictError);
            }
            db.execute(
                "INSERT INTO pr_reviews(id,pr_gate_id,reviewer_vendor,reviewer_model,reviewed_sha,verdict,
                    comments,github_review_id,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",
                params![
                    new_id('v'),
                    gate_id,
                    enum_value(input.reviewer_vendor)?,
                    input.reviewer_model,
                    input.reviewed_sha,
                    enum_value(input.verdict)?,
                    input.comments,
                    input.github_review_id,
                    now(db)?
                ],
            )?;
            publish_pr_gate(db, &gate_id)
        })
    }

    pub fn begin_revise_round(&self, gate_id: &str) -> Result<PrGateDto> {
        let gate_id = gate_id.to_owned();
        self.write(move |db| {
            if gate_state(db, &gate_id)?.1 {
                return Err(Error::ConflictError);
            }
            db.execute(
                "UPDATE pr_gates SET revise_rounds=revise_rounds+1,status='revise_requested' WHERE id=?1",
                [&gate_id],
            )?;
            publish_pr_gate(db, &gate_id)
        })
    }

    /// Crash-safe ledger: commits `auto_merge_armed = 1` *before* the caller
    /// asks GitHub to enable auto-merge. Refused while globally paused or when
    /// `expected_head_sha` no longer matches the recorded head.
    pub fn arm_auto_merge(&self, gate_id: &str, expected_head_sha: &str) -> Result<PrGateDto> {
        let (gate_id, expected) = (gate_id.to_owned(), expected_head_sha.to_owned());
        self.write(move |db| {
            if load_pause(db)?.paused {
                return Err(Error::ConflictError);
            }
            let (status, armed, head) = gate_state(db, &gate_id)?;
            if armed {
                return load_pr_gate(db, &gate_id);
            }
            if head != expected || status != PrGateStatus::Approved {
                return Err(Error::ConflictError);
            }
            db.execute(
                "UPDATE pr_gates SET auto_merge_armed=1,status='auto_merge_armed' WHERE id=?1",
                [&gate_id],
            )?;
            publish_pr_gate(db, &gate_id)
        })
    }

    /// Clears the ledger once GitHub confirmed auto-merge was disabled (or the
    /// enable call failed), moving the gate to `next`.
    pub fn disarm_auto_merge(&self, gate_id: &str, next: PrGateStatus) -> Result<PrGateDto> {
        ensure(
            next != PrGateStatus::AutoMergeArmed,
            "disarm must leave the armed state",
        )?;
        let gate_id = gate_id.to_owned();
        self.write(move |db| {
            gate_state(db, &gate_id)?;
            db.execute(
                "UPDATE pr_gates SET auto_merge_armed=0,status=?2 WHERE id=?1",
                params![gate_id, enum_value(next)?],
            )?;
            publish_pr_gate(db, &gate_id)
        })
    }

    pub fn pr_gate(&self, gate_id: &str) -> Result<PrGateDto> {
        self.read(|db| load_pr_gate(db, gate_id))
    }

    pub fn list_pr_gates(&self) -> Result<Vec<PrGateDto>> {
        self.read(|db| {
            ids(
                db,
                "SELECT id FROM pr_gates ORDER BY created_at DESC, rowid DESC",
            )?
            .iter()
            .map(|id| load_pr_gate(db, id))
            .collect()
        })
    }

    /// Gates whose auto-merge must be revoked on Pause or reconciled after a crash.
    pub fn armed_pr_gates(&self) -> Result<Vec<PrGateDto>> {
        self.read(|db| {
            ids(
                db,
                "SELECT id FROM pr_gates WHERE auto_merge_armed=1 ORDER BY rowid",
            )?
            .iter()
            .map(|id| load_pr_gate(db, id))
            .collect()
        })
    }

    // ---- SCM observer cursors ---------------------------------------------

    pub fn scm_cursor(&self, repo_slug: &str, pr_number: u32) -> Result<Option<ScmCursor>> {
        self.read(|db| {
            Ok(db
                .query_row(
                    "SELECT repo_slug,pr_number,last_event_etag,last_check_sha,last_comment_id
                     FROM scm_observer_cursors WHERE repo_slug=?1 AND pr_number=?2",
                    params![repo_slug, pr_number],
                    |r| {
                        Ok(ScmCursor {
                            repo_slug: r.get(0)?,
                            pr_number: r.get(1)?,
                            last_event_etag: r.get(2)?,
                            last_check_sha: r.get(3)?,
                            last_comment_id: r.get(4)?,
                        })
                    },
                )
                .optional()?)
        })
    }

    pub fn save_scm_cursor(&self, cursor: ScmCursor) -> Result<()> {
        ensure(!cursor.repo_slug.is_empty(), "repository is required")?;
        self.write(move |db| {
            db.execute(
                "INSERT INTO scm_observer_cursors(repo_slug,pr_number,last_event_etag,last_check_sha,
                    last_comment_id,updated_at) VALUES(?1,?2,?3,?4,?5,?6)
                 ON CONFLICT(repo_slug,pr_number) DO UPDATE SET last_event_etag=excluded.last_event_etag,
                    last_check_sha=excluded.last_check_sha,last_comment_id=excluded.last_comment_id,
                    updated_at=excluded.updated_at",
                params![
                    cursor.repo_slug,
                    cursor.pr_number,
                    cursor.last_event_etag,
                    cursor.last_check_sha,
                    cursor.last_comment_id,
                    now(db)?
                ],
            )?;
            Ok(())
        })
    }

    // ---- Global pause -----------------------------------------------------

    pub fn pause_state(&self) -> Result<PauseStateDto> {
        self.read(load_pause)
    }

    /// Persists the global Pause flag. Armed gates stay in the ledger until the
    /// caller confirms GitHub revoked them, so a crash mid-pause is retried.
    pub fn set_pause(&self, paused: bool, reason: Option<String>) -> Result<PauseStateDto> {
        self.write(move |db| {
            db.execute(
                "UPDATE host_pause_state SET paused=?1,reason=?2,updated_at=?3 WHERE singleton=1",
                params![paused, reason, now(db)?],
            )?;
            let state = load_pause(db)?;
            Store::append_change(
                db,
                ChangeTopic::Pause,
                "host",
                &serde_json::to_value(&state)?,
            )?;
            Ok(state)
        })
    }
}
