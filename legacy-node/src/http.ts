import { execFile as execFileCb } from 'node:child_process'
import type { EventEmitter } from 'node:events'
import { existsSync, readFileSync, realpathSync } from 'node:fs'
import { createServer, type IncomingMessage, type ServerResponse } from 'node:http'
import type { AddressInfo } from 'node:net'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { promisify } from 'node:util'
import { z } from 'zod'
import { parseCodexLine } from './codex.ts'
import { commitSummary } from './git-effect.ts'
import { DispatchClosedError, type HostEvent, type TurnRunner } from './runner.ts'
import type { Runs } from './runs.ts'
import {
  BusyError,
  ConflictError,
  FrozenError,
  StaleStateError,
  type SessionRow,
  type Store,
  type TurnStatus,
} from './store.ts'

const execFile = promisify(execFileCb)
const PUBLIC_DIR = fileURLToPath(new URL('../public', import.meta.url))

export interface HttpServer {
  url: string
  port: number
  close(): Promise<void>
}

class BadRequestError extends Error {}

const CreateSessionSchema = z.strictObject({
  name: z.string().trim().min(1),
  cwd: z.string().trim().min(1),
})

const UnfreezeSchema = z.strictObject({
  note: z.string().trim().min(1),
})

const SendMessageSchema = z.strictObject({
  text: z.string().trim().min(1),
})

const CreateRunSchema = z.strictObject({
  sessionId: z.string().trim().min(1),
  requestId: z.string().trim().min(1),
  instructions: z.string().trim().min(1),
  base: z.string().trim().min(1).optional(),
})

const ResolveRunSchema = z.strictObject({
  decision: z.enum(['accept', 'fail', 'allow_retry']),
  sha: z.string().trim().optional(),
  note: z.string(),
})

async function readJsonBody(req: IncomingMessage): Promise<unknown> {
  let raw = ''
  for await (const chunk of req) {
    raw += chunk.toString()
    if (raw.length > 1024 * 1024) {
      throw new BadRequestError('request body too large')
    }
  }
  if (!raw.trim()) {
    throw new BadRequestError('empty request body')
  }
  try {
    return JSON.parse(raw)
  } catch {
    throw new BadRequestError('invalid JSON')
  }
}

function sendJson(res: ServerResponse, status: number, data: unknown): void {
  res
    .writeHead(status, { 'Content-Type': 'application/json; charset=utf-8' })
    .end(JSON.stringify(data))
}

function sanitizeEvent(ev: HostEvent): HostEvent {
  if (ev.type === 'turn') {
    const turnCopy = { ...ev.turn } as Record<string, unknown>
    delete turnCopy.mcpToken
    return { type: 'turn', turn: turnCopy as typeof ev.turn }
  }
  return ev
}

async function readGitOrigin(cwd: string): Promise<string | null> {
  try {
    const { stdout } = await execFile('git', ['-C', cwd, 'remote', 'get-url', 'origin'], {
      encoding: 'utf8',
    })
    return stdout.trim() || null
  } catch {
    return null
  }
}

async function isGitWorkTree(cwd: string): Promise<boolean> {
  try {
    const { stdout } = await execFile(
      'git',
      ['-C', cwd, 'rev-parse', '--is-inside-work-tree'],
      { encoding: 'utf8' },
    )
    return stdout.trim() === 'true'
  } catch {
    return false
  }
}

async function sessionSummary(
  store: Store,
  s: SessionRow,
): Promise<
  SessionRow & {
    busy: boolean
    lastTurnStatus: TurnStatus | null
    queuedCount: number
    target: string | null
  }
> {
  const busy = Boolean(store.runningTurn(s.id) || store.activeRun(s.id))
  const lastTurnStatus = store.lastTurn(s.id)?.status ?? null
  const queuedCount = store.listMessages(s.id).filter((m) => m.queued).length
  const target = await readGitOrigin(s.cwd)
  return {
    ...s,
    busy,
    lastTurnStatus,
    queuedCount,
    target,
  }
}

function mapErrorStatus(err: unknown): number {
  if (
    err instanceof BusyError ||
    err instanceof FrozenError ||
    err instanceof ConflictError ||
    err instanceof StaleStateError ||
    err instanceof DispatchClosedError
  ) {
    return 409
  }
  if (err instanceof BadRequestError) {
    return 400
  }
  return 500
}

