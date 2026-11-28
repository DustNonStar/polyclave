use crate::{enum_value, now, Error, Result};
use polyclave_proto::{ChangeTopic, SseEnvelope};
use rusqlite::{params, Connection, OpenFlags, Transaction, TransactionBehavior};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

type Job = Box<dyn FnOnce(&mut Connection) + Send + 'static>;
const KEEP_CHANGES: i64 = 10_000;
const MAX_IDLE_READERS: usize = 4;

#[derive(Clone)]
pub struct Store(Arc<Inner>);

struct Inner {
    path: PathBuf,
    sender: Option<mpsc::SyncSender<Job>>,
    thread: Option<JoinHandle<()>>,
    readers: Mutex<Vec<Connection>>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(worker) = self.thread.take() {
            // Normally owned by the host. Never join the current thread if a
            // job held the final Store handle.
            if worker.thread().id() != std::thread::current().id() {
                let _ = worker.join();
            }
        }
    }
}

fn configure(c: &Connection) -> Result<()> {
    c.busy_timeout(Duration::from_millis(5000))?;
    c.pragma_update(None, "foreign_keys", "ON")?;
    Ok(())
}

#[derive(Debug)]
pub enum Changes {
    Events(Vec<SseEnvelope>),
    ResyncRequired,
}

