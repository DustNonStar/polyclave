import { realpathSync } from 'node:fs'
import { DatabaseSync, type SQLInputValue } from 'node:sqlite'
import { mcpToken, runId as makeRunId, sessionId as makeSessionId } from './ids.ts'

export type TurnStatus = 'running' | 'completed' | 'cancelled' | 'interrupted' | 'failed'
export type TurnPhase = 'prepared' | 'authorized'
export type EndReason = 'exit' | 'cancel' | 'host_lost' | 'never_started' | 'dispatch_closed'
export type ClaimMode = 'chat' | 'start' | 'correction' | 'retry'
export type WriterCheck = 'excluded' | 'possible'
export type RunStatus = 'pending' | 'running' | 'done' | 'interrupted' | 'failed' | 'needs_review'
export type MessageSource = 'user' | 'agent' | 'host'

export interface SessionRow {
  id: string
  name: string
  cwd: string
  agentSessionId: string | null
  frozenReason: string | null
  frozenAt: string | null
  createdAt: string
}

export interface TurnRow {
  id: number
  sessionId: string
  runId: string | null
  status: TurnStatus
  phase: TurnPhase
  hostInstanceId: string
  bootTime: string
  agentPgid: number | null
  agentStartedAt: string | null
  mcpToken: string
  exitCode: number | null
  endReason: EndReason | null
  writerCheck: WriterCheck | null
  startedAt: string
  endedAt: string | null
}

export interface MessageRow {
  id: number
  sessionId: string
  turnId: number | null
  source: MessageSource
  text: string
  queued: boolean
  at: string
}

export interface RunRow {
  id: string
  sessionId: string
  requestId: string
  instructions: string
  remote: string
  base: string
  baseSha: string
  branch: string
  worktreePath: string
  agentSessionId: string | null
  status: RunStatus
  corrections: number
  retries: number
  detail: string | null
  resultSha: string | null
  observedSha: string | null
  resultSource: 'agent' | 'user' | null
  resolution: string | null
  updatedAt: string
}

export interface ResultRow {
  id: number
  runId: string
  turnId: number
  accepted: boolean
  payload: string
  error: string | null
  at: string
}

export interface EventRow {
  id: number
  turnId: number
  type: string
  json: string
}

export type SettleOutcome =
  | 'accepted'
  | 'already_accepted'
  | 'different_result_accepted'
  | 'turn_not_active'

export class BusyError extends Error {}
export class FrozenError extends Error {}
export class ConflictError extends Error {}
export class StaleStateError extends Error {}

function nowIso(): string {
  return new Date().toISOString()
}

function isUniqueViolation(err: unknown): boolean {
  return (
    typeof err === 'object' &&
    err !== null &&
    'errcode' in err &&
    (err as { errcode?: unknown }).errcode === 2067
  )
}

function normalizeText(value: string): string {
  return value.trim()
}

function safeRealpath(p: string): string {
  try {
    return realpathSync(p)
  } catch {
    return p
  }
}

type RawSession = {
  id: string
  name: string
  cwd: string
  agent_session_id: string | null
  frozen_reason: string | null
  frozen_at: string | null
  created_at: string
}

type RawTurn = {
  id: number
  session_id: string
  run_id: string | null
  status: TurnStatus
  phase: TurnPhase
  host_instance_id: string
  boot_time: string
  agent_pgid: number | null
  agent_started_at: string | null
  mcp_token: string
  exit_code: number | null
  end_reason: EndReason | null
  writer_check: WriterCheck | null
  started_at: string
  ended_at: string | null
}

type RawMessage = {
  id: number
  session_id: string
  turn_id: number | null
  source: MessageSource
  text: string
  queued: number
  at: string
}

type RawRun = {
  id: string
  session_id: string
  request_id: string
  instructions: string
  remote: string
  base: string
  base_sha: string
  branch: string
  worktree_path: string
  agent_session_id: string | null
  status: RunStatus
  corrections: number
  retries: number
  detail: string | null
  result_sha: string | null
  observed_sha: string | null
  result_source: 'agent' | 'user' | null
  resolution: string | null
  updated_at: string
}

