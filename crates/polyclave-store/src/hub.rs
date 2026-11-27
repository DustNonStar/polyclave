//! All Hub writes use the same writer transaction as Worker and memory writes.
use crate::{now, Error, Result, Store};
use polyclave_proto::ChangeTopic;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HubMessage {
    pub id: i64,
    pub turn_id: Option<i64>,
    pub source: String,
    pub text: String,
    pub reasoning: Option<String>,
    pub metadata: Option<Value>,
    pub queued: bool,
}

pub struct HubTurn {
    pub id: i64,
    pub session: String,
    pub profile: String,
    pub model: String,
    pub repo: String,
    pub summary: String,
    pub input: HubMessage,
    mcp_token: String,
}
impl HubTurn {
    /// Only the host-side transport uses this, never serialization or logs.
    pub fn turn_credential(&self) -> &str {
        &self.mcp_token
    }
}
#[derive(Clone, Debug, PartialEq)]
pub struct ThreadDelta {
    pub handle: Option<String>,
    pub last_synced: i64,
    pub messages: Vec<HubMessage>,
}
#[derive(Default)]
pub struct HubCompletion {
    pub text: String,
    pub reasoning: Option<String>,
    pub metadata: Option<Value>,
    pub native_thread: Option<String>,
    pub delivered: Vec<i64>,
    pub rebuilt: bool,
}

const MESSAGE_COLUMNS: &str = "m.id,m.turn_id,m.source,m.text,m.reasoning,m.metadata_json,m.queued";
fn message(db: &Connection, id: i64) -> Result<HubMessage> {
    Ok(db.query_row(
        &format!("SELECT {MESSAGE_COLUMNS} FROM messages m WHERE id=?1"),
        [id],
        row,
    )?)
}
fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<HubMessage> {
    Ok(HubMessage {
        id: r.get(0)?,
        turn_id: r.get(1)?,
        source: r.get(2)?,
        text: r.get(3)?,
        reasoning: r.get(4)?,
        metadata: r
            .get::<_, Option<String>>(5)?
            .map(|s| crate::parse_json(s, 5))
            .transpose()?,
        queued: r.get(6)?,
    })
}
fn publish(db: &Connection, id: i64) -> Result<()> {
    Store::append_change(
        db,
        ChangeTopic::Message,
        &id.to_string(),
        &serde_json::to_value(message(db, id)?)?,
    )?;
    Ok(())
}

impl Store {
    pub fn hub_enqueue(&self, session: &str, intent: &str, text: &str) -> Result<HubMessage> {
        if intent.is_empty()
            || intent.len() > 256
            || text.trim().is_empty()
            || text.len() > 256 * 1024
        {
            return Err(Error::Invalid("invalid Hub input"));
        }
        let (session, intent, text) = (session.to_owned(), intent.to_owned(), text.to_owned());
        self.write(move|db|{
            let role:Option<String>=db.query_row("SELECT role FROM sessions WHERE id=?1",[&session],|r|r.get(0)).optional()?;
            if !matches!(role.as_deref(),Some("hub"|"manager")){return Err(Error::NotFound);}
            let old:Option<i64>=db.query_row("SELECT message_id FROM hub_input_requests WHERE session_id=?1 AND request_id=?2",
                params![session,intent],|r|r.get(0)).optional()?;
            if let Some(id)=old{
                let item=message(db,id)?;
                if item.text!=text{return Err(Error::ConflictError);}
                return Ok(item);
            }
            db.execute("INSERT INTO messages(session_id,source,text,queued,created_at) VALUES(?1,'user',?2,1,?3)",
                params![session,text,now(db)?])?;
            let id=db.last_insert_rowid();
            db.execute("INSERT INTO hub_input_requests(session_id,request_id,message_id) VALUES(?1,?2,?3)",params![session,intent,id])?;
            publish(db,id)?;
            message(db,id)
        })
    }

