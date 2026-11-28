
CREATE TABLE provider_profiles (
  id              TEXT PRIMARY KEY,
  display_name    TEXT NOT NULL,
  kind            TEXT NOT NULL CHECK (kind IN ('openai_compat', 'anthropic_compat', 'codex_cli', 'claude_cli')),
  base_url        TEXT,
  keyring_account TEXT,
  models_json     TEXT NOT NULL,
  created_at      TEXT NOT NULL
);

CREATE TABLE sessions (
  id              TEXT PRIMARY KEY,
  name            TEXT NOT NULL,
  role            TEXT NOT NULL CHECK (role IN ('hub', 'manager', 'worker')),
  vendor          TEXT,
  repo_cwd        TEXT NOT NULL,
  profile_id      TEXT NOT NULL REFERENCES provider_profiles(id),
  provider_model  TEXT NOT NULL,
  rolling_summary TEXT NOT NULL DEFAULT '',
  agent_thread_id TEXT,
  created_at      TEXT NOT NULL
);

CREATE TABLE hub_provider_threads (
  session_id         TEXT NOT NULL REFERENCES sessions(id),
  profile_id         TEXT NOT NULL REFERENCES provider_profiles(id),
  agent_thread_id    TEXT NOT NULL,
  last_synced_msg_id INTEGER NOT NULL DEFAULT 0,
  updated_at         TEXT NOT NULL,
  PRIMARY KEY (session_id, profile_id)
);

CREATE TABLE messages (
  id              INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id      TEXT NOT NULL REFERENCES sessions(id),
  turn_id         INTEGER REFERENCES turns(id),
  source          TEXT NOT NULL CHECK (source IN ('user', 'agent', 'host')),
  client_kind     TEXT CHECK (client_kind IN ('desktop', 'web', 'ios', 'android', 'cli')),
  provider_label  TEXT,
  reasoning       TEXT,
  text            TEXT NOT NULL,
  metadata_json   TEXT,
  queued          INTEGER NOT NULL DEFAULT 0 CHECK (queued IN (0, 1)),
  created_at      TEXT NOT NULL
);

CREATE TABLE admission_policies (
  repo_cwd        TEXT PRIMARY KEY,
  inferred_mode   TEXT NOT NULL DEFAULT 'observe' CHECK (inferred_mode IN ('off', 'observe', 'auto')),
  bot_actors_json TEXT NOT NULL DEFAULT '[]',
  updated_at      TEXT NOT NULL
);

CREATE TABLE tickets (
  id              TEXT PRIMARY KEY,
  hub_session_id  TEXT NOT NULL REFERENCES sessions(id),
  repo_cwd        TEXT NOT NULL,
  title           TEXT NOT NULL,
  mandate_json    TEXT NOT NULL,
  source_kind     TEXT NOT NULL CHECK (source_kind IN ('explicit_user', 'inferred_event')),
  basis_msg_id    INTEGER UNIQUE REFERENCES messages(id),
  max_runs        INTEGER NOT NULL DEFAULT 4 CHECK (max_runs BETWEEN 1 AND 20),
  runs_used       INTEGER NOT NULL DEFAULT 0 CHECK (runs_used >= 0),
  status          TEXT NOT NULL CHECK (status IN ('proposed', 'active', 'blocked_on_user', 'done', 'cancelled')),
  created_at      TEXT NOT NULL,
  updated_at      TEXT NOT NULL
);

CREATE TABLE runs (
  id                  TEXT PRIMARY KEY,
  ticket_id           TEXT NOT NULL REFERENCES tickets(id),
  hub_session_id      TEXT NOT NULL REFERENCES sessions(id),
  worker_session_id   TEXT NOT NULL UNIQUE REFERENCES sessions(id),
  request_id          TEXT NOT NULL,
  repo_cwd            TEXT NOT NULL,
  vendor              TEXT NOT NULL CHECK (vendor IN ('codex', 'claude')),
  model               TEXT,
  instructions        TEXT NOT NULL,
  context_refs_json   TEXT,
  mirror_remote       TEXT NOT NULL,
  base_branch         TEXT NOT NULL,
  base_sha            TEXT NOT NULL,
  status              TEXT NOT NULL CHECK (status IN ('pending', 'running', 'done', 'interrupted', 'failed', 'needs_review')),
  current_attempt     INTEGER NOT NULL DEFAULT 0,
  max_attempts        INTEGER NOT NULL DEFAULT 3,
  accepted_attempt_id INTEGER,
  detail              TEXT,
  result_sha          TEXT,
  observed_sha        TEXT,
  result_source       TEXT CHECK (result_source IN ('agent', 'user')),
  resolution          TEXT,
  created_at          TEXT NOT NULL,
  updated_at          TEXT NOT NULL
);
CREATE UNIQUE INDEX idx_runs_idempotent_req ON runs(hub_session_id, request_id);

CREATE TABLE attempts (
  id               INTEGER PRIMARY KEY AUTOINCREMENT,
  run_id           TEXT NOT NULL REFERENCES runs(id),
  attempt_no       INTEGER NOT NULL,
  from_sha         TEXT,
  branch           TEXT NOT NULL,
  worktree_path    TEXT NOT NULL,
  agent_thread_id  TEXT,
  corrections      INTEGER NOT NULL DEFAULT 0,
  status           TEXT NOT NULL CHECK (status IN ('prepared', 'running', 'completed', 'cancelled', 'superseded', 'interrupted', 'failed')),
  created_at       TEXT NOT NULL,
  ended_at         TEXT,
  UNIQUE(run_id, attempt_no)
);

