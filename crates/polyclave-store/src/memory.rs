use crate::stage4::{cosine, decode_f32_le, local_embedding, memory_embedding_blob};
use crate::{decode_enum, enum_value, new_id, now, parse_json, Error, Result, Store};
use polyclave_proto::{ChangeTopic, MemoryItemDto, MemoryKind, MemoryScope};
use rusqlite::{named_params, params, Connection};
use serde_json::{json, Value};
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub struct CandidateMemory {
    pub kind: MemoryKind,
    pub title: String,
    pub content: String,
    pub tags: Vec<String>,
}

#[derive(Debug, Default)]
pub struct MemoryFilter<'a> {
    pub repo: Option<&'a str>,
    pub scope: Option<MemoryScope>,
    pub query: Option<&'a str>,
    pub kinds: &'a [MemoryKind],
    pub limit: Option<usize>,
}

const COLUMNS: &str = "m.id,m.scope,m.repo_cwd,m.kind,m.title,m.content,m.tags_json,m.evidence_json,m.status,m.supersedes_id,m.created_by,m.source_run_id,m.source_turn_id,m.created_at,m.updated_at";

fn read_memory(row: &rusqlite::Row<'_>) -> rusqlite::Result<MemoryItemDto> {
    Ok(MemoryItemDto {
        id: row.get(0)?,
        scope: decode_enum(&row.get::<_, String>(1)?)?,
        repo_cwd: row.get(2)?,
        kind: decode_enum(&row.get::<_, String>(3)?)?,
        title: row.get(4)?,
        content: row.get(5)?,
        tags: parse_json(row.get(6)?, 6)?,
        evidence: row
            .get::<_, Option<String>>(7)?
            .map(|s| parse_json(s, 7))
            .transpose()?,
        status: decode_enum(&row.get::<_, String>(8)?)?,
        supersedes_id: row.get(9)?,
        created_by: row.get(10)?,
        source_run_id: row.get(11)?,
        source_turn_id: row.get(12)?,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
    })
}

impl Store {
    /// Provenance is derived from the authenticated turn, not the tool arguments.
    pub fn propose_memory(&self, turn_id: i64, input: CandidateMemory) -> Result<MemoryItemDto> {
        if input.title.trim().is_empty()
            || input.content.trim().is_empty()
            || input.content.len() > 16_384
            || input.title.len() > 256
            || input.tags.len() > 32
            || input.tags.iter().any(|t| t.len() > 64)
        {
            return Err(Error::Invalid("invalid memory content"));
        }
        self.write(move |db| {
            use rusqlite::OptionalExtension;
            let source: Option<(String,String,String)> = db.query_row(
               "SELECT r.repo_cwd,r.id,r.vendor FROM turns t JOIN runs r ON r.id=t.run_id
                WHERE t.id=?1 AND t.status='running' AND t.phase='authorized' AND t.mcp_revoked=0 AND t.attempt_id=
                (SELECT id FROM attempts WHERE run_id=r.id AND attempt_no=r.current_attempt)",
               [turn_id], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
            let Some((repo,run,author))=source else {return Err(Error::ConflictError)};
            let id = new_id('m');
            let blob = memory_embedding_blob(&input.title, &input.content, &input.tags);
            db.execute("INSERT INTO memory_items(id,scope,repo_cwd,kind,title,content,tags_json,status,created_by,
                 source_run_id,source_turn_id,embedding_blob,created_at,updated_at) VALUES(?1,'project',?2,?3,?4,?5,?6,'candidate',?7,?8,?9,?10,?11,?11)",
                 params![id,repo,enum_value(input.kind)?,input.title,input.content,serde_json::to_string(&input.tags)?,author,run,turn_id,blob,now(db)?])?;
            let record = db.query_row(&format!("SELECT {COLUMNS} FROM memory_items m WHERE m.id=?1"),[&id],read_memory)?;
            Store::append_change(db,ChangeTopic::Memory,&id,&serde_json::to_value(&record)?)?;
            Ok(record)
        })
    }

