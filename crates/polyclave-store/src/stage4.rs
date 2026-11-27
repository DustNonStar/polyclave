//! Stage 4 durable state: CUA desktop mutex lock, hybrid memory embeddings,
//! local evolution suggestions (with rollback), Teach-Once local skills, and opt-in
//! community sharing preference.
use crate::{decode_enum, enum_value, new_id, now, parse_json, Error, Result, Store};
use polyclave_proto::{
    ChangeTopic, CuaLockDto, EvolutionKind, EvolutionStatus, EvolutionSuggestionDto, LocalSkillDto,
    MemoryItemDto, Vendor,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;

/// Additive, idempotent Stage 4 schema.
pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS cua_lock_state (
  singleton           INTEGER PRIMARY KEY CHECK (singleton = 1),
  locked              INTEGER NOT NULL DEFAULT 0 CHECK (locked IN (0, 1)),
  holder_run_id       TEXT,
  target_window       TEXT,
  idle_threshold_secs INTEGER NOT NULL DEFAULT 30,
  acquired_at         TEXT,
  reclaimed_at        TEXT
);
INSERT OR IGNORE INTO cua_lock_state (singleton, locked, idle_threshold_secs) VALUES (1, 0, 30);

CREATE TABLE IF NOT EXISTS evolution_suggestions (
  id            TEXT PRIMARY KEY,
  kind          TEXT NOT NULL CHECK (kind IN ('routing', 'memory_rule', 'role_brief')),
  title         TEXT NOT NULL,
  rationale     TEXT NOT NULL,
  payload_json  TEXT NOT NULL,
  status        TEXT NOT NULL CHECK (status IN ('proposed', 'applied', 'rolled_back', 'rejected')),
  supersedes_id TEXT REFERENCES evolution_suggestions(id),
  created_at    TEXT NOT NULL,
  updated_at    TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS local_skills (
  id                  TEXT PRIMARY KEY,
  name                TEXT NOT NULL,
  version             INTEGER NOT NULL CHECK (version >= 1),
  supersedes_id       TEXT REFERENCES local_skills(id),
  description         TEXT NOT NULL,
  body_md             TEXT NOT NULL,
  enabled             INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
  synced_vendors_json TEXT NOT NULL DEFAULT '[]',
  created_at          TEXT NOT NULL,
  UNIQUE (name, version)
);

CREATE TABLE IF NOT EXISTS community_preferences (
  singleton  INTEGER PRIMARY KEY CHECK (singleton = 1),
  opt_in     INTEGER NOT NULL DEFAULT 0 CHECK (opt_in IN (0, 1)),
  updated_at TEXT NOT NULL
);
INSERT OR IGNORE INTO community_preferences (singleton, opt_in, updated_at)
VALUES (1, 0, '2026-11-27T00:00:00.000Z');

CREATE TRIGGER IF NOT EXISTS memory_items_invalidate_emb
AFTER UPDATE OF title, content, tags_json ON memory_items
BEGIN
  UPDATE memory_items SET embedding_blob = NULL WHERE rowid = new.rowid;
END;
";

#[derive(Clone, Debug)]
pub struct NewEvolutionSuggestion {
    pub kind: EvolutionKind,
    pub title: String,
    pub rationale: String,
    pub payload_json: Value,
    pub supersedes_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct NewLocalSkill {
    pub name: String,
    pub description: String,
    pub body_md: String,
    pub synced_vendors: Vec<Vendor>,
}

fn read_cua(db: &Connection) -> Result<CuaLockDto> {
    db.query_row(
        "SELECT locked, holder_run_id, target_window, idle_threshold_secs, acquired_at, reclaimed_at
         FROM cua_lock_state WHERE singleton=1",
        [],
        |r| {
            Ok(CuaLockDto {
                locked: r.get::<_, i64>(0)? != 0,
                holder_run_id: r.get(1)?,
                target_window: r.get(2)?,
                idle_threshold_secs: r.get::<_, u32>(3)?,
                acquired_at: r.get(4)?,
                reclaimed_at: r.get(5)?,
            })
        },
    )
    .map_err(Error::from)
}

fn read_evolution(db: &Connection, id: &str) -> Result<EvolutionSuggestionDto> {
    db.query_row(
        "SELECT id, kind, title, rationale, payload_json, status, supersedes_id, created_at, updated_at
         FROM evolution_suggestions WHERE id=?1",
        [id],
        |r| {
            let kind: String = r.get(1)?;
            let status: String = r.get(5)?;
            Ok(EvolutionSuggestionDto {
                id: r.get(0)?,
                kind: decode_enum(&kind)?,
                title: r.get(2)?,
                rationale: r.get(3)?,
                payload_json: parse_json(r.get(4)?, 4)?,
                status: decode_enum(&status)?,
                supersedes_id: r.get(6)?,
                created_at: r.get(7)?,
                updated_at: r.get(8)?,
            })
        },
    )
    .optional()?
    .ok_or(Error::NotFound)
}

fn read_skill(db: &Connection, id: &str) -> Result<LocalSkillDto> {
    db.query_row(
        "SELECT id, name, version, supersedes_id, description, body_md, enabled, synced_vendors_json, created_at
         FROM local_skills WHERE id=?1",
        [id],
        |r| {
            Ok(LocalSkillDto {
                id: r.get(0)?,
                name: r.get(1)?,
                version: r.get::<_, u32>(2)?,
                supersedes_id: r.get(3)?,
                description: r.get(4)?,
                body_md: r.get(5)?,
                enabled: r.get::<_, i64>(6)? != 0,
                synced_vendors: parse_json(r.get(7)?, 7)?,
                created_at: r.get(8)?,
            })
        },
    )
    .optional()?
    .ok_or(Error::NotFound)
}

/// Deterministic 128-dim unit-normalized FNV-1a hashed character-trigram embedding.
pub fn local_embedding(text: &str) -> Vec<f32> {
    let mut vec = vec![0.0f32; 128];
    let bytes = text.to_lowercase().into_bytes();
    if bytes.is_empty() {
        return vec;
    }
    for win in bytes.windows(3.min(bytes.len())) {
        let mut h = 2166136261u32;
        for &b in win {
            h ^= u32::from(b);
            h = h.wrapping_mul(16777619);
        }
        vec[(h as usize) % 128] += 1.0;
    }
    let norm = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for v in &mut vec {
            *v /= norm;
        }
    }
    vec
}

pub(crate) fn memory_embedding_blob(title: &str, content: &str, tags: &[String]) -> Vec<u8> {
    let combined = if tags.is_empty() {
        format!("{title}\n{content}")
    } else {
        format!("{title}\n{content}\n{}", tags.join(" "))
    };
    encode_f32_le(&local_embedding(&combined))
}

pub(crate) fn encode_f32_le(vec: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(vec.len() * 4);
    for &f in vec {
        out.extend_from_slice(&f.to_le_bytes());
    }
    out
}

pub(crate) fn decode_f32_le(bytes: &[u8]) -> Vec<f32> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .copied()
        .map(f32::from_le_bytes)
        .collect()
}

pub(crate) fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for (&x, &y) in a.iter().zip(b.iter()) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    let denom = na.sqrt() * nb.sqrt();
    if denom == 0.0 {
        0.0
    } else {
        dot / denom
    }
}