type RawResult = {
  id: number
  run_id: string
  turn_id: number
  accepted: number
  payload: string
  error: string | null
  at: string
}

function mapSession(r: RawSession): SessionRow {
  return {
    id: r.id,
    name: r.name,
    cwd: r.cwd,
    agentSessionId: r.agent_session_id,
    frozenReason: r.frozen_reason,
    frozenAt: r.frozen_at,
    createdAt: r.created_at,
  }
}

function mapTurn(r: RawTurn): TurnRow {
  return {
    id: r.id,
    sessionId: r.session_id,
    runId: r.run_id,
    status: r.status,
    phase: r.phase,
    hostInstanceId: r.host_instance_id,
    bootTime: r.boot_time,
    agentPgid: r.agent_pgid,
    agentStartedAt: r.agent_started_at,
    mcpToken: r.mcp_token,
    exitCode: r.exit_code,
    endReason: r.end_reason,
    writerCheck: r.writer_check,
    startedAt: r.started_at,
    endedAt: r.ended_at,
  }
}

function mapMessage(r: RawMessage): MessageRow {
  return {
    id: r.id,
    sessionId: r.session_id,
    turnId: r.turn_id,
    source: r.source,
    text: r.text,
    queued: r.queued !== 0,
    at: r.at,
  }
}

function mapRun(r: RawRun): RunRow {
  return {
    id: r.id,
    sessionId: r.session_id,
    requestId: r.request_id,
    instructions: r.instructions,
    remote: r.remote,
    base: r.base,
    baseSha: r.base_sha,
    branch: r.branch,
    worktreePath: r.worktree_path,
    agentSessionId: r.agent_session_id,
    status: r.status,
    corrections: r.corrections,
    retries: r.retries,
    detail: r.detail,
    resultSha: r.result_sha,
    observedSha: r.observed_sha,
    resultSource: r.result_source,
    resolution: r.resolution,
    updatedAt: r.updated_at,
  }
}

function mapResult(r: RawResult): ResultRow {
  return {
    id: r.id,
    runId: r.run_id,
    turnId: r.turn_id,
    accepted: r.accepted !== 0,
    payload: r.payload,
    error: r.error,
    at: r.at,
  }
}

export class Store {
  private readonly db: DatabaseSync

  constructor(path: string) {
    this.db = new DatabaseSync(path)
    this.db.exec('PRAGMA busy_timeout = 5000;')
    this.db.exec('PRAGMA journal_mode = WAL;')
    this.db.exec('PRAGMA foreign_keys = ON;')
    this.db.exec(`
      CREATE TABLE IF NOT EXISTS sessions (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        cwd TEXT NOT NULL,
        agent_session_id TEXT,
        frozen_reason TEXT,
        frozen_at TEXT,
        created_at TEXT NOT NULL
      );

      CREATE TABLE IF NOT EXISTS runs (
        id TEXT PRIMARY KEY,
        session_id TEXT NOT NULL REFERENCES sessions(id),
        request_id TEXT NOT NULL,
        instructions TEXT NOT NULL,
        remote TEXT NOT NULL,
        base TEXT NOT NULL,
        base_sha TEXT NOT NULL,
        branch TEXT NOT NULL,
        worktree_path TEXT NOT NULL,
        agent_session_id TEXT,
        status TEXT NOT NULL,
        corrections INTEGER NOT NULL DEFAULT 0,
        retries INTEGER NOT NULL DEFAULT 0,
        detail TEXT,
        result_sha TEXT,
        observed_sha TEXT,
        result_source TEXT,
        resolution TEXT,
        updated_at TEXT NOT NULL
      );

      CREATE UNIQUE INDEX IF NOT EXISTS idx_runs_req ON runs(session_id, request_id);
      CREATE UNIQUE INDEX IF NOT EXISTS idx_runs_active ON runs(session_id) WHERE status IN ('pending', 'running');

      CREATE TABLE IF NOT EXISTS turns (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        session_id TEXT NOT NULL REFERENCES sessions(id),
        run_id TEXT REFERENCES runs(id),
        status TEXT NOT NULL,
        phase TEXT NOT NULL,
        host_instance_id TEXT NOT NULL,
        boot_time TEXT NOT NULL,
        agent_pgid INTEGER,
        agent_started_at TEXT,
        mcp_token TEXT NOT NULL,
        exit_code INTEGER,
        end_reason TEXT,
        writer_check TEXT,
        started_at TEXT NOT NULL,
        ended_at TEXT
      );

      CREATE UNIQUE INDEX IF NOT EXISTS idx_turns_running ON turns(session_id) WHERE status = 'running';
      CREATE UNIQUE INDEX IF NOT EXISTS idx_turns_token ON turns(mcp_token);

      CREATE TABLE IF NOT EXISTS messages (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        session_id TEXT NOT NULL REFERENCES sessions(id),
        turn_id INTEGER REFERENCES turns(id),
        source TEXT NOT NULL,
        text TEXT NOT NULL,
        queued INTEGER NOT NULL DEFAULT 0,
        at TEXT NOT NULL
      );

      CREATE TABLE IF NOT EXISTS events (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        turn_id INTEGER NOT NULL REFERENCES turns(id),
        type TEXT NOT NULL,
        json TEXT NOT NULL
      );

      CREATE TABLE IF NOT EXISTS results (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        run_id TEXT NOT NULL REFERENCES runs(id),
        turn_id INTEGER NOT NULL REFERENCES turns(id),
        accepted INTEGER NOT NULL,
        payload TEXT NOT NULL,
        error TEXT,
        at TEXT NOT NULL
      );

      CREATE UNIQUE INDEX IF NOT EXISTS idx_results_accepted ON results(run_id) WHERE accepted = 1;
    `)
  }