    /// Host-only fact writer. Must be called from the transaction that commits the verdict.
    pub fn record_run_memory(
        db: &Connection,
        run: &str,
        repo: &str,
        title: &str,
        sha: Option<&str>,
    ) -> Result<()> {
        let id = new_id('m');
        let blob = memory_embedding_blob(title, title, &[]);
        db.execute("INSERT INTO memory_items(id,scope,repo_cwd,kind,title,content,evidence_json,status,created_by,
           source_run_id,embedding_blob,created_at,updated_at) VALUES(?1,'run',?2,'receipt',?3,?3,?4,'verified','host',?5,?6,?7,?7)",
           params![id,repo,title,json!({"run_id":run,"sha":sha}).to_string(),run,blob,now(db)?])?;
        let record = db.query_row(
            &format!("SELECT {COLUMNS} FROM memory_items m WHERE m.id=?1"),
            [&id],
            read_memory,
        )?;
        Store::append_change(db, ChangeTopic::Memory, &id, &serde_json::to_value(record)?)?;
        Ok(())
    }

    pub fn find_memory(&self, filter: MemoryFilter<'_>) -> Result<Vec<MemoryItemDto>> {
        let clean = filter
            .query
            .map(|q| q.replace('\0', " "))
            .unwrap_or_default();
        let raw_query = clean.trim().to_owned();
        if raw_query.len() > 2048 {
            return Err(Error::Invalid("memory query too long"));
        }
        let query = (!raw_query.is_empty())
            .then(|| {
                raw_query
                    .split_whitespace()
                    .map(|term| format!("\"{}\"", term.replace('"', "\"\"")))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .filter(|q| !q.is_empty());
        let kinds = serde_json::to_string(filter.kinds)?;
        let scope = filter.scope.map(enum_value).transpose()?;
        let limit = filter.limit.unwrap_or(20).clamp(1, 100) as u32;
        self.read(|db|{
            let Some(fts_query) = query.as_deref() else {
                let mut stmt=db.prepare(&format!("SELECT {COLUMNS} FROM memory_items m
                      WHERE m.status='verified' AND (:scope IS NULL OR m.scope=:scope)
                      AND (:repo IS NULL OR m.repo_cwd=:repo OR m.scope='global')
                      AND (:kinds='[]' OR m.kind IN (SELECT value FROM json_each(:kinds)))
                      ORDER BY m.updated_at DESC,m.id LIMIT :limit"))?;
                let result=stmt.query_map(named_params!{":scope":scope,":repo":filter.repo,":kinds":kinds,":limit":limit},read_memory)?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                return Ok(result);
            };

            let fetch_limit = (limit * 2).min(200);
            let mut stmt=db.prepare(&format!("SELECT {COLUMNS} FROM memory_items m JOIN memory_items_fts ON memory_items_fts.rowid=m.rowid
                  WHERE m.status='verified' AND (:scope IS NULL OR m.scope=:scope)
                  AND (:repo IS NULL OR m.repo_cwd=:repo OR m.scope='global')
                  AND (:kinds='[]' OR m.kind IN (SELECT value FROM json_each(:kinds))) AND memory_items_fts MATCH :query
                  ORDER BY m.updated_at DESC,m.id LIMIT :limit"))?;
            let fts_results=stmt.query_map(named_params!{":scope":scope,":repo":filter.repo,":kinds":kinds,":query":fts_query,":limit":fetch_limit},read_memory)?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            let q_vec = local_embedding(&raw_query);
            let mut sem_stmt = db.prepare(&format!("SELECT {COLUMNS}, m.embedding_blob FROM memory_items m
                  WHERE m.status='verified' AND m.embedding_blob IS NOT NULL
                  AND (:scope IS NULL OR m.scope=:scope)
                  AND (:repo IS NULL OR m.repo_cwd=:repo OR m.scope='global')
                  AND (:kinds='[]' OR m.kind IN (SELECT value FROM json_each(:kinds)))"))?;
            let mut sem_scored: Vec<(MemoryItemDto, f32)> = sem_stmt
                .query_map(named_params!{":scope":scope,":repo":filter.repo,":kinds":kinds}, |r| {
                    let dto = read_memory(r)?;
                    let blob: Vec<u8> = r.get(15)?;
                    Ok((dto, blob))
                })?
                .filter_map(|res| {
                    let (dto, blob) = res.ok()?;
                    let score = cosine(&q_vec, &decode_f32_le(&blob));
                    (score >= 0.45).then_some((dto, score))
                })
                .collect();
            sem_scored.sort_by(|a, b| b.1.total_cmp(&a.1));
            sem_scored.truncate(fetch_limit as usize);

            let mut fused: HashMap<String, (MemoryItemDto, f32)> = HashMap::new();
            for (rank, item) in fts_results.into_iter().enumerate() {
                let s = 1.0 / (60.0 + rank as f32);
                fused.entry(item.id.clone()).and_modify(|e| e.1 += s).or_insert((item, s));
            }
            for (rank, (item, _)) in sem_scored.into_iter().enumerate() {
                let s = 0.75 / (60.0 + rank as f32);
                fused.entry(item.id.clone()).and_modify(|e| e.1 += s).or_insert((item, s));
            }
            let mut merged: Vec<(MemoryItemDto, f32)> = fused.into_values().collect();
            merged.sort_by(|a, b| {
                b.1.total_cmp(&a.1)
                    .then_with(|| b.0.updated_at.cmp(&a.0.updated_at))
                    .then_with(|| a.0.id.cmp(&b.0.id))
            });
            merged.truncate(limit as usize);
            Ok(merged.into_iter().map(|(dto, _)| dto).collect())
        })
    }
}

/// Trusted actor identity is assigned by the host, never deserialized from a tool.
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum Curator {
    User,
    Hub,
}
#[derive(Clone)]
pub enum MemoryAction {
    Commit,
    Supersede(String),
    Archive,
}

impl Store {
    /// Draft a correction only from a persisted user message in the selected project.
    pub fn draft_correction(
        &self,
        session: &str,
        message: i64,
        kind: MemoryKind,
        title: &str,
        content: &str,
    ) -> Result<MemoryItemDto> {
        if !matches!(
            kind,
            MemoryKind::Constraint | MemoryKind::Preference | MemoryKind::Pitfall
        ) || title.trim().is_empty()
            || title.len() > 256
            || content.trim().is_empty()
            || content.len() > 4096
        {
            return Err(Error::Invalid("invalid correction draft"));
        }
        let (session, title, content) = (session.to_owned(), title.to_owned(), content.to_owned());
        self.write(move|db|{
            use rusqlite::OptionalExtension;
            let row:Option<(String,Option<i64>)>=db.query_row("SELECT s.repo_cwd,m.turn_id FROM messages m
                JOIN sessions s ON s.id=m.session_id WHERE m.id=?1 AND m.session_id=?2 AND m.source='user'",
                params![message,session],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
            let Some((repo,turn))=row else{return Err(Error::NotFound);};
            let id=new_id('m');
            let blob = memory_embedding_blob(&title, &content, &[]);
            db.execute("INSERT INTO memory_items(id,scope,repo_cwd,kind,title,content,evidence_json,status,created_by,
                source_turn_id,embedding_blob,created_at,updated_at) VALUES(?1,'project',?2,?3,?4,?5,?6,'candidate','user',?7,?8,?9,?9)",
                params![id,repo,enum_value(kind)?,title,content,json!({"source_msg_id":message,"requires_user_confirmation":true}).to_string(),turn,blob,now(db)?])?;
            publish_memory(db,&id)
        })
    }

    pub fn curate_memory(
        &self,
        repo: &str,
        id: &str,
        actor: Curator,
        action: MemoryAction,
    ) -> Result<MemoryItemDto> {
        let (repo, id) = (repo.to_owned(), id.to_owned());
        self.write(move|db|{
            use rusqlite::OptionalExtension;
            let existing=db.query_row(&format!("SELECT {COLUMNS} FROM memory_items m WHERE m.id=?1"),[&id],read_memory)
                .optional()?.ok_or(Error::NotFound)?;
            let in_scope=existing.repo_cwd.as_deref()==Some(repo.as_str()) ||
                (existing.scope==MemoryScope::Global && actor==Curator::User);
            if !in_scope{return Err(Error::ConflictError);}
            if actor!=Curator::User && existing.evidence.as_ref().and_then(|e|e.get("requires_user_confirmation")).and_then(Value::as_bool)==Some(true) {
                return Err(Error::ConflictError);
            }
            let stamp=now(db)?;
            let blob = memory_embedding_blob(&existing.title, &existing.content, &existing.tags);
            match action {
                MemoryAction::Commit=>{
                    if existing.status!=polyclave_proto::MemoryStatus::Candidate{return Err(Error::ConflictError);}
                    db.execute("UPDATE memory_items SET status='verified',embedding_blob=COALESCE(embedding_blob,?3),updated_at=?2 WHERE id=?1",params![id,stamp,blob])?;
                },
                MemoryAction::Supersede(prior)=>{
                    if id==prior || existing.status!=polyclave_proto::MemoryStatus::Candidate {return Err(Error::ConflictError);}
                    let old=db.query_row(&format!("SELECT {COLUMNS} FROM memory_items m WHERE m.id=?1"),[&prior],read_memory)
                        .optional()?.ok_or(Error::NotFound)?;
                    if old.scope!=existing.scope || old.repo_cwd!=existing.repo_cwd || old.kind!=existing.kind ||
                        old.status!=polyclave_proto::MemoryStatus::Verified{
                        return Err(Error::ConflictError);
                    }
                    db.execute("UPDATE memory_items SET status='superseded',updated_at=?2 WHERE id=?1",params![prior,stamp])?;
                    publish_memory(db,&prior)?;
                    db.execute("UPDATE memory_items SET status='verified',supersedes_id=?2,embedding_blob=COALESCE(embedding_blob,?4),updated_at=?3 WHERE id=?1",params![id,prior,stamp,blob])?;
                },
                MemoryAction::Archive=>{
                    if matches!(existing.status,polyclave_proto::MemoryStatus::Archived|polyclave_proto::MemoryStatus::Superseded){
                        return Err(Error::ConflictError);
                    }
                    db.execute("UPDATE memory_items SET status='archived',updated_at=?2 WHERE id=?1",params![id,stamp])?;
                }
            }
            publish_memory(db,&id)
        })
    }
}
fn publish_memory(db: &Connection, id: &str) -> Result<MemoryItemDto> {
    let item = db.query_row(
        &format!("SELECT {COLUMNS} FROM memory_items m WHERE m.id=?1"),
        [id],
        read_memory,
    )?;
    Store::append_change(db, ChangeTopic::Memory, id, &serde_json::to_value(&item)?)?;
    Ok(item)
}

impl Store {
    pub fn draft_run_correction(
        &self,
        repo: &str,
        run: &str,
        kind: MemoryKind,
        title: &str,
        content: &str,
    ) -> Result<MemoryItemDto> {
        if !matches!(
            kind,
            MemoryKind::Constraint | MemoryKind::Preference | MemoryKind::Pitfall
        ) || title.trim().is_empty()
            || title.len() > 256
            || content.trim().is_empty()
            || content.len() > 4096
        {
            return Err(Error::Invalid("invalid correction draft"));
        }
        let (repo, run, title, content) = (
            repo.to_owned(),
            run.to_owned(),
            title.to_owned(),
            content.to_owned(),
        );
        self.write(move|db|{
            use rusqlite::OptionalExtension;
            let decision:Option<String>=db.query_row("SELECT resolution FROM runs WHERE id=?1 AND repo_cwd=?2
                AND resolution IN ('fail','allow_retry')",params![run,repo],|r|r.get(0)).optional()?;
            let Some(decision)=decision else{return Err(Error::NotFound);};
            let id=new_id('m');
            let blob = memory_embedding_blob(&title, &content, &[]);
            db.execute("INSERT INTO memory_items(id,scope,repo_cwd,kind,title,content,evidence_json,status,created_by,
                source_run_id,embedding_blob,created_at,updated_at) VALUES(?1,'project',?2,?3,?4,?5,?6,'candidate','user',?7,?8,?9,?9)",
                params![id,repo,enum_value(kind)?,title,content,json!({"run_id":run,"resolution":decision,"requires_user_confirmation":true}).to_string(),run,blob,now(db)?])?;
            publish_memory(db,&id)
        })
    }
}