impl Store {
    pub fn cua_state(&self) -> Result<CuaLockDto> {
        self.read(read_cua)
    }

    /// Acquires the single-machine CUA desktop mutex if unlocked and user is idle.
    /// Never blocks; returns `Error::BusyError` immediately if occupied or user is active.
    pub fn cua_acquire(
        &self,
        holder_run_id: impl Into<String>,
        target_window: impl Into<String>,
        observed_idle_secs: u32,
    ) -> Result<CuaLockDto> {
        let holder = holder_run_id.into();
        let target = target_window.into();
        if holder.trim().is_empty() || target.trim().is_empty() {
            return Err(Error::Invalid("holder_run_id and target_window required"));
        }
        self.write(move |db| {
            let cur = read_cua(db)?;
            if cur.locked || observed_idle_secs < cur.idle_threshold_secs {
                return Err(Error::BusyError);
            }
            let ts = now(db)?;
            db.execute(
                "UPDATE cua_lock_state
                 SET locked=1, holder_run_id=?1, target_window=?2, acquired_at=?3
                 WHERE singleton=1 AND locked=0",
                params![holder, target, ts],
            )?;
            let dto = read_cua(db)?;
            Store::append_change(db, ChangeTopic::Cua, "cua", &serde_json::to_value(&dto)?)?;
            Ok(dto)
        })
    }

    pub fn cua_release(&self, holder_run_id: &str) -> Result<CuaLockDto> {
        let holder = holder_run_id.to_owned();
        self.write(move |db| {
            let cur = read_cua(db)?;
            if !cur.locked {
                return Ok(cur);
            }
            if cur.holder_run_id.as_deref() != Some(holder.as_str()) {
                return Err(Error::ConflictError);
            }
            db.execute(
                "UPDATE cua_lock_state SET locked=0, holder_run_id=NULL, target_window=NULL WHERE singleton=1",
                [],
            )?;
            let dto = read_cua(db)?;
            Store::append_change(
                db,
                ChangeTopic::Cua,
                "cua",
                &serde_json::to_value(&dto)?,
            )?;
            Ok(dto)
        })
    }