    pub fn hub_messages(&self, session: &str) -> Result<Vec<HubMessage>> {
        self.read(|db| {
            let mut q = db.prepare(&format!(
                "SELECT {MESSAGE_COLUMNS} FROM messages m WHERE session_id=?1 ORDER BY id"
            ))?;
            let messages = q
                .query_map([session], row)?
                .collect::<rusqlite::Result<_>>()?;
            Ok(messages)
        })
    }

    pub fn hub_claim(&self, session: &str, host: &str, boot: &str) -> Result<Option<HubTurn>> {
        let (session, host, boot) = (session.to_owned(), host.to_owned(), boot.to_owned());
        self.write(move|db|{
            let (role,profile,model,repo,summary):(String,String,String,String,String)=db.query_row(
                "SELECT role,profile_id,provider_model,repo_cwd,rolling_summary FROM sessions WHERE id=?1",[&session],
                |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?.ok_or(Error::NotFound)?;
            if !matches!(role.as_str(),"hub"|"manager"){return Err(Error::ConflictError);}
            if db.query_row("SELECT EXISTS(SELECT 1 FROM turns WHERE session_id=?1 AND status='running' AND run_id IS NULL AND mode!='consult')",
                [&session],|r|r.get::<_,bool>(0))?{return Err(Error::BusyError);}
            let input:Option<i64>=db.query_row("SELECT id FROM messages WHERE session_id=?1 AND source IN ('user','host') AND queued=1
                AND turn_id IS NULL ORDER BY id LIMIT 1",[&session],|r|r.get(0)).optional()?;
            let Some(input)=input else{return Ok(None);};
            let key=uuid::Uuid::new_v4().simple().to_string();
            db.execute("INSERT INTO turns(session_id,mode,status,phase,host_instance_id,boot_epoch,mcp_token,started_at)
                VALUES(?1,?2,'running','prepared',?3,?4,?5,?6)",params![session,role,host,boot,key,now(db)?])?;
            let id=db.last_insert_rowid();
            db.execute("INSERT INTO hub_turn_profiles(turn_id,profile_id) VALUES(?1,?2)",params![id,profile])?;
            db.execute("UPDATE messages SET turn_id=?2,queued=0 WHERE id=?1",params![input,id])?;
            publish(db,input)?;
            Ok(Some(HubTurn{id,session,profile,model,repo,summary,input:message(db,input)?,mcp_token:key}))
        })
    }

    pub fn hub_authorize(&self, turn: i64, process: Option<(u32, String)>) -> Result<()> {
        self.write(move|db|{
            let (pid,start)=process.map(|(pid,start)|(Some(pid.to_string()),Some(start))).unwrap_or_default();
            if db.execute("UPDATE turns SET phase='authorized',os_proc_id=?2,proc_started_at=?3
                WHERE id=?1 AND phase='prepared' AND status='running' AND mode IN ('hub','manager')",params![turn,pid,start])?!=1 {
                return Err(Error::ConflictError);
            }
            Ok(())
        })
    }

    /// Snapshot each delivered message individually. A queued input may have a
    /// lower ID than an assistant reply and must not be skipped by a high cursor.
    pub fn hub_delta(&self, claim: &HubTurn, rebuild: bool) -> Result<ThreadDelta> {
        self.read(|db|{
            let state:Option<(String,i64)>=db.query_row("SELECT agent_thread_id,last_synced_msg_id FROM hub_provider_threads
                WHERE session_id=?1 AND profile_id=?2",params![claim.session,claim.profile],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
            let state=if rebuild{None}else{state};
            let delta=state.is_some();
            let mut q=db.prepare(&format!("SELECT {MESSAGE_COLUMNS} FROM messages m WHERE m.session_id=?1 AND m.queued=0
                AND (NOT ?3 OR NOT EXISTS (SELECT 1 FROM hub_thread_ack a WHERE a.session_id=?1 AND a.profile_id=?2 AND a.message_id=m.id))
                ORDER BY m.id"))?;
            let messages=q.query_map(params![claim.session,claim.profile,delta],row)?.collect::<rusqlite::Result<_>>()?;
            Ok(match state{
                Some((handle,last_synced))=>ThreadDelta{handle:Some(handle),last_synced,messages},
                None=>ThreadDelta{handle:None,last_synced:0,messages},
            })
        })
    }

    pub fn hub_record_handle(
        &self,
        session: &str,
        profile: &str,
        turn: i64,
        handle: &str,
    ) -> Result<()> {
        let (session, profile, handle) =
            (session.to_owned(), profile.to_owned(), handle.to_owned());
        self.write(move|db|{
            let valid=db.query_row("SELECT EXISTS(SELECT 1 FROM turns WHERE id=?1 AND session_id=?2 AND status='running' AND phase='authorized')",
                params![turn,session],|r|r.get::<_,bool>(0))?;
            if !valid{return Err(Error::ConflictError);}
            db.execute("INSERT INTO hub_provider_threads(session_id,profile_id,agent_thread_id,last_synced_msg_id,updated_at)
                VALUES(?1,?2,?3,0,?4) ON CONFLICT(session_id,profile_id) DO NOTHING",params![session,profile,handle,now(db)?])?;
            Ok(())
        })
    }

    /// A provider explicitly refused to open the native thread. Rebuild under
    /// a new launch grant without duplicating the user's canonical message.
    pub fn hub_replace_missing_turn(&self, mut claim: HubTurn) -> Result<HubTurn> {
        self.write(move|db|{
            let key=uuid::Uuid::new_v4().simple().to_string();
            if db.execute("UPDATE turns SET status='failed',end_reason='exit',writer_check='excluded',
                mcp_revoked=1,ended_at=?2 WHERE id=?1 AND status='running'",params![claim.id,now(db)?])?!=1 {
                return Err(Error::ConflictError);
            }
            db.execute("INSERT INTO turns(session_id,mode,status,phase,host_instance_id,boot_epoch,mcp_token,started_at)
                SELECT session_id,mode,'running','prepared',host_instance_id,boot_epoch,?2,?3 FROM turns WHERE id=?1",
                params![claim.id,key,now(db)?])?;
            claim.id=db.last_insert_rowid();
            claim.mcp_token=key;
            db.execute("INSERT INTO hub_turn_profiles(turn_id,profile_id) VALUES(?1,?2)",params![claim.id,claim.profile])?;
            db.execute("UPDATE messages SET turn_id=?2 WHERE id=?1",params![claim.input.id,claim.id])?;
            claim.input=message(db,claim.input.id)?;
            publish(db,claim.input.id)?;
            Ok(claim)
        })
    }

    pub fn hub_finish(&self, claim: HubTurn, reply: HubCompletion) -> Result<HubMessage> {
        self.write(move|db|{
            if db.execute("UPDATE turns SET status='completed',end_reason='exit',writer_check='excluded',mcp_revoked=1,ended_at=?2
                WHERE id=?1 AND status='running' AND phase='authorized'",params![claim.id,now(db)?])?!=1{
                return Err(Error::ConflictError);
            }
            db.execute("INSERT INTO messages(session_id,turn_id,source,provider_label,text,reasoning,metadata_json,created_at)
                VALUES(?1,?2,'agent',?3,?4,?5,?6,?7)",params![claim.session,claim.id,claim.profile,reply.text,
                reply.reasoning,reply.metadata.map(|v|v.to_string()),now(db)?])?;
            let id=db.last_insert_rowid();
            if let Some(thread)=reply.native_thread {
                if reply.rebuilt {
                    db.execute("DELETE FROM hub_thread_ack WHERE session_id=?1 AND profile_id=?2",params![claim.session,claim.profile])?;
                }
                db.execute("INSERT INTO hub_provider_threads(session_id,profile_id,agent_thread_id,last_synced_msg_id,updated_at)
                    VALUES(?1,?2,?3,?4,?5) ON CONFLICT(session_id,profile_id) DO UPDATE
                    SET agent_thread_id=excluded.agent_thread_id,last_synced_msg_id=excluded.last_synced_msg_id,updated_at=excluded.updated_at",
                    params![claim.session,claim.profile,thread,id,now(db)?])?;
                for delivered in reply.delivered.into_iter().chain([id]){
                    db.execute("INSERT OR IGNORE INTO hub_thread_ack(session_id,profile_id,message_id)
                        SELECT ?1,?2,id FROM messages WHERE id=?3 AND session_id=?1 AND queued=0",params![claim.session,claim.profile,delivered])?;
                }
            }
            publish(db,id)?;
            message(db,id)
        })
    }

    pub fn hub_end_without_reply(&self, turn: i64, reason: &str) -> Result<()> {
        if !matches!(
            reason,
            "never_started" | "cancel" | "host_lost" | "exit" | "dispatch_closed"
        ) {
            return Err(Error::Invalid("invalid Hub end reason"));
        }
        let reason = reason.to_owned();
        self.write(move |db| end_without_reply(db, turn, &reason, false))
    }

    /// Worker recovery has already classified and cleared the actual release markers.
    pub fn hub_recover_inputs(&self) -> Result<()> {
        self.write(|db|{
            let mut q=db.prepare("SELECT t.id,t.end_reason FROM turns t WHERE t.mode IN ('hub','manager') AND
                t.end_reason IN ('never_started','host_lost') AND NOT EXISTS (SELECT 1 FROM hub_input_recovery a WHERE a.turn_id=t.id)
                ORDER BY t.id")?;
            let stale=q.query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
            for (turn,why) in stale {end_without_reply(db,turn,&why,true)?;}
            Ok(())
        })
    }
}

fn end_without_reply(db: &Connection, turn: i64, reason: &str, recovered: bool) -> Result<()> {
    let inserted = db.execute(
        "INSERT OR IGNORE INTO hub_input_recovery(turn_id) SELECT id FROM turns
        WHERE id=?1 AND mode IN ('hub','manager') AND (status='running' OR ?2)",
        params![turn, recovered],
    )?;
    if inserted == 0 {
        return Ok(());
    }
    if !recovered {
        let status = match reason {
            "exit" => "failed",
            "cancel" => "cancelled",
            _ => "interrupted",
        };
        db.execute("UPDATE turns SET status=?2,end_reason=?3,mcp_revoked=1,writer_check='excluded',ended_at=?4 WHERE id=?1",
            params![turn,status,reason,now(db)?])?;
    }
    if matches!(reason, "never_started" | "dispatch_closed") {
        let input: Option<i64> = db
            .query_row(
                "SELECT id FROM messages WHERE turn_id=?1 AND source='user'",
                [turn],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = input {
            db.execute(
                "UPDATE messages SET queued=1,turn_id=NULL WHERE id=?1",
                [id],
            )?;
            publish(db, id)?;
        }
    } else if reason == "host_lost" {
        db.execute("INSERT OR IGNORE INTO hub_thread_ack(session_id,profile_id,message_id)
            SELECT t.session_id,p.profile_id,m.id FROM turns t JOIN hub_turn_profiles p ON p.turn_id=t.id
            JOIN hub_provider_threads h ON h.session_id=t.session_id AND h.profile_id=p.profile_id
            JOIN messages m ON m.turn_id=t.id AND m.source='user' WHERE t.id=?1",[turn])?;
        db.execute("INSERT INTO messages(session_id,turn_id,source,text,metadata_json,created_at)
            SELECT session_id,id,'host','The prior conversation was interrupted.',?2,?3 FROM turns WHERE id=?1",
            params![turn,json!({"kind":"hub_interrupted"}).to_string(),now(db)?])?;
        publish(db, db.last_insert_rowid())?;
    }
    Ok(())
}

impl Store {
    pub fn register_manager(
        &self,
        hub: &str,
        profile: &str,
        model: &str,
        name: &str,
    ) -> Result<String> {
        if name.trim().is_empty() || name.len() > 120 {
            return Err(Error::Invalid("invalid advisor name"));
        }
        let (hub, profile, model, name) = (
            hub.to_owned(),
            profile.to_owned(),
            model.to_owned(),
            name.to_owned(),
        );
        self.write(move|db|{
            let repo:String=db.query_row("SELECT repo_cwd FROM sessions WHERE id=?1 AND role='hub'",[&hub],|r|r.get(0))
                .optional()?.ok_or(Error::NotFound)?;
            let (kind,models):(String,String)=db.query_row("SELECT kind,models_json FROM provider_profiles WHERE id=?1",[&profile],
                |r|Ok((r.get(0)?,r.get(1)?))).optional()?.ok_or(Error::NotFound)?;
            let allowed:Vec<String>=serde_json::from_str(&models)?;
            if !allowed.contains(&model){return Err(Error::ConflictError);}
            let existing:Option<String>=db.query_row("SELECT id FROM sessions WHERE role='manager' AND repo_cwd=?1 AND profile_id=?2 AND provider_model=?3",
                params![repo,profile,model],|r|r.get(0)).optional()?;
            if let Some(id)=existing{return Ok(id);}
            let id=crate::new_id('s');
            let vendor=match kind.as_str(){"codex_cli"=>Some("codex"),"claude_cli"=>Some("claude"),_=>None};
            db.execute("INSERT INTO sessions(id,name,role,vendor,repo_cwd,profile_id,provider_model,created_at)
                VALUES(?1,?2,'manager',?3,?4,?5,?6,?7)",
                params![id,name,vendor,repo,profile,model,now(db)?])?;
            Ok(id)
        })
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct TicketIntent {
    pub id: String,
    pub hub: String,
    pub repo: String,
    pub used: u32,
    pub limit: u32,
    pub status: String,
}
impl Store {
    pub fn hub_ticket(
        &self,
        hub: &str,
        source: i64,
        title: &str,
        mandate: Value,
        max: u32,
    ) -> Result<TicketIntent> {
        if !(1..=20).contains(&max)
            || title.trim().is_empty()
            || title.len() > 256
            || mandate.to_string().len() > 16_384
        {
            return Err(Error::Invalid("invalid Ticket mandate"));
        }
        let (hub, title) = (hub.to_owned(), title.to_owned());
        self.write(move|db|{
            let repo:String=db.query_row("SELECT s.repo_cwd FROM sessions s JOIN messages m ON m.session_id=s.id
                WHERE s.id=?1 AND s.role='hub' AND m.id=?2 AND m.source='user'",params![hub,source],|r|r.get(0)).optional()?.ok_or(Error::ConflictError)?;
            let existing:Option<(String,String,String,u32)>=db.query_row("SELECT id,title,mandate_json,max_runs FROM tickets WHERE basis_msg_id=?1",
                [source],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
            let data=mandate.to_string();
            if let Some((id,prior_title,prior_data,prior_budget))=existing{
                if prior_title!=title || prior_data!=data || prior_budget!=max{return Err(Error::ConflictError);}
                return load_ticket(db,&id);
            }
            let id=crate::new_id('t');
            let stamp=now(db)?;
            db.execute("INSERT INTO tickets(id,hub_session_id,repo_cwd,title,mandate_json,source_kind,basis_msg_id,max_runs,status,created_at,updated_at)
                VALUES(?1,?2,?3,?4,?5,'explicit_user',?6,?7,'active',?8,?8)",params![id,hub,repo,title,data,source,max,stamp])?;
            publish_ticket(db,&id)
        })
    }
    pub fn hub_ticket_for_dispatch(
        &self,
        hub: &str,
        ticket: &str,
        repo: &str,
    ) -> Result<TicketIntent> {
        let (hub, ticket, repo) = (hub.to_owned(), ticket.to_owned(), repo.to_owned());
        let checked = self.write(move |db| {
            let t = load_ticket(db, &ticket)?;
            if t.hub != hub || t.status != "active" {
                return Ok(None);
            }
            if t.repo != repo || t.used >= t.limit {
                db.execute(
                    "UPDATE tickets SET status='blocked_on_user',updated_at=?2 WHERE id=?1",
                    params![ticket, now(db)?],
                )?;
                publish_ticket(db, &ticket)?;
                return Ok(None);
            }
            Ok(Some(t))
        })?;
        checked.ok_or(Error::ConflictError)
    }
    pub fn hub_close_ticket_budget(&self, ticket: &str) -> Result<()> {
        let ticket = ticket.to_owned();
        self.write(move |db| {
            let t = load_ticket(db, &ticket)?;
            if t.status == "active" && t.used >= t.limit {
                db.execute(
                    "UPDATE tickets SET status='blocked_on_user',updated_at=?2 WHERE id=?1",
                    params![ticket, now(db)?],
                )?;
                publish_ticket(db, &ticket)?;
            }
            Ok(())
        })
    }
    pub fn hub_ticket_mandate(&self, id: &str) -> Result<(TicketIntent, Value)> {
        self.read(|db| {
            let t = load_ticket(db, id)?;
            let mandate =
                db.query_row("SELECT mandate_json FROM tickets WHERE id=?1", [id], |r| {
                    r.get::<_, String>(0)
                })?;
            Ok((t, serde_json::from_str(&mandate)?))
        })
    }
    /// Consume only a first, canonical terminal Run receipt. External input never
    /// supplies the receipt body or gets to select a different Ticket.
    pub fn hub_worker_receipt(&self, run: &str) -> Result<Option<String>> {
        let run = run.to_owned();
        self.write(move|db|{
            let row:Option<(String,String,String,Option<String>)>=db.query_row(
                "SELECT hub_session_id,ticket_id,status,result_sha FROM runs WHERE id=?1
                 AND status IN ('done','failed','needs_review','interrupted')",
                 [&run],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
            let Some((hub,ticket,status,sha))=row else{return Ok(None);};
            let changed=db.execute("INSERT OR IGNORE INTO hub_consumed_receipts(run_id,status) VALUES(?1,?2)",params![run,status])?;
            if changed==0{return Ok(None);}
            let payload=json!({"kind":"worker_receipt","run_id":run,"ticket_id":ticket,"status":status,"sha":sha});
            db.execute("INSERT INTO messages(session_id,source,text,metadata_json,created_at,queued)
                VALUES(?1,'host','Host verified Worker status',?2,?3,1)",params![hub,payload.to_string(),now(db)?])?;
            publish(db,db.last_insert_rowid())?;
            Ok(Some(hub))
        })
    }
    pub fn hub_inference_policy(&self, repo: &str) -> Result<(String, Vec<String>, Vec<String>)> {
        self.read(|db|{
            let saved:Option<(String,String,String)>=db.query_row(
                "SELECT inferred_mode,bot_actors_json,human_actors_json FROM admission_policies WHERE repo_cwd=?1",
                [repo],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
            let Some((mode,bots,humans))=saved else{return Ok(("observe".into(),vec![],vec![]));};
            Ok((mode,serde_json::from_str(&bots)?,serde_json::from_str(&humans)?))
        })
    }

    pub fn hub_scope(&self, session: &str) -> Result<(String, String, String)> {
        self.read(|db|db.query_row("SELECT s.role,s.repo_cwd,p.kind FROM sessions s JOIN provider_profiles p ON s.profile_id=p.id
            WHERE s.id=?1 AND s.role IN ('hub','manager')",[session],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))
            .optional()?.ok_or(Error::NotFound))
    }
    pub fn hub_idle_sessions(&self) -> Result<Vec<String>> {
        self.read(|db|{
            let mut q=db.prepare("SELECT DISTINCT m.session_id FROM messages m
                 JOIN sessions s ON s.id=m.session_id WHERE m.queued=1 AND m.turn_id IS NULL AND s.role IN ('hub','manager')")?;
            let v=q.query_map([],|r|r.get::<_,String>(0))?.collect::<rusqlite::Result<_>>()?;
            Ok(v)
        })
    }
    pub fn hub_tool_scope(
        &self,
        credential: &str,
        instance: &str,
    ) -> Result<Option<(i64, String, String, String)>> {
        self.read(|db|Ok(db.query_row("SELECT t.id,s.id,s.role,s.repo_cwd FROM turns t JOIN sessions s ON t.session_id=s.id
            WHERE t.host_instance_id=?1 AND t.mcp_token=?2 AND t.status='running' AND t.phase='authorized'
            AND t.mcp_revoked=0 AND t.run_id IS NULL AND t.mode=s.role AND s.role IN ('hub','manager')",
            params![instance,credential],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?))
    }
}
fn load_ticket(db: &Connection, id: &str) -> Result<TicketIntent> {
    db.query_row(
        "SELECT id,hub_session_id,repo_cwd,runs_used,max_runs,status FROM tickets WHERE id=?1",
        [id],
        |r| {
            Ok(TicketIntent {
                id: r.get(0)?,
                hub: r.get(1)?,
                repo: r.get(2)?,
                used: r.get(3)?,
                limit: r.get(4)?,
                status: r.get(5)?,
            })
        },
    )
    .optional()?
    .ok_or(Error::NotFound)
}
fn publish_ticket(db: &Connection, id: &str) -> Result<TicketIntent> {
    let t = load_ticket(db, id)?;
    Store::append_change(db, ChangeTopic::Ticket, id, &serde_json::to_value(&t)?)?;
    Ok(t)
}

impl Store {
    pub fn builtin_profiles(&self) -> Result<()> {
        self.write(|db|{
            for (id,label,kind,base,account,models) in [
                ("builtin-hub","Codex CLI","codex_cli",None,None,"[\"default\"]"),
                ("claude-cli","Claude CLI","claude_cli",None,None,"[\"default\"]"),
                ("deepseek-api","DeepSeek API","openai_compat",Some("https://api.deepseek.com"),Some("deepseek"),"[\"deepseek-v4.1\"]"),
            ] {
                db.execute("INSERT OR IGNORE INTO provider_profiles(id,display_name,kind,base_url,keyring_account,models_json,created_at)
                    VALUES(?1,?2,?3,?4,?5,?6,?7)",params![id,label,kind,base,account,models,now(db)?])?;
            }
            Ok(())
        })
    }
    pub fn hub_profiles(&self) -> Result<Vec<Value>> {
        self.read(|db|{
            let mut q=db.prepare("SELECT id,display_name,kind,models_json FROM provider_profiles ORDER BY id")?;
            let rows=q.query_map([],|r|{
                let models:Vec<String>=crate::parse_json(r.get(3)?,3)?;
                Ok(json!({"id":r.get::<_,String>(0)?,"display_name":r.get::<_,String>(1)?,"kind":r.get::<_,String>(2)?,"models":models}))
            })?.collect::<rusqlite::Result<_>>()?;
            Ok(rows)
        })
    }
    pub fn hub_switch_profile(&self, session: &str, profile: &str, model: &str) -> Result<()> {
        let (session, profile, model) = (session.to_owned(), profile.to_owned(), model.to_owned());
        self.write(move|db|{
            let models:String=db.query_row("SELECT models_json FROM provider_profiles WHERE id=?1",[&profile],|r|r.get(0))
                .optional()?.ok_or(Error::NotFound)?;
            if !serde_json::from_str::<Vec<String>>(&models)?.contains(&model){return Err(Error::ConflictError);}
            let running=db.query_row("SELECT EXISTS(SELECT 1 FROM turns WHERE session_id=?1 AND status='running')",
                [&session],|r|r.get::<_,bool>(0))?;
            if running {return Err(Error::BusyError);}
            if db.execute("UPDATE sessions SET profile_id=?2,provider_model=?3 WHERE id=?1 AND role IN ('hub','manager')",
                params![session,profile,model])?!=1{return Err(Error::NotFound);}
            Ok(())
        })
    }
}