impl Store {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut db = Connection::open(&path)?;
        configure(&db)?;
        let mode: String = db.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
        if mode != "wal" {
            return Err(Error::Invalid("SQLite WAL mode is required"));
        }
        db.pragma_update(None, "synchronous", "FULL")?;
        let migration = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let version: i64 = migration.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version == 0 {
            migration.execute_batch(include_str!("schema.sql"))?;
            migration.pragma_update(None, "user_version", 1)?;
        }
        if version <= 1 {
            migration.execute_batch("ALTER TABLE turns ADD COLUMN mcp_revoked INTEGER NOT NULL DEFAULT 0 CHECK (mcp_revoked IN (0, 1));")?;
            migration.pragma_update(None, "user_version", 2)?;
        }
        if version <= 2 {
            migration.execute_batch("
                CREATE TABLE hub_input_requests(
                    session_id TEXT NOT NULL REFERENCES sessions(id),request_id TEXT NOT NULL,
                    message_id INTEGER NOT NULL UNIQUE REFERENCES messages(id),PRIMARY KEY(session_id,request_id));
                CREATE TABLE hub_input_recovery(turn_id INTEGER PRIMARY KEY REFERENCES turns(id));
                CREATE TABLE hub_turn_profiles(turn_id INTEGER PRIMARY KEY REFERENCES turns(id),profile_id TEXT NOT NULL REFERENCES provider_profiles(id));
                CREATE TABLE hub_thread_ack(
                    session_id TEXT NOT NULL,profile_id TEXT NOT NULL,message_id INTEGER NOT NULL REFERENCES messages(id),
                    FOREIGN KEY(session_id,profile_id) REFERENCES hub_provider_threads(session_id,profile_id),
                    PRIMARY KEY(session_id,profile_id,message_id));")?;
            migration.pragma_update(None, "user_version", 3)?;
        }
        if version <= 3 {
            migration.execute_batch("
                CREATE TABLE hub_consumed_receipts(run_id TEXT NOT NULL REFERENCES runs(id),status TEXT NOT NULL,
                    PRIMARY KEY(run_id,status));
                ALTER TABLE admission_policies ADD COLUMN human_actors_json TEXT NOT NULL DEFAULT '[]';")?;
            migration.pragma_update(None, "user_version", 4)?;
        }
        if version <= 4 {
            migration.execute_batch(
                "CREATE TABLE hub_api_replay(
                message_id INTEGER PRIMARY KEY REFERENCES messages(id),
                transcript_json TEXT NOT NULL);",
            )?;
            migration.pragma_update(None, "user_version", 5)?;
        }
        if version <= 5 {
            // Stage 2: additive and idempotent.
            migration.execute_batch(crate::stage2::SCHEMA)?;
            migration.pragma_update(None, "user_version", 6)?;
        }
        if version <= 6 {
            // Stage 3: additive and idempotent.
            migration.execute_batch(crate::stage3::SCHEMA)?;
            migration.pragma_update(None, "user_version", 7)?;
        }
        if version <= 7 {
            // Stage 4: additive and idempotent.
            migration.execute_batch(crate::stage4::SCHEMA)?;
            migration.pragma_update(None, "user_version", 8)?;
        }
        if version <= 8 {
            migration.execute_batch(
                "CREATE TRIGGER IF NOT EXISTS memory_items_invalidate_emb
                 AFTER UPDATE OF title, content, tags_json ON memory_items
                 BEGIN
                   UPDATE memory_items SET embedding_blob = NULL WHERE rowid = new.rowid;
                 END;",
            )?;
            migration.pragma_update(None, "user_version", 9)?;
        }
        if version <= 9 {
            // Origin snapshot for the PR gate publish step: additive.
            migration.execute_batch(crate::publish::SCHEMA)?;
            migration.pragma_update(None, "user_version", 10)?;
        } else if version != 10 {
            return Err(Error::Invalid("unsupported database schema"));
        }
        // Additive upgrades that check the live schema instead of user_version,
        // so they apply once to every v10 ledger, new or existing.
        crate::attempts::upgrade(&migration)?;
        crate::stage2::upgrade(&migration)?;
        crate::mesh_limits::upgrade(&migration)?;
        crate::remote_dispatch::upgrade(&migration)?;
        crate::scm_cursors::upgrade(&migration)?;
        crate::test_results::upgrade(&migration)?;
        migration.commit()?;
        let (tx, rx) = mpsc::sync_channel::<Job>(128);
        let thread = std::thread::Builder::new()
            .name("polyclave-sqlite-writer".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    job(&mut db);
                }
            })
            .map_err(|_| Error::Closed)?;
        Ok(Self(Arc::new(Inner {
            path,
            sender: Some(tx),
            thread: Some(thread),
            readers: Mutex::new(Vec::new()),
        })))
    }

    pub fn write<T, F>(&self, job: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&Transaction<'_>) -> Result<T> + Send + 'static,
    {
        if self
            .0
            .thread
            .as_ref()
            .is_some_and(|t| t.thread().id() == std::thread::current().id())
        {
            return Err(Error::Invalid("recursive writer job"));
        }
        let (tx, rx) = mpsc::sync_channel(1);
        self.0
            .sender
            .as_ref()
            .ok_or(Error::Closed)?
            .send(Box::new(move |db| {
                let result = (|| {
                    let tr = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
                    let result = job(&tr)?;
                    tr.commit()?;
                    Ok(result)
                })();
                let _ = tx.send(result);
            }))
            .map_err(|_| Error::Closed)?;
        rx.recv().map_err(|_| Error::Closed)?
    }

    pub fn read<T>(&self, job: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let db = self.0.readers.lock().map_err(|_| Error::Closed)?.pop();
        let db = if let Some(db) = db {
            db
        } else {
            let db = Connection::open_with_flags(&self.0.path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            configure(&db)?;
            db
        };
        let output = job(&db);
        let mut pool = self.0.readers.lock().map_err(|_| Error::Closed)?;
        if pool.len() < MAX_IDLE_READERS {
            pool.push(db);
        }
        output
    }

    pub fn changes(&self, since: Option<i64>) -> Result<Changes> {
        self.read(|db| {
            let (min, max): (Option<i64>, Option<i64>) =
                db.query_row("SELECT MIN(seq), MAX(seq) FROM change_log", [], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })?;
            let (Some(min), Some(max)) = (min, max) else {
                return Ok(if since.is_some_and(|s| s != 0) {
                    Changes::ResyncRequired
                } else {
                    Changes::Events(Vec::new())
                });
            };
            if since.is_some_and(|s| s < min - 1 || s > max) {
                return Ok(Changes::ResyncRequired);
            }
            let mut stmt = db.prepare(
                "SELECT seq,topic,entity_id,payload_json,created_at
                 FROM change_log WHERE seq > ?1 ORDER BY seq",
            )?;
            let mut rows = stmt.query([since.unwrap_or(0)])?;
            let mut all = Vec::new();
            while let Some(r) = rows.next()? {
                all.push(SseEnvelope {
                    seq: r.get(0)?,
                    topic: crate::decode_enum(&r.get::<_, String>(1)?)?,
                    entity_id: r.get(2)?,
                    payload: crate::parse_json(r.get(3)?, 3)?,
                    created_at: r.get(4)?,
                });
            }
            Ok(Changes::Events(all))
        })
    }

    pub fn append_change(
        db: &Connection,
        topic: ChangeTopic,
        entity_id: &str,
        payload: &Value,
    ) -> Result<i64> {
        fn sensitive(v: &Value) -> bool {
            match v {
                Value::Object(o) => {
                    [
                        "mcp_token",
                        "local_auth_token",
                        "bot_token",
                        "reviewer_token",
                    ]
                    .iter()
                    .any(|k| o.contains_key(*k))
                        || o.values().any(sensitive)
                }
                Value::Array(xs) => xs.iter().any(sensitive),
                _ => false,
            }
        }
        if sensitive(payload) {
            return Err(Error::Invalid("secret field in public event"));
        }
        db.execute(
            "INSERT INTO change_log(topic,entity_id,payload_json,created_at) VALUES(?1,?2,?3,?4)",
            params![
                enum_value(topic)?,
                entity_id,
                serde_json::to_string(payload)?,
                now(db)?
            ],
        )?;
        let seq = db.last_insert_rowid();
        db.execute(
            "DELETE FROM change_log WHERE seq <= COALESCE((SELECT seq FROM change_log
                   ORDER BY seq DESC LIMIT 1 OFFSET ?1), 0)",
            [KEEP_CHANGES],
        )?;
        Ok(seq)
    }
}