    /// Immediate unconditional user reclaim of the desktop CUA lock.
    pub fn cua_reclaim(&self) -> Result<CuaLockDto> {
        self.write(|db| {
            let ts = now(db)?;
            db.execute(
                "UPDATE cua_lock_state
                 SET locked=0, holder_run_id=NULL, target_window=NULL, reclaimed_at=?1
                 WHERE singleton=1",
                [&ts],
            )?;
            let dto = read_cua(db)?;
            Store::append_change(db, ChangeTopic::Cua, "cua", &serde_json::to_value(&dto)?)?;
            Ok(dto)
        })
    }

    pub fn store_memory_embedding(&self, memory_id: &str, embedding: &[f32]) -> Result<()> {
        let id = memory_id.to_owned();
        let blob = encode_f32_le(embedding);
        self.write(move |db| {
            let changed = db.execute(
                "UPDATE memory_items SET embedding_blob=?1 WHERE id=?2",
                params![blob, id],
            )?;
            if changed == 0 {
                return Err(Error::NotFound);
            }
            Ok(())
        })
    }

    pub fn hybrid_search_memory(
        &self,
        query_vec: &[f32],
        limit: usize,
    ) -> Result<Vec<(MemoryItemDto, f32)>> {
        let q = query_vec.to_vec();
        self.read(move |db| {
            let mut stmt = db.prepare(
                "SELECT id, scope, repo_cwd, kind, title, content, tags_json, evidence_json,
                        status, supersedes_id, created_by, source_run_id, source_turn_id,
                        created_at, updated_at, embedding_blob
                 FROM memory_items
                 WHERE status='verified' AND embedding_blob IS NOT NULL",
            )?;
            let rows = stmt.query_map([], |r| {
                let scope: String = r.get(1)?;
                let kind: String = r.get(3)?;
                let status: String = r.get(8)?;
                let blob: Vec<u8> = r.get(15)?;
                let dto = MemoryItemDto {
                    id: r.get(0)?,
                    scope: decode_enum(&scope)?,
                    repo_cwd: r.get(2)?,
                    kind: decode_enum(&kind)?,
                    title: r.get(4)?,
                    content: r.get(5)?,
                    tags: parse_json(r.get(6)?, 6)?,
                    evidence: r
                        .get::<_, Option<String>>(7)?
                        .map(|s| parse_json(s, 7))
                        .transpose()?,
                    status: decode_enum(&status)?,
                    supersedes_id: r.get(9)?,
                    created_by: r.get(10)?,
                    source_run_id: r.get(11)?,
                    source_turn_id: r.get(12)?,
                    created_at: r.get(13)?,
                    updated_at: r.get(14)?,
                };
                let score = cosine(&q, &decode_f32_le(&blob));
                Ok((dto, score))
            })?;
            let mut scored = rows.collect::<rusqlite::Result<Vec<_>>>()?;
            scored.sort_by(|a, b| b.1.total_cmp(&a.1));
            scored.truncate(limit.clamp(1, 50));
            Ok(scored)
        })
    }