  close(): void {
    this.db.close()
  }

  private tx<T>(fn: () => T): T {
    this.db.exec('BEGIN IMMEDIATE')
    try {
      const result = fn()
      this.db.exec('COMMIT')
      return result
    } catch (err) {
      try {
        this.db.exec('ROLLBACK')
      } catch {}
      if (isUniqueViolation(err)) {
        throw new BusyError('session is busy')
      }
      throw err
    }
  }

  createSession(i: { name: string; cwd: string }): SessionRow {
    const id = makeSessionId()
    const createdAt = nowIso()
    const cwd = safeRealpath(i.cwd)
    this.db
      .prepare('INSERT INTO sessions (id, name, cwd, created_at) VALUES (?, ?, ?, ?)')
      .run(id, i.name, cwd, createdAt)
    return this.getSession(id)!
  }

  getSession(id: string): SessionRow | undefined {
    const row = this.db.prepare('SELECT * FROM sessions WHERE id = ?').get(id) as
      | RawSession
      | undefined
    return row ? mapSession(row) : undefined
  }

  listSessions(): SessionRow[] {
    const rows = this.db
      .prepare('SELECT * FROM sessions ORDER BY created_at ASC, id ASC')
      .all() as RawSession[]
    return rows.map(mapSession)
  }

  setSessionThread(id: string, agentSessionId: string): void {
    this.db.prepare('UPDATE sessions SET agent_session_id = ? WHERE id = ?').run(agentSessionId, id)
  }

  freezeSession(id: string, reason: string): void {
    const at = nowIso()
    this.db
      .prepare(
        'UPDATE sessions SET frozen_reason = COALESCE(frozen_reason, ?), frozen_at = COALESCE(frozen_at, ?) WHERE id = ?',
      )
      .run(reason, at, id)
  }

  unfreezeSession(id: string): void {
    this.db
      .prepare('UPDATE sessions SET frozen_reason = NULL, frozen_at = NULL WHERE id = ?')
      .run(id)
  }

  replayRun(i: {
    sessionId: string
    requestId: string
    instructions: string
    base: string
  }): RunRow | undefined {
    const row = this.db
      .prepare('SELECT * FROM runs WHERE session_id = ? AND request_id = ?')
      .get(i.sessionId, i.requestId) as RawRun | undefined
    if (!row) return undefined
    if (
      row.instructions !== normalizeText(i.instructions) ||
      row.base !== normalizeText(i.base)
    ) {
      throw new ConflictError('request id already used with different input')
    }
    return mapRun(row)
  }

