use crate::{decode_enum, enum_value, new_id, now, parse_json, Error, Result, Store};
use polyclave_proto::{ChangeTopic, MemoryItemDto, MemoryKind, MemoryScope};
use rusqlite::{named_params, params, Connection};
use serde_json::{json, Value};

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
            db.execute("INSERT INTO memory_items(id,scope,repo_cwd,kind,title,content,tags_json,status,created_by,
                 source_run_id,source_turn_id,created_at,updated_at) VALUES(?1,'project',?2,?3,?4,?5,?6,'candidate',?7,?8,?9,?10,?10)",
                 params![id,repo,enum_value(input.kind)?,input.title,input.content,serde_json::to_string(&input.tags)?,author,run,turn_id,now(db)?])?;
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
        db.execute("INSERT INTO memory_items(id,scope,repo_cwd,kind,title,content,evidence_json,status,created_by,
           source_run_id,created_at,updated_at) VALUES(?1,'run',?2,'receipt',?3,?3,?4,'verified','host',?5,?6,?6)",
           params![id,repo,title,json!({"run_id":run,"sha":sha}).to_string(),run,now(db)?])?;
        let record = db.query_row(
            &format!("SELECT {COLUMNS} FROM memory_items m WHERE m.id=?1"),
            [&id],
            read_memory,
        )?;
        Store::append_change(db, ChangeTopic::Memory, &id, &serde_json::to_value(record)?)?;
        Ok(())
    }

    pub fn find_memory(&self, filter: MemoryFilter<'_>) -> Result<Vec<MemoryItemDto>> {
        let query = filter.query.map(|q| q.trim()).filter(|q| !q.is_empty());
        if query.is_some_and(|q| q.len() > 2048) {
            return Err(Error::Invalid("memory query too long"));
        }
        let query = query.map(|q| format!("\"{}\"", q.replace('"', "\"\"")));
        let kinds = serde_json::to_string(filter.kinds)?;
        let scope = filter.scope.map(enum_value).transpose()?;
        let limit = filter.limit.unwrap_or(20).clamp(1, 100) as u32;
        self.read(|db|{
            let joins=if query.is_some() {"JOIN memory_items_fts ON memory_items_fts.rowid=m.rowid"} else {""};
            let pred=if query.is_some() {"AND memory_items_fts MATCH :query"} else {"AND :query IS NULL"};
            let mut stmt=db.prepare(&format!("SELECT {COLUMNS} FROM memory_items m {joins}
                  WHERE m.status='verified' AND (:scope IS NULL OR m.scope=:scope)
                  AND (:repo IS NULL OR m.repo_cwd=:repo OR m.scope='global')
                  AND (:kinds='[]' OR m.kind IN (SELECT value FROM json_each(:kinds))) {pred}
                  ORDER BY m.updated_at DESC,m.id LIMIT :limit"))?;
            let result=stmt.query_map(named_params!{":scope":scope,":repo":filter.repo,":kinds":kinds,":query":query,":limit":limit},read_memory)?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(result)
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
            db.execute("INSERT INTO memory_items(id,scope,repo_cwd,kind,title,content,evidence_json,status,created_by,
                source_turn_id,created_at,updated_at) VALUES(?1,'project',?2,?3,?4,?5,?6,'candidate','user',?7,?8,?8)",
                params![id,repo,enum_value(kind)?,title,content,json!({"source_msg_id":message,"requires_user_confirmation":true}).to_string(),turn,now(db)?])?;
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
            match action {
                MemoryAction::Commit=>{
                    if existing.status!=polyclave_proto::MemoryStatus::Candidate{return Err(Error::ConflictError);}
                    db.execute("UPDATE memory_items SET status='verified',updated_at=?2 WHERE id=?1",params![id,stamp])?;
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
                    db.execute("UPDATE memory_items SET status='verified',supersedes_id=?2,updated_at=?3 WHERE id=?1",params![id,prior,stamp])?;
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
            db.execute("INSERT INTO memory_items(id,scope,repo_cwd,kind,title,content,evidence_json,status,created_by,
                source_run_id,created_at,updated_at) VALUES(?1,'project',?2,?3,?4,?5,?6,'candidate','user',?7,?8,?8)",
                params![id,repo,enum_value(kind)?,title,content,json!({"run_id":run,"resolution":decision,"requires_user_confirmation":true}).to_string(),run,now(db)?])?;
            publish_memory(db,&id)
        })
    }
}
