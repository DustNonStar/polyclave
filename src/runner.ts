import type { EventEmitter } from 'node:events'
import { existsSync, mkdirSync, realpathSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import type { Readable } from 'node:stream'
import {
  buildCodexArgs,
  gitCommonDir,
  parseCodexLine,
  type AgentEvent,
} from './codex.ts'
import { bootTime, killGroup, processStartTime, spawnSupervised } from './proc.ts'
import {
  BusyError,
  FrozenError,
  StaleStateError,
  type ClaimMode,
  type MessageRow,
  type RunRow,
  type SessionRow,
  type Store,
  type TurnRow,
} from './store.ts'

export type HostEvent =
  | { type: 'message'; message: MessageRow }
  | { type: 'turn'; turn: Omit<TurnRow, 'mcpToken'> }
  | { type: 'agent'; sessionId: string; turnId: number; event: AgentEvent }
  | { type: 'run'; run: RunRow }
  | { type: 'session'; session: SessionRow }

export interface TurnOutcome {
  turn: TurnRow
  reply: string
  selfExited: boolean
}

export interface RunnerDeps {
  store: Store
  mcpUrl: string
  codexCommand: string[]
  codexHome: string
  hostInstanceId: string
  extraEnv?: NodeJS.ProcessEnv
  bus: EventEmitter
}

export class DispatchClosedError extends Error {}

const INTERRUPT_NOTE =
  'Note from the host: your previous turn was interrupted. Check the current state before continuing.'

function stripToken(turn: TurnRow): Omit<TurnRow, 'mcpToken'> {
  const { mcpToken: _ignored, ...rest } = turn
  return rest
}

function safeRealpath(p: string): string {
  try {
    return realpathSync(p)
  } catch {
    return p
  }
}

interface ActiveTurnEntry {
  sessionId: string
  pgid: number
  cancelRequested: boolean
  stdout?: Readable
  stderr?: Readable
}

export class TurnRunner {
  private readonly d: RunnerDeps
  private dispatchOpen = true
  private readonly activeTurns = new Map<number, ActiveTurnEntry>()
  private readonly pending = new Set<Promise<unknown>>()

  constructor(d: RunnerDeps) {
    this.d = d
  }

  setDispatch(open: boolean): void {
    this.dispatchOpen = open
  }

  isDispatchOpen(): boolean {
    return this.dispatchOpen
  }

  private emit(ev: HostEvent): void {
    this.d.bus.emit('event', ev)
  }

  private track<T>(p: Promise<T>): Promise<T> {
    this.pending.add(p)
    const cleanup = () => {
      this.pending.delete(p)
    }
    void p.then(cleanup, cleanup)
    return p
  }

  private buildEnv(token: string): NodeJS.ProcessEnv {
    const env: NodeJS.ProcessEnv = { ...process.env, ...this.d.extraEnv }
    delete env.GH_TOKEN
    delete env.GITHUB_TOKEN
    for (const key of Object.keys(env)) {
      if (key.startsWith('POLYCLAVE_')) {
        delete env[key]
      }
    }
    env.CODEX_HOME = safeRealpath(this.d.codexHome)
    env.POLYCLAVE_MCP_TOKEN = token
    return env
  }

  private shouldAddInterruptNote(sessionId: string): boolean {
    const prev = this.d.store.lastChatTurn(sessionId)
    if (!prev) return false
    if (prev.status !== 'interrupted' && prev.status !== 'cancelled') return false
    if (prev.endReason === 'never_started') return false

    const messages = this.d.store.listMessages(sessionId)
    const alreadyNoted = messages.some((m) => {
      if (m.source !== 'host') return false
      if (!m.text.startsWith(INTERRUPT_NOTE)) return false
      if (m.turnId !== null) return m.turnId > prev.id
      return prev.endedAt !== null && m.at >= prev.endedAt
    })
    return !alreadyNoted
  }

  sendUser(sessionId: string, text: string): MessageRow {
    const msg = this.d.store.addMessage({
      sessionId,
      turnId: null,
      source: 'user',
      text,
      queued: true,
    })
    this.emit({ type: 'message', message: msg })

    if (this.dispatchOpen) {
      try {
        const p = this.launchTurn({
          sessionId,
          text,
          mode: 'chat',
          messageId: msg.id,
        })
        this.track(p.catch(() => {}))
      } catch (err) {
        if (
          !(err instanceof BusyError) &&
          !(err instanceof FrozenError) &&
          !(err instanceof StaleStateError) &&
          !(err instanceof DispatchClosedError)
        ) {
          throw err
        }
      }
    }

    return msg
  }

  runTurn(i: {
    sessionId: string
    text: string
    mode: ClaimMode
    runId?: string
    previousTurnId?: number
  }): Promise<TurnOutcome> {
    try {
      return this.track(
        this.launchTurn({
          sessionId: i.sessionId,
          text: i.text,
          mode: i.mode,
          runId: i.runId,
          previousTurnId: i.previousTurnId,
        }),
      )
    } catch (err) {
      return Promise.reject(err)
    }
  }

  private launchTurn(i: {
    sessionId: string
    text: string
    mode: ClaimMode
    runId?: string
    previousTurnId?: number
    messageId?: number
  }): Promise<TurnOutcome> {
    if (!this.dispatchOpen) {
      throw new DispatchClosedError('dispatch is closed')
    }

    const addNote = i.mode === 'chat' && this.shouldAddInterruptNote(i.sessionId)
    const turn = this.d.store.claimTurn({
      sessionId: i.sessionId,
      mode: i.mode,
      runId: i.runId,
      previousTurnId: i.previousTurnId,
      messageId: i.messageId,
      hostInstanceId: this.d.hostInstanceId,
      bootTime: bootTime(),
    })
    this.emit({ type: 'turn', turn: stripToken(turn) })

    return this.runClaimedTurn(turn, i.text, addNote)
  }

  private async runClaimedTurn(
    turn: TurnRow,
    text: string,
    addNote: boolean,
  ): Promise<TurnOutcome> {
    let sup: Awaited<ReturnType<typeof spawnSupervised>> | null = null

    const abortNeverStarted = () => {
      if (sup) {
        sup.abort()
        sup.child.stdout.destroy()
        sup.child.stderr.destroy()
      }
      if (
        this.d.store.endTurn(turn.id, 'interrupted', {
          exitCode: null,
          endReason: 'never_started',
          writerCheck: 'excluded',
        })
      ) {
        this.emit({ type: 'turn', turn: stripToken(this.d.store.getTurn(turn.id)!) })
      }
    }

    try {
      const session = this.d.store.getSession(turn.sessionId)
      if (!session) throw new Error(`unknown session ${turn.sessionId}`)

      let cwd: string
      let writableRoots: string[]
      let agentSessionId: string | null

      if (turn.runId === null) {
        cwd = realpathSync(session.cwd)
        writableRoots = [gitCommonDir(cwd)]
        agentSessionId = session.agentSessionId
      } else {
        const run = this.d.store.getRun(turn.runId)
        if (!run) throw new Error(`unknown run ${turn.runId}`)
        cwd = realpathSync(run.worktreePath)
        writableRoots = [gitCommonDir(cwd), realpathSync(run.remote)]
        agentSessionId = run.agentSessionId
      }

      const prompt = addNote ? `${INTERRUPT_NOTE}\n\n${text}` : text
      const codexArgs = buildCodexArgs({
        agentSessionId,
        prompt,
        mcpUrl: this.d.mcpUrl,
        writableRoots,
      })

      sup = await spawnSupervised([...this.d.codexCommand, ...codexArgs], {
        cwd,
        env: this.buildEnv(turn.mcpToken),
      })

      const barrierDir = process.env.POLYCLAVE_TEST_BARRIER_BEFORE_AUTHORIZE
      if (barrierDir) {
        mkdirSync(barrierDir, { recursive: true })
        writeFileSync(join(barrierDir, 'reached'), '1')
        const releaseFile = join(barrierDir, 'release')
        while (
          !existsSync(releaseFile) &&
          this.d.store.getTurn(turn.id)?.status === 'running'
        ) {
          await new Promise((r) => setTimeout(r, 20))
        }
      }

      if (!this.dispatchOpen) {
        abortNeverStarted()
        return {
          turn: this.d.store.getTurn(turn.id)!,
          reply: '',
          selfExited: false,
        }
      }

      const startedAt = processStartTime(sup.pgid)
      const authorized = this.d.store.authorizeTurn(turn.id, sup.pgid, startedAt)
      if (!authorized) {
        abortNeverStarted()
        return {
          turn: this.d.store.getTurn(turn.id)!,
          reply: '',
          selfExited: false,
        }
      }

      if (addNote) {
        const noteMsg = this.d.store.addMessage({
          sessionId: turn.sessionId,
          turnId: turn.id,
          source: 'host',
          text: INTERRUPT_NOTE,
        })
        this.emit({ type: 'message', message: noteMsg })
      }

      this.emit({ type: 'turn', turn: stripToken(this.d.store.getTurn(turn.id)!) })

      const activeEntry: ActiveTurnEntry = {
        sessionId: turn.sessionId,
        pgid: sup.pgid,
        cancelRequested: false,
        stdout: sup.child.stdout,
        stderr: sup.child.stderr,
      }
      this.activeTurns.set(turn.id, activeEntry)

      let stdoutBuf = ''
      let stderrBuf = ''
      let reply = ''
      let threadRecorded = agentSessionId !== null

      const processLine = (line: string) => {
        const parsed = parseCodexLine(line)
        if (!parsed) return

        this.d.store.addEvent(turn.id, parsed.type ?? 'unknown', line)

        if (parsed.threadId && !threadRecorded) {
          threadRecorded = true
          if (turn.runId === null) {
            this.d.store.setSessionThread(turn.sessionId, parsed.threadId)
            const s = this.d.store.getSession(turn.sessionId)
            if (s) this.emit({ type: 'session', session: s })
          } else {
            this.d.store.setRunThread(turn.runId, parsed.threadId)
            const r = this.d.store.getRun(turn.runId)
            if (r) this.emit({ type: 'run', run: r })
          }
        }

        if (parsed.event) {
          this.emit({
            type: 'agent',
            sessionId: turn.sessionId,
            turnId: turn.id,
            event: parsed.event,
          })
          if (parsed.event.kind === 'message.done') {
            reply = parsed.event.text
          }
        }
      }

      sup.child.stdout.on('data', (chunk: Buffer) => {
        stdoutBuf += chunk.toString('utf8')
        let nl: number
        while ((nl = stdoutBuf.indexOf('\n')) >= 0) {
          const line = stdoutBuf.slice(0, nl).trim()
          stdoutBuf = stdoutBuf.slice(nl + 1)
          if (line) processLine(line)
        }
      })

      sup.child.stderr.on('data', (chunk: Buffer) => {
        stderrBuf += chunk.toString('utf8')
        if (stderrBuf.length > 8192) {
          stderrBuf = stderrBuf.slice(-4096)
        }
      })

      const exitPromise = new Promise<{ code: number | null; signal: NodeJS.Signals | null }>(
        (resolve) => {
          sup!.child.on('exit', (code, signal) => {
            setImmediate(() => {
              if (stdoutBuf.trim()) {
                processLine(stdoutBuf.trim())
                stdoutBuf = ''
              }
              sup!.child.stdout.destroy()
              sup!.child.stderr.destroy()
              resolve({ code, signal })
            })
          })
        },
      )

      sup.go()

      const { code, signal } = await exitPromise
      this.activeTurns.delete(turn.id)

      const currentTurn = this.d.store.getTurn(turn.id)!
      if (activeEntry.cancelRequested || currentTurn.status !== 'running') {
        return { turn: currentTurn, reply, selfExited: false }
      }

      const exitCode = code ?? (signal ? 1 : 0)
      if (exitCode === 0) {
        this.d.store.endTurn(turn.id, 'completed', {
          exitCode: 0,
          endReason: 'exit',
          writerCheck: 'excluded',
        })
        if (reply) {
          const agentMsg = this.d.store.addMessage({
            sessionId: turn.sessionId,
            turnId: turn.id,
            source: 'agent',
            text: reply,
          })
          this.emit({ type: 'message', message: agentMsg })
        }
      } else {
        this.d.store.endTurn(turn.id, 'failed', {
          exitCode,
          endReason: 'exit',
          writerCheck: 'excluded',
        })
        const tail = stderrBuf.slice(-800)
        const hostMsg = this.d.store.addMessage({
          sessionId: turn.sessionId,
          turnId: turn.id,
          source: 'host',
          text: `Codex failed: ${tail}`,
        })
        this.emit({ type: 'message', message: hostMsg })
      }

      const finalTurn = this.d.store.getTurn(turn.id)!
      this.emit({ type: 'turn', turn: stripToken(finalTurn) })

      if (turn.runId === null) {
        this.drainQueues()
      }

      return { turn: finalTurn, reply, selfExited: true }
    } catch (err) {
      abortNeverStarted()
      throw err
    }
  }

  async cancel(sessionId: string): Promise<boolean> {
    const running = this.d.store.runningTurn(sessionId)
    if (!running) return false

    const active = this.activeTurns.get(running.id)
    if (active) active.cancelRequested = true

    if (running.phase === 'prepared') {
      const ended = this.d.store.endTurn(running.id, 'cancelled', {
        exitCode: null,
        endReason: 'never_started',
        writerCheck: 'excluded',
      })
      if (ended) {
        const updatedTurn = this.d.store.getTurn(running.id)
        if (updatedTurn) this.emit({ type: 'turn', turn: stripToken(updatedTurn) })
      }
      return ended
    }

    const ended = this.d.store.endTurn(running.id, 'cancelled', {
      exitCode: null,
      endReason: 'cancel',
      writerCheck: 'possible',
      freezeReason: `cancelled while an agent was running (turn ${running.id})`,
    })

    if (ended) {
      const updatedTurn = this.d.store.getTurn(running.id)
      if (updatedTurn) this.emit({ type: 'turn', turn: stripToken(updatedTurn) })
      const updatedSession = this.d.store.getSession(sessionId)
      if (updatedSession) this.emit({ type: 'session', session: updatedSession })
    }

    if (running.agentPgid !== null) {
      await killGroup(running.agentPgid)
    }
    if (active) {
      active.stdout?.destroy()
      active.stderr?.destroy()
    }

    return ended
  }

  drainQueues(): void {
    if (!this.dispatchOpen) return
    for (const session of this.d.store.listSessions()) {
      if (session.frozenAt !== null) continue
      if (this.d.store.runningTurn(session.id)) continue
      if (this.d.store.activeRun(session.id)) continue
      const msg = this.d.store.nextQueued(session.id)
      if (!msg) continue
      try {
        const p = this.launchTurn({
          sessionId: session.id,
          text: msg.text,
          mode: 'chat',
          messageId: msg.id,
        })
        this.track(p.catch(() => {}))
      } catch (err) {
        if (
          !(err instanceof BusyError) &&
          !(err instanceof FrozenError) &&
          !(err instanceof StaleStateError) &&
          !(err instanceof DispatchClosedError)
        ) {
          throw err
        }
      }
    }
  }

  async idle(): Promise<void> {
    while (this.pending.size > 0) {
      await Promise.allSettled([...this.pending])
    }
  }

  async shutdown(): Promise<void> {
    this.setDispatch(false)
    await this.idle()
  }

  terminateGroups(): void {
    for (const entry of this.activeTurns.values()) {
      try {
        process.kill(-entry.pgid, 'SIGTERM')
      } catch {}
    }
  }
}