  acceptRun(i: {
    sessionId: string
    requestId: string
    instructions: string
    remote: string
    base: string
    baseSha: string
    worktreePath: (runId: string) => string
  }): { run: RunRow; created: boolean } {
    const instructions = normalizeText(i.instructions)
    const base = normalizeText(i.base)
    return this.tx(() => {
      const existing = this.db
        .prepare('SELECT * FROM runs WHERE session_id = ? AND request_id = ?')
        .get(i.sessionId, i.requestId) as RawRun | undefined
      if (existing) {
        if (existing.instructions !== instructions || existing.base !== base) {
          throw new ConflictError('request id already used with different input')
        }
        return { run: mapRun(existing), created: false }
      }

      const sess = this.getSession(i.sessionId)
      if (!sess) throw new Error(`unknown session ${i.sessionId}`)
      if (sess.frozenAt !== null) {
        throw new FrozenError(sess.frozenReason ?? 'session is frozen')
      }
      if (this.runningTurn(i.sessionId)) {
        throw new BusyError('session has a running turn')
      }

      const id = makeRunId()
      const branch = `agent/${id}`
      const worktreePath = i.worktreePath(id)
      const updatedAt = nowIso()

      this.db
        .prepare(
          `INSERT INTO runs (
            id, session_id, request_id, instructions, remote, base, base_sha,
            branch, worktree_path, status, updated_at
          ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'pending', ?)`,
        )
        .run(
          id,
          i.sessionId,
          i.requestId,
          instructions,
          i.remote,
          base,
          i.baseSha,
          branch,
          worktreePath,
          updatedAt,
        )

      return { run: this.getRun(id)!, created: true }
    })
  }

  claimTurn(i: {
    sessionId: string
    mode: ClaimMode
    runId?: string
    previousTurnId?: number
    messageId?: number
    hostInstanceId: string
    bootTime: string
  }): TurnRow {
    return this.tx(() => {
      const sess = this.getSession(i.sessionId)
      if (!sess) throw new Error(`unknown session ${i.sessionId}`)
      if (sess.frozenAt !== null) {
        throw new FrozenError(sess.frozenReason ?? 'session is frozen')
      }

      if (i.mode === 'chat') {
        if (i.runId !== undefined) {
          throw new StaleStateError('chat turn cannot have a runId')
        }
        if (this.activeRun(i.sessionId)) {
          throw new BusyError('session has an active run')
        }
        if (i.messageId !== undefined) {
          const msg = this.db
            .prepare('SELECT * FROM messages WHERE id = ? AND session_id = ?')
            .get(i.messageId, i.sessionId) as RawMessage | undefined
          if (!msg || msg.queued === 0 || msg.turn_id !== null) {
            throw new StaleStateError('message is not queued and unbound')
          }
        }
      } else if (i.mode === 'start') {
        if (!i.runId) throw new StaleStateError('runId required')
        const res = this.db
          .prepare(
            "UPDATE runs SET status = 'running', updated_at = ? WHERE id = ? AND session_id = ? AND status = 'pending'",
          )
          .run(nowIso(), i.runId, i.sessionId)
        if (res.changes !== 1) {
          throw new StaleStateError('run is not pending')
        }
      } else if (i.mode === 'correction') {
        if (!i.runId || i.previousTurnId === undefined) {
          throw new StaleStateError('runId and previousTurnId required')
        }
        const run = this.getRun(i.runId)
        if (!run || run.sessionId !== i.sessionId || run.status !== 'running') {
          throw new StaleStateError('run is not running')
        }
        const latest = this.lastRunTurn(i.runId)
        if (!latest || latest.id !== i.previousTurnId) {
          throw new StaleStateError('previousTurnId does not match latest turn of run')
        }
      } else if (i.mode === 'retry') {
        if (!i.runId) throw new StaleStateError('runId required')
        const res = this.db
          .prepare(
            "UPDATE runs SET status = 'running', retries = retries + 1, corrections = 0, updated_at = ? WHERE id = ? AND session_id = ? AND status = 'interrupted' AND retries < 3",
          )
          .run(nowIso(), i.runId, i.sessionId)
        if (res.changes !== 1) {
          throw new StaleStateError('run cannot be retried')
        }
      }

      const token = mcpToken()
      const startedAt = nowIso()
      const insert = this.db
        .prepare(
          `INSERT INTO turns (
            session_id, run_id, status, phase, host_instance_id, boot_time, mcp_token, started_at
          ) VALUES (?, ?, 'running', 'prepared', ?, ?, ?, ?)`,
        )
        .run(i.sessionId, i.runId ?? null, i.hostInstanceId, i.bootTime, token, startedAt)

      const turnId = Number(insert.lastInsertRowid)

      if (i.mode === 'chat' && i.messageId !== undefined) {
        const bind = this.db
          .prepare(
            'UPDATE messages SET turn_id = ?, queued = 0 WHERE id = ? AND session_id = ? AND queued = 1 AND turn_id IS NULL',
          )
          .run(turnId, i.messageId, i.sessionId)
        if (bind.changes !== 1) {
          throw new StaleStateError('failed to bind queued message')
        }
      }

      return this.getTurn(turnId)!
    })
  }