export async function startHttpServer(d: {
  store: Store
  runner: TurnRunner
  runs: Runs
  bus: EventEmitter
  hostInstanceId: string
  port: number
  startedAt?: string
}): Promise<HttpServer> {
  const startedAt = d.startedAt ?? new Date().toISOString()
  const sseClients = new Set<{
    res: ServerResponse
    listener: (ev: HostEvent) => void
    ping: NodeJS.Timeout
  }>()

  const server = createServer(async (req, res) => {
    const method = req.method ?? 'GET'
    const url = new URL(req.url ?? '/', 'http://127.0.0.1')
    const pathname = url.pathname

    try {
      if (method === 'GET' && pathname === '/') {
        const htmlPath = join(PUBLIC_DIR, 'index.html')
        if (!existsSync(htmlPath)) {
          sendJson(res, 404, { error: 'page not found' })
          return
        }
        res
          .writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' })
          .end(readFileSync(htmlPath))
        return
      }

      if (method === 'GET' && pathname === '/app.js') {
        const jsPath = join(PUBLIC_DIR, 'app.js')
        if (!existsSync(jsPath)) {
          sendJson(res, 404, { error: 'script not found' })
          return
        }
        res
          .writeHead(200, { 'Content-Type': 'application/javascript; charset=utf-8' })
          .end(readFileSync(jsPath))
        return
      }

      if (method === 'GET' && pathname === '/favicon.ico') {
        res.writeHead(204).end()
        return
      }

      if (method === 'GET' && pathname === '/api/status') {
        sendJson(res, 200, {
          hostInstanceId: d.hostInstanceId,
          startedAt,
          dispatchOpen: d.runner.isDispatchOpen(),
        })
        return
      }

      if (method === 'GET' && pathname === '/api/stream') {
        res.writeHead(200, {
          'Content-Type': 'text/event-stream',
          'Cache-Control': 'no-cache',
          Connection: 'keep-alive',
        })
        res.write(': connected\n\n')

        const listener = (ev: HostEvent) => {
          try {
            res.write(`data: ${JSON.stringify(sanitizeEvent(ev))}\n\n`)
          } catch {}
        }
        const ping = setInterval(() => {
          try {
            res.write(': ping\n\n')
          } catch {}
        }, 15000)

        const entry = { res, listener, ping }
        sseClients.add(entry)
        d.bus.on('event', listener)

        const cleanup = () => {
          if (!sseClients.delete(entry)) return
          d.bus.off('event', listener)
          clearInterval(ping)
        }
        req.on('close', cleanup)
        res.on('close', cleanup)
        return
      }

      if (method === 'GET' && pathname === '/api/sessions') {
        const sessions = await Promise.all(
          d.store.listSessions().map((s) => sessionSummary(d.store, s)),
        )
        sendJson(res, 200, sessions)
        return
      }

      if (method === 'POST' && pathname === '/api/sessions') {
        const raw = await readJsonBody(req)
        const parsed = CreateSessionSchema.safeParse(raw)
        if (!parsed.success) {
          sendJson(res, 400, { error: 'invalid session input' })
          return
        }
        if (!existsSync(parsed.data.cwd)) {
          sendJson(res, 400, { error: 'cwd must be a git work tree' })
          return
        }
        const realCwd = realpathSync(parsed.data.cwd)
        if (!(await isGitWorkTree(realCwd))) {
          sendJson(res, 400, { error: 'cwd must be a git work tree' })
          return
        }
        const session = d.store.createSession({ name: parsed.data.name, cwd: realCwd })
        d.bus.emit('event', { type: 'session', session })
        sendJson(res, 201, await sessionSummary(d.store, session))
        return
      }

      const sessionMatch = /^\/api\/sessions\/([^/]+)\/(unfreeze|messages|cancel|turns)$/.exec(
        pathname,
      )
      if (sessionMatch) {
        const sessionId = decodeURIComponent(sessionMatch[1]!)
        const action = sessionMatch[2]!
        const session = d.store.getSession(sessionId)
        if (!session) {
          sendJson(res, 404, { error: 'session not found' })
          return
        }

        if (method === 'POST' && action === 'unfreeze') {
          const raw = await readJsonBody(req)
          const parsed = UnfreezeSchema.safeParse(raw)
          if (!parsed.success) {
            sendJson(res, 400, { error: 'note is required' })
            return
          }
          const current = d.store.getSession(sessionId)
          if (!current || !current.frozenReason) {
            sendJson(res, 409, { error: 'session is not frozen' })
            return
          }
          d.store.unfreezeSession(sessionId)
          const msg = d.store.addMessage({
            sessionId,
            turnId: null,
            source: 'host',
            text: `Unfrozen by user: ${parsed.data.note}`,
          })
          const updated = d.store.getSession(sessionId)!
          d.bus.emit('event', { type: 'message', message: msg })
          d.bus.emit('event', { type: 'session', session: updated })
          d.runner.drainQueues()
          sendJson(res, 200, await sessionSummary(d.store, updated))
          return
        }

        if (method === 'GET' && action === 'messages') {
          sendJson(res, 200, d.store.listMessages(sessionId))
          return
        }

        if (method === 'GET' && action === 'turns') {
          const turns = d.store.listSessionTurns(sessionId).map((t) => {
            const rawEvents = d.store.listTurnEvents(t.id)
            const events = rawEvents
              .map((e) => parseCodexLine(e.json))
              .filter((p): p is NonNullable<typeof p> => Boolean(p?.event))
              .map((p) => p.event!)
            return { ...t, events }
          })
          sendJson(res, 200, turns)
          return
        }

        if (method === 'POST' && action === 'messages') {
          const raw = await readJsonBody(req)
          const parsed = SendMessageSchema.safeParse(raw)
          if (!parsed.success) {
            sendJson(res, 400, { error: 'text is required' })
            return
          }
          const message = d.runner.sendUser(sessionId, parsed.data.text)
          sendJson(res, 202, { message })
          return
        }

        if (method === 'POST' && action === 'cancel') {
          const cancelled = await d.runner.cancel(sessionId)
          sendJson(res, 200, { cancelled })
          return
        }
      }

      if (method === 'GET' && pathname === '/api/runs') {
        sendJson(res, 200, d.store.listRuns())
        return
      }

      if (method === 'POST' && pathname === '/api/runs') {
        const raw = await readJsonBody(req)
        const parsed = CreateRunSchema.safeParse(raw)
        if (!parsed.success) {
          sendJson(res, 400, { error: 'invalid run input' })
          return
        }
        const session = d.store.getSession(parsed.data.sessionId)
        if (!session) {
          sendJson(res, 404, { error: 'session not found' })
          return
        }
        try {
          const { run, created } = await d.runs.create({
            sessionId: parsed.data.sessionId,
            requestId: parsed.data.requestId,
            instructions: parsed.data.instructions,
            base: parsed.data.base,
          })
          if (created) {
            void d.runs.start(run.id).catch(() => {})
            sendJson(res, 201, run)
          } else {
            sendJson(res, 200, run)
          }
        } catch (err) {
          const status = mapErrorStatus(err)
          if (status !== 500) {
            sendJson(res, status, { error: err instanceof Error ? err.message : String(err) })
            return
          }
          sendJson(res, 400, { error: err instanceof Error ? err.message : String(err) })
        }
        return
      }

      const runDetailMatch = /^\/api\/runs\/([^/]+)$/.exec(pathname)
      if (method === 'GET' && runDetailMatch) {
        const runId = decodeURIComponent(runDetailMatch[1]!)
        const run = d.store.getRun(runId)
        if (!run) {
          sendJson(res, 404, { error: 'run not found' })
          return
        }
        const commitSha = run.observedSha ?? run.resultSha
        const commit = commitSha ? await commitSummary(run.remote, commitSha) : null
        sendJson(res, 200, {
          run,
          turns: d.store.listRunTurns(runId),
          acceptedResult: d.store.acceptedResult(runId) ?? null,
          lastRejectedError: d.store.lastRejectedError(runId),
          commit,
        })
        return
      }

      const runActionMatch = /^\/api\/runs\/([^/]+)\/(retry|resolve)$/.exec(pathname)
      if (method === 'POST' && runActionMatch) {
        const runId = decodeURIComponent(runActionMatch[1]!)
        const action = runActionMatch[2]!
        const run = d.store.getRun(runId)
        if (!run) {
          sendJson(res, 404, { error: 'run not found' })
          return
        }

        if (action === 'retry') {
          const updated = await d.runs.retry(runId)
          sendJson(res, 200, updated)
          return
        }

        const raw = await readJsonBody(req)
        const parsed = ResolveRunSchema.safeParse(raw)
        if (!parsed.success) {
          sendJson(res, 400, { error: 'invalid resolve input' })
          return
        }
        if (parsed.data.decision === 'accept') {
          if (!parsed.data.sha || !/^[0-9a-f]{40}$/.test(parsed.data.sha)) {
            sendJson(res, 400, { error: 'valid 40-char sha is required for accept' })
            return
          }
          const updated = await d.runs.resolve(runId, {
            decision: 'accept',
            sha: parsed.data.sha,
            note: parsed.data.note,
          })
          sendJson(res, 200, updated)
          return
        }

        const updated = await d.runs.resolve(runId, {
          decision: parsed.data.decision,
          note: parsed.data.note,
        })
        sendJson(res, 200, updated)
        return
      }

      sendJson(res, 404, { error: 'not found' })
    } catch (err) {
      if (res.headersSent) return
      const status = mapErrorStatus(err)
      sendJson(res, status, { error: err instanceof Error ? err.message : String(err) })
    }
  })

  await new Promise<void>((resolve) => {
    server.listen(d.port, '127.0.0.1', () => resolve())
  })
  const addr = server.address() as AddressInfo

  return {
    url: `http://127.0.0.1:${addr.port}`,
    port: addr.port,
    close() {
      for (const client of [...sseClients]) {
        d.bus.off('event', client.listener)
        clearInterval(client.ping)
        sseClients.delete(client)
      }
      return new Promise<void>((resolve) => {
        server.closeAllConnections?.()
        server.close(() => resolve())
      })
    },
  }
}
