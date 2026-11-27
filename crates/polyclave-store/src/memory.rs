use crate::{decode_enum, enum_value, new_id, now, parse_json, Error, Result, Store};
use polyclave_proto::{ChangeTopic, MemoryItemDto, MemoryKind, MemoryScope};
use rusqlite::{named_params, params, Connection};
use serde_json::json;

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