  authorizeTurn(turnId: number, pgid: number, startedAt: string | null): boolean {
    const res = this.db
      .prepare(
        "UPDATE turns SET phase = 'authorized', agent_pgid = ?, agent_started_at = ? WHERE id = ? AND status = 'running' AND phase = 'prepared'",
      )
      .run(pgid, startedAt, turnId)
    return res.changes === 1
  }

  endTurn(
    turnId: number,
    status: Exclude<TurnStatus, 'running'>,
    f: {
      exitCode: number | null
      endReason: EndReason
      writerCheck: WriterCheck
      freezeReason?: string
    },
  ): boolean {
    return this.tx(() => {
      const endedAt = nowIso()
      const res = this.db
        .prepare(
          "UPDATE turns SET status = ?, exit_code = ?, end_reason = ?, writer_check = ?, ended_at = ? WHERE id = ? AND status = 'running'",
        )
        .run(status, f.exitCode, f.endReason, f.writerCheck, endedAt, turnId)
      if (res.changes !== 1) return false

      const turn = this.getTurn(turnId)!
      if (f.freezeReason !== undefined) {
        this.db
          .prepare(
            'UPDATE sessions SET frozen_reason = COALESCE(frozen_reason, ?), frozen_at = COALESCE(frozen_at, ?) WHERE id = ?',
          )
          .run(f.freezeReason, endedAt, turn.sessionId)
      }
      if (f.endReason === 'never_started') {
        this.db
          .prepare('UPDATE messages SET queued = 1, turn_id = NULL WHERE turn_id = ?')
          .run(turnId)
      }
      return true
    })
  }

  lastChatTurn(sessionId: string): TurnRow | undefined {
    const row = this.db
      .prepare(
        'SELECT * FROM turns WHERE session_id = ? AND run_id IS NULL ORDER BY id DESC LIMIT 1',
      )
      .get(sessionId) as RawTurn | undefined
    return row ? mapTurn(row) : undefined
  }

  lastRunTurn(runId: string): TurnRow | undefined {
    const row = this.db
      .prepare('SELECT * FROM turns WHERE run_id = ? ORDER BY id DESC LIMIT 1')
      .get(runId) as RawTurn | undefined
    return row ? mapTurn(row) : undefined
  }

  turnByToken(token: string, hostInstanceId: string): TurnRow | undefined {
    const row = this.db
      .prepare(
        "SELECT * FROM turns WHERE mcp_token = ? AND host_instance_id = ? AND status = 'running' AND phase = 'authorized'",
      )
      .get(token, hostInstanceId) as RawTurn | undefined
    return row ? mapTurn(row) : undefined
  }

  getTurn(id: number): TurnRow | undefined {
    const row = this.db.prepare('SELECT * FROM turns WHERE id = ?').get(id) as
      | RawTurn
      | undefined
    return row ? mapTurn(row) : undefined
  }

  runningTurns(): TurnRow[] {
    const rows = this.db
      .prepare("SELECT * FROM turns WHERE status = 'running' ORDER BY id ASC")
      .all() as RawTurn[]
    return rows.map(mapTurn)
  }