CREATE TABLE turns (
  id               INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id       TEXT NOT NULL REFERENCES sessions(id),
  run_id           TEXT REFERENCES runs(id),
  attempt_id       INTEGER REFERENCES attempts(id),
  parent_turn_id   INTEGER REFERENCES turns(id),
  mode             TEXT NOT NULL CHECK (mode IN ('hub', 'manager', 'worker_chat', 'start', 'correction', 'retry', 'consult')),
  status           TEXT NOT NULL CHECK (status IN ('running', 'completed', 'cancelled', 'interrupted', 'failed')),
  phase            TEXT NOT NULL CHECK (phase IN ('prepared', 'authorized')),
  host_instance_id TEXT NOT NULL,
  boot_epoch       TEXT NOT NULL,
  os_proc_id       TEXT,
  proc_started_at  TEXT,
  mcp_token        TEXT NOT NULL UNIQUE,
  exit_code        INTEGER,
  end_reason       TEXT CHECK (end_reason IN ('exit', 'cancel', 'host_lost', 'never_started', 'dispatch_closed')),
  writer_check     TEXT CHECK (writer_check IN ('excluded', 'possible')),
  started_at       TEXT NOT NULL,
  ended_at         TEXT,
  CHECK (
    (mode IN ('worker_chat', 'start', 'correction', 'retry') AND run_id IS NOT NULL AND attempt_id IS NOT NULL)
    OR (mode IN ('hub', 'manager') AND run_id IS NULL)
    OR (mode = 'consult' AND parent_turn_id IS NOT NULL)
  )
);

CREATE UNIQUE INDEX idx_turns_running_session
  ON turns(session_id) WHERE status = 'running' AND run_id IS NULL AND mode != 'consult';
CREATE UNIQUE INDEX idx_turns_running_run
  ON turns(run_id) WHERE status = 'running' AND run_id IS NOT NULL AND mode != 'consult';

CREATE TABLE results (
  id               INTEGER PRIMARY KEY AUTOINCREMENT,
  run_id           TEXT NOT NULL REFERENCES runs(id),
  attempt_id       INTEGER NOT NULL REFERENCES attempts(id),
  turn_id          INTEGER NOT NULL REFERENCES turns(id),
  accepted         INTEGER NOT NULL CHECK (accepted IN (0, 1)),
  payload_json     TEXT NOT NULL,
  error            TEXT,
  created_at       TEXT NOT NULL
);
CREATE UNIQUE INDEX idx_results_one_accepted_per_attempt ON results(attempt_id) WHERE accepted = 1;

CREATE TABLE memory_items (
  id              TEXT PRIMARY KEY,
  scope           TEXT NOT NULL CHECK (scope IN ('global', 'project', 'run')),
  repo_cwd        TEXT,
  kind            TEXT NOT NULL CHECK (kind IN ('decision', 'constraint', 'progress', 'codebase_fact', 'pitfall', 'preference', 'receipt')),
  title           TEXT NOT NULL,
  content         TEXT NOT NULL,
  tags_json       TEXT NOT NULL DEFAULT '[]',
  evidence_json   TEXT,
  status          TEXT NOT NULL CHECK (status IN ('candidate', 'verified', 'superseded', 'archived')),
  supersedes_id   TEXT REFERENCES memory_items(id),
  created_by      TEXT NOT NULL,
  source_run_id   TEXT REFERENCES runs(id),
  source_turn_id  INTEGER REFERENCES turns(id),
  embedding_blob  BLOB,
  created_at      TEXT NOT NULL,
  updated_at      TEXT NOT NULL,
  CHECK ((scope = 'global') OR (repo_cwd IS NOT NULL))
);
CREATE INDEX idx_memory_scope_repo_status
  ON memory_items(scope, repo_cwd, status, updated_at DESC);

CREATE VIRTUAL TABLE memory_items_fts USING fts5(
  title,
  content,
  tags_json,
  content='memory_items',
  content_rowid='rowid'
);

CREATE TABLE events (
  id               INTEGER PRIMARY KEY AUTOINCREMENT,
  turn_id          INTEGER NOT NULL REFERENCES turns(id),
  event_kind       TEXT NOT NULL,
  payload_json     TEXT NOT NULL,
  created_at       TEXT NOT NULL
);

CREATE TABLE change_log (
  seq              INTEGER PRIMARY KEY AUTOINCREMENT,
  topic            TEXT NOT NULL,
  entity_id        TEXT NOT NULL,
  payload_json     TEXT NOT NULL,
  created_at       TEXT NOT NULL
);

CREATE TRIGGER memory_fts_insert AFTER INSERT ON memory_items BEGIN
  INSERT INTO memory_items_fts(rowid,title,content,tags_json)
  VALUES(new.rowid,new.title,new.content,new.tags_json);
END;
CREATE TRIGGER memory_fts_delete AFTER DELETE ON memory_items BEGIN
  INSERT INTO memory_items_fts(memory_items_fts,rowid,title,content,tags_json)
  VALUES('delete',old.rowid,old.title,old.content,old.tags_json);
END;
CREATE TRIGGER memory_fts_update AFTER UPDATE ON memory_items BEGIN
  INSERT INTO memory_items_fts(memory_items_fts,rowid,title,content,tags_json)
  VALUES('delete',old.rowid,old.title,old.content,old.tags_json);
  INSERT INTO memory_items_fts(rowid,title,content,tags_json)
  VALUES(new.rowid,new.title,new.content,new.tags_json);
END;