    pub fn create_evolution_suggestion(
        &self,
        input: NewEvolutionSuggestion,
    ) -> Result<EvolutionSuggestionDto> {
        let id = new_id('e');
        let kind_str = enum_value(input.kind)?;
        let status_str = enum_value(EvolutionStatus::Proposed)?;
        let payload = serde_json::to_string(&input.payload_json)?;
        self.write(move |db| {
            let ts = now(db)?;
            db.execute(
                "INSERT INTO evolution_suggestions
                 (id, kind, title, rationale, payload_json, status, supersedes_id, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
                params![
                    id,
                    kind_str,
                    input.title,
                    input.rationale,
                    payload,
                    status_str,
                    input.supersedes_id,
                    ts
                ],
            )?;
            let dto = read_evolution(db, &id)?;
            Store::append_change(
                db,
                ChangeTopic::Evolution,
                &dto.id,
                &serde_json::to_value(&dto)?,
            )?;
            Ok(dto)
        })
    }

    pub fn list_evolution_suggestions(&self) -> Result<Vec<EvolutionSuggestionDto>> {
        self.read(|db| {
            let mut stmt = db.prepare(
                "SELECT id, kind, title, rationale, payload_json, status, supersedes_id, created_at, updated_at
                 FROM evolution_suggestions ORDER BY rowid DESC",
            )?;
            let rows = stmt.query_map([], |r| {
                let kind: String = r.get(1)?;
                let status: String = r.get(5)?;
                Ok(EvolutionSuggestionDto {
                    id: r.get(0)?,
                    kind: decode_enum(&kind)?,
                    title: r.get(2)?,
                    rationale: r.get(3)?,
                    payload_json: parse_json(r.get(4)?, 4)?,
                    status: decode_enum(&status)?,
                    supersedes_id: r.get(6)?,
                    created_at: r.get(7)?,
                    updated_at: r.get(8)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Error::from)
        })
    }

    pub fn set_evolution_status(
        &self,
        id: &str,
        status: EvolutionStatus,
    ) -> Result<EvolutionSuggestionDto> {
        let id = id.to_owned();
        let status_str = enum_value(status)?;
        self.write(move |db| {
            let ts = now(db)?;
            let changed = db.execute(
                "UPDATE evolution_suggestions SET status=?1, updated_at=?2 WHERE id=?3",
                params![status_str, ts, id],
            )?;
            if changed == 0 {
                return Err(Error::NotFound);
            }
            let dto = read_evolution(db, &id)?;
            Store::append_change(
                db,
                ChangeTopic::Evolution,
                &dto.id,
                &serde_json::to_value(&dto)?,
            )?;
            Ok(dto)
        })
    }

    pub fn save_local_skill(&self, input: NewLocalSkill) -> Result<LocalSkillDto> {
        let name = input.name.trim().to_owned();
        if name.is_empty() || input.body_md.trim().is_empty() {
            return Err(Error::Invalid("skill name and body_md required"));
        }
        let vendors_json = serde_json::to_string(&input.synced_vendors)?;
        let id = new_id('k');
        self.write(move |db| {
            let prev: Option<(String, u32)> = db
                .query_row(
                    "SELECT id, version FROM local_skills WHERE name=?1 ORDER BY version DESC LIMIT 1",
                    [&name],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let (supersedes_id, version) = match prev {
                Some((prev_id, prev_ver)) => {
                    db.execute("UPDATE local_skills SET enabled=0 WHERE name=?1", [&name])?;
                    (Some(prev_id), prev_ver + 1)
                }
                None => (None, 1),
            };
            let ts = now(db)?;
            db.execute(
                "INSERT INTO local_skills
                 (id, name, version, supersedes_id, description, body_md, enabled, synced_vendors_json, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, ?8)",
                params![
                    id,
                    name,
                    version,
                    supersedes_id,
                    input.description,
                    input.body_md,
                    vendors_json,
                    ts
                ],
            )?;
            let dto = read_skill(db, &id)?;
            Store::append_change(
                db,
                ChangeTopic::Skill,
                &dto.id,
                &serde_json::to_value(&dto)?,
            )?;
            Ok(dto)
        })
    }

    pub fn list_local_skills(&self) -> Result<Vec<LocalSkillDto>> {
        self.read(|db| {
            let mut stmt = db.prepare(
                "SELECT id, name, version, supersedes_id, description, body_md, enabled, synced_vendors_json, created_at
                 FROM local_skills ORDER BY rowid DESC",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(LocalSkillDto {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    version: r.get::<_, u32>(2)?,
                    supersedes_id: r.get(3)?,
                    description: r.get(4)?,
                    body_md: r.get(5)?,
                    enabled: r.get::<_, i64>(6)? != 0,
                    synced_vendors: parse_json(r.get(7)?, 7)?,
                    created_at: r.get(8)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Error::from)
        })
    }

    pub fn rollback_local_skill(&self, id: &str) -> Result<LocalSkillDto> {
        let id = id.to_owned();
        self.write(move |db| {
            let current = read_skill(db, &id)?;
            db.execute(
                "UPDATE local_skills SET enabled=0 WHERE name=?1",
                [&current.name],
            )?;
            let target_id = current.supersedes_id.unwrap_or(current.id);
            db.execute(
                "UPDATE local_skills SET enabled=1 WHERE id=?1",
                [&target_id],
            )?;
            let dto = read_skill(db, &target_id)?;
            Store::append_change(
                db,
                ChangeTopic::Skill,
                &dto.id,
                &serde_json::to_value(&dto)?,
            )?;
            Ok(dto)
        })
    }

    pub fn community_opt_in(&self) -> Result<bool> {
        self.read(|db| {
            let v: i64 = db.query_row(
                "SELECT opt_in FROM community_preferences WHERE singleton=1",
                [],
                |r| r.get(0),
            )?;
            Ok(v != 0)
        })
    }

    pub fn set_community_opt_in(&self, opt_in: bool) -> Result<bool> {
        self.write(move |db| {
            let ts = now(db)?;
            db.execute(
                "UPDATE community_preferences SET opt_in=?1, updated_at=?2 WHERE singleton=1",
                params![i64::from(opt_in), ts],
            )?;
            Ok(opt_in)
        })
    }
}