  runningTurn(sessionId: string): TurnRow | undefined {
    const row = this.db
      .prepare("SELECT * FROM turns WHERE session_id = ? AND status = 'running'")
      .get(sessionId) as RawTurn | undefined
    return row ? mapTurn(row) : undefined
  }

  lastTurn(sessionId: string): TurnRow | undefined {
    const row = this.db
      .prepare('SELECT * FROM turns WHERE session_id = ? ORDER BY id DESC LIMIT 1')
      .get(sessionId) as RawTurn | undefined
    return row ? mapTurn(row) : undefined
  }

  addMessage(i: {
    sessionId: string
    turnId: number | null
    source: MessageSource
    text: string
    queued?: boolean
  }): MessageRow {
    const at = nowIso()
    const queued = i.queued ? 1 : 0
    const res = this.db
      .prepare(
        'INSERT INTO messages (session_id, turn_id, source, text, queued, at) VALUES (?, ?, ?, ?, ?, ?)',
      )
      .run(i.sessionId, i.turnId, i.source, i.text, queued, at)
    const row = this.db
      .prepare('SELECT * FROM messages WHERE id = ?')
      .get(Number(res.lastInsertRowid)) as RawMessage
    return mapMessage(row)
  }

  listMessages(sessionId: string): MessageRow[] {
    const rows = this.db
      .prepare('SELECT * FROM messages WHERE session_id = ? ORDER BY id ASC')
      .all(sessionId) as RawMessage[]
    return rows.map(mapMessage)
  }

  nextQueued(sessionId: string): MessageRow | undefined {
    const row = this.db
      .prepare(
        'SELECT * FROM messages WHERE session_id = ? AND queued = 1 AND turn_id IS NULL ORDER BY id ASC LIMIT 1',
      )
      .get(sessionId) as RawMessage | undefined
    return row ? mapMessage(row) : undefined
  }

  addEvent(turnId: number, type: string, json: string): void {
    this.db
      .prepare('INSERT INTO events (turn_id, type, json) VALUES (?, ?, ?)')
      .run(turnId, type, json)
  }

  getRun(id: string): RunRow | undefined {
    const row = this.db.prepare('SELECT * FROM runs WHERE id = ?').get(id) as
      | RawRun
      | undefined
    return row ? mapRun(row) : undefined
  }

  listRuns(): RunRow[] {
    const rows = this.db
      .prepare('SELECT * FROM runs ORDER BY rowid ASC')
      .all() as RawRun[]
    return rows.map(mapRun)
  }

  activeRun(sessionId: string): RunRow | undefined {
    const row = this.db
      .prepare("SELECT * FROM runs WHERE session_id = ? AND status IN ('pending', 'running')")
      .get(sessionId) as RawRun | undefined
    return row ? mapRun(row) : undefined
  }

  updateRun(
    id: string,
    patch: Partial<Omit<RunRow, 'id' | 'sessionId' | 'requestId'>>,
    expect: RunStatus[],
  ): RunRow {
    if (expect.length === 0) throw new StaleStateError('expect must be non-empty')
    const columnMap: Record<keyof Omit<RunRow, 'id' | 'sessionId' | 'requestId'>, string> = {
      instructions: 'instructions',
      remote: 'remote',
      base: 'base',
      baseSha: 'base_sha',
      branch: 'branch',
      worktreePath: 'worktree_path',
      agentSessionId: 'agent_session_id',
      status: 'status',
      corrections: 'corrections',
      retries: 'retries',
      detail: 'detail',
      resultSha: 'result_sha',
      observedSha: 'observed_sha',
      resultSource: 'result_source',
      resolution: 'resolution',
      updatedAt: 'updated_at',
    }
    const sets: string[] = []
    const values: SQLInputValue[] = []
    for (const [k, v] of Object.entries(patch)) {
      if (v === undefined) continue
      const col = columnMap[k as keyof typeof columnMap]
      if (!col) continue
      sets.push(`${col} = ?`)
      values.push(v as SQLInputValue)
    }
    if (patch.updatedAt === undefined) {
      sets.push('updated_at = ?')
      values.push(nowIso())
    }
    const placeholders = expect.map(() => '?').join(', ')
    return this.tx(() => {
      const res = this.db
        .prepare(`UPDATE runs SET ${sets.join(', ')} WHERE id = ? AND status IN (${placeholders})`)
        .run(...values, id, ...expect)
      if (res.changes !== 1) {
        throw new StaleStateError('run state changed concurrently')
      }
      return this.getRun(id)!
    })
  }

  setRunThread(id: string, agentSessionId: string): void {
    this.db
      .prepare('UPDATE runs SET agent_session_id = ?, updated_at = ? WHERE id = ?')
      .run(agentSessionId, nowIso(), id)
  }

  addRejectedResult(i: {
    runId: string
    turnId: number
    payload: unknown
    error: string
  }): void {
    this.db
      .prepare(
        'INSERT INTO results (run_id, turn_id, accepted, payload, error, at) VALUES (?, ?, 0, ?, ?, ?)',
      )
      .run(i.runId, i.turnId, JSON.stringify(i.payload ?? null), i.error, nowIso())
  }

  lastRejectedError(runId: string): string | null {
    const row = this.db
      .prepare(
        'SELECT error FROM results WHERE run_id = ? AND accepted = 0 ORDER BY id DESC LIMIT 1',
      )
      .get(runId) as { error: string | null } | undefined
    return row?.error ?? null
  }

  settleResult(i: {
    token: string
    hostInstanceId: string
    runId: string
    branch: string
    sha: string
  }): SettleOutcome {
    return this.tx(() => {
      const turn = this.db
        .prepare(
          "SELECT * FROM turns WHERE mcp_token = ? AND host_instance_id = ? AND status = 'running' AND phase = 'authorized' AND run_id = ?",
        )
        .get(i.token, i.hostInstanceId, i.runId) as RawTurn | undefined
      if (!turn) return 'turn_not_active'

      const existing = this.db
        .prepare('SELECT * FROM results WHERE run_id = ? AND accepted = 1')
        .get(i.runId) as RawResult | undefined
      if (existing) {
        const parsed = JSON.parse(existing.payload) as { branch?: string; sha?: string }
        if (parsed.branch === i.branch && parsed.sha === i.sha) {
          return 'already_accepted'
        }
        return 'different_result_accepted'
      }

      this.db
        .prepare(
          'INSERT INTO results (run_id, turn_id, accepted, payload, error, at) VALUES (?, ?, 1, ?, NULL, ?)',
        )
        .run(
          i.runId,
          turn.id,
          JSON.stringify({ run_id: i.runId, branch: i.branch, sha: i.sha }),
          nowIso(),
        )
      return 'accepted'
    })
  }

  acceptedResult(runId: string): ResultRow | undefined {
    const row = this.db
      .prepare('SELECT * FROM results WHERE run_id = ? AND accepted = 1')
      .get(runId) as RawResult | undefined
    return row ? mapResult(row) : undefined
  }

  listSessionTurns(sessionId: string): Omit<TurnRow, 'mcpToken'>[] {
    const rows = this.db
      .prepare('SELECT * FROM turns WHERE session_id = ? ORDER BY id ASC')
      .all(sessionId) as RawTurn[]
    return rows.map((r) => {
      const { mcpToken: _ignored, ...rest } = mapTurn(r)
      return rest
    })
  }

  listRunTurns(runId: string): Omit<TurnRow, 'mcpToken'>[] {
    const rows = this.db
      .prepare('SELECT * FROM turns WHERE run_id = ? ORDER BY id ASC')
      .all(runId) as RawTurn[]
    return rows.map((r) => {
      const { mcpToken: _ignored, ...rest } = mapTurn(r)
      return rest
    })
  }

  listTurnEvents(turnId: number): EventRow[] {
    const rows = this.db
      .prepare('SELECT id, turn_id, type, json FROM events WHERE turn_id = ? ORDER BY id ASC')
      .all(turnId) as { id: number; turn_id: number; type: string; json: string }[]
    return rows.map((r) => ({
      id: r.id,
      turnId: r.turn_id,
      type: r.type,
      json: r.json,
    }))
  }
}
