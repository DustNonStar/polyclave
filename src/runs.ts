import { execFile as execFileCb } from 'node:child_process'
import type { EventEmitter } from 'node:events'
import { existsSync, mkdirSync, realpathSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { promisify } from 'node:util'
import {
  classifyRun,
  preflightTarget,
  prepareWorktree,
  resolveBaseSha,
  WorktreeConflictError,
  type BranchCheck,
  type checkRunBranch,
} from './git-effect.ts'
import { DispatchClosedError, type TurnOutcome, type TurnRunner } from './runner.ts'
import {
  BusyError,
  ConflictError,
  FrozenError,
  StaleStateError,
  type RunRow,
  type Store,
} from './store.ts'

const execFile = promisify(execFileCb)

export type Resolution =
  | { decision: 'accept'; sha: string; note: string }
  | { decision: 'fail' | 'allow_retry'; note: string }

function shellQuote(s: string): string {
  if (/^[A-Za-z0-9_@%+=:,./-]+$/.test(s)) return s
  return `'${s.replace(/'/g, "'\\''")}'`
}

export function runBrief(run: RunRow, resumed: boolean): string {
  const prefix = resumed
    ? 'This task was interrupted earlier and its branch is not on the remote. Check this worktree first and reuse any existing commits instead of redoing them.\n\n'
    : ''
  const remote = shellQuote(run.remote)
  return (
    `${prefix}Task ${run.id}. ${run.instructions}\n` +
    `You are in a worktree on branch ${run.branch}, created from commit ${run.baseSha}. Commit your work there. Every commit needs a summary line, a blank line, and the git trailer "Run-Id: ${run.id}" at the end; for example: git commit -m "<summary>" --trailer "Run-Id: ${run.id}". Push with: git push ${remote} refs/heads/${run.branch}:refs/heads/${run.branch}\n` +
    `When the branch is pushed, call the submit_result tool of the host MCP server with {"run_id": "${run.id}", "branch": "${run.branch}", "sha": "<pushed commit sha>"}. Then reply with one sentence.`
  )
}

export function correctionText(run: RunRow, reason: string): string {
  const remote = shellQuote(run.remote)
  return (
    `Task ${run.id}: the host has no accepted result yet (${reason}). Nothing has been pushed for it. ` +
    `Finish the task in this worktree, commit with the trailer "Run-Id: ${run.id}", ` +
    `push with: git push ${remote} refs/heads/${run.branch}:refs/heads/${run.branch} ` +
    `and then call submit_result with {"run_id": "${run.id}", "branch": "${run.branch}", "sha": "<pushed commit sha>"}.`
  )
}

async function verifyRemoteAccept(run: RunRow, sha: string): Promise<{ tip: string } | { absent: true }> {
  const realRemote = realpathSync(run.remote)
  let tip: string
  try {
    const { stdout } = await execFile(
      'git',
      ['-C', realRemote, 'rev-parse', '--verify', '--quiet', `refs/heads/${run.branch}`],
      { encoding: 'utf8' },
    )
    tip = stdout.trim()
  } catch (err) {
    const e = err as { code?: number | string; stdout?: string; stderr?: string }
    if (e.code === 1 && !(e.stdout ?? '').trim() && !(e.stderr ?? '').trim()) {
      return { absent: true }
    }
    throw err
  }
  if (tip !== sha) {
    return { tip }
  }
  await execFile('git', ['-C', realRemote, 'merge-base', '--is-ancestor', run.baseSha, tip])
  const { stdout: countOut } = await execFile(
    'git',
    ['-C', realRemote, 'rev-list', '--count', `${run.baseSha}..${tip}`],
    { encoding: 'utf8' },
  )
  if (Number(countOut.trim()) === 0) {
    throw new StaleStateError('no commits after base')
  }
  return { tip }
}

function resolutionText(decision: string, note: string, seen: string, at: string): string {
  const trimmed = note.trim()
  return `${trimmed ? `${decision}: ${trimmed}` : decision} (seen ${seen} at ${at})`
}

export class Runs {
  private readonly d: {
    store: Store
    runner: TurnRunner
    stateDir: string
    check: typeof checkRunBranch
    bus: EventEmitter
  }
  private readonly inFlight = new Set<string>()
  private readonly settling = new Set<Promise<RunRow>>()

  constructor(d: {
    store: Store
    runner: TurnRunner
    stateDir: string
    check: typeof checkRunBranch
    bus: EventEmitter
  }) {
    this.d = d
  }

  private emitRun(run: RunRow): void {
    this.d.bus.emit('event', { type: 'run', run })
  }

  private async safeCheck(
    run: RunRow,
  ): Promise<BranchCheck | { state: 'error'; message: string }> {
    try {
      return await this.d.check({
        remote: run.remote,
        baseSha: run.baseSha,
        branch: run.branch,
        runId: run.id,
      })
    } catch (err) {
      return {
        state: 'error',
        message: err instanceof Error ? err.message : String(err),
      }
    }
  }

  async idle(): Promise<void> {
    while (this.settling.size > 0 || !this.settlingAndRunnerIdle()) {
      await Promise.allSettled([...this.settling])
      await this.d.runner.idle()
    }
  }

  private settlingAndRunnerIdle(): boolean {
    return this.settling.size === 0
  }

  async create(i: {
    sessionId: string
    requestId: string
    instructions: string
    base?: string
  }): Promise<{ run: RunRow; created: boolean }> {
    const base = (i.base ?? 'main').trim()
    const replay = this.d.store.replayRun({
      sessionId: i.sessionId,
      requestId: i.requestId,
      instructions: i.instructions,
      base,
    })
    if (replay) {
      return { run: replay, created: false }
    }

    const session = this.d.store.getSession(i.sessionId)
    if (!session) throw new Error(`unknown session ${i.sessionId}`)
    if (session.frozenAt !== null) {
      throw new FrozenError(session.frozenReason ?? 'session is frozen')
    }
    if (this.d.store.runningTurn(i.sessionId) || this.d.store.activeRun(i.sessionId)) {
      throw new BusyError('session is busy')
    }

    const { remote } = await preflightTarget(session.cwd)
    const baseSha = await resolveBaseSha(remote, base)
    const realState = realpathSync(this.d.stateDir)

    const res = this.d.store.acceptRun({
      sessionId: i.sessionId,
      requestId: i.requestId,
      instructions: i.instructions,
      remote,
      base,
      baseSha,
      worktreePath: (id) => join(realState, 'worktrees', id),
    })

    const barrierDir = process.env.POLYCLAVE_TEST_BARRIER_AFTER_ACCEPT
    if (barrierDir) {
      mkdirSync(barrierDir, { recursive: true })
      writeFileSync(join(barrierDir, 'reached'), '1')
      const releaseFile = join(barrierDir, 'release')
      while (!existsSync(releaseFile)) {
        await new Promise((r) => setTimeout(r, 20))
      }
    }

    if (res.created) {
      this.emitRun(res.run)
    }
    return res
  }

  async start(runId: string): Promise<RunRow> {
    if (this.inFlight.has(runId)) {
      throw new BusyError(`run ${runId} already has an operation in progress`)
    }
    this.inFlight.add(runId)
    try {
      const run = this.d.store.getRun(runId)
      if (!run) throw new Error(`unknown run ${runId}`)
      if (run.status !== 'pending') {
        throw new StaleStateError(`run ${runId} is ${run.status}, expected pending`)
      }
      const session = this.d.store.getSession(run.sessionId)
      if (!session) throw new Error(`unknown session ${run.sessionId}`)
      if (session.frozenAt !== null) {
        throw new FrozenError(session.frozenReason ?? 'session is frozen')
      }
      if (this.d.store.runningTurn(run.sessionId)) {
        throw new BusyError('session has a running turn')
      }

      try {
        await prepareWorktree({
          cwd: session.cwd,
          remote: run.remote,
          baseSha: run.baseSha,
          branch: run.branch,
          path: run.worktreePath,
        })
      } catch (err) {
        const updated =
          err instanceof WorktreeConflictError
            ? this.d.store.updateRun(
                run.id,
                { status: 'needs_review', detail: err.message },
                ['pending'],
              )
            : this.d.store.updateRun(
                run.id,
                {
                  status: 'interrupted',
                  detail: `worktree preparation failed: ${err instanceof Error ? err.message : String(err)}`,
                },
                ['pending'],
              )
        this.emitRun(updated)
        this.d.runner.drainQueues()
        return updated
      }

      const turnPromise = this.d.runner.runTurn({
        sessionId: run.sessionId,
        text: runBrief(run, false),
        mode: 'start',
        runId: run.id,
      })

      if (this.d.store.getRun(run.id)!.status !== 'running') {
        await turnPromise
      }
      this.emitRun(this.d.store.getRun(run.id)!)

      const settleP = this.settleLoop(run.id, turnPromise)
      this.settling.add(settleP)
      void settleP.then(
        () => this.settling.delete(settleP),
        () => this.settling.delete(settleP),
      )
      return await settleP
    } finally {
      this.inFlight.delete(runId)
    }
  }

  async retry(runId: string): Promise<RunRow> {
    if (this.inFlight.has(runId)) {
      throw new BusyError(`run ${runId} already has an operation in progress`)
    }
    this.inFlight.add(runId)
    try {
      const run = this.d.store.getRun(runId)
      if (!run) throw new Error(`unknown run ${runId}`)
      if (run.status !== 'interrupted') {
        throw new StaleStateError(`run ${runId} is ${run.status}, expected interrupted`)
      }
      const session = this.d.store.getSession(run.sessionId)
      if (!session) throw new Error(`unknown session ${run.sessionId}`)
      if (session.frozenAt !== null) {
        throw new FrozenError(session.frozenReason ?? 'session is frozen')
      }
      if (this.d.store.runningTurn(run.sessionId)) {
        throw new BusyError('session has a running turn')
      }

      if (run.retries >= 3) {
        const updated = this.d.store.updateRun(
          run.id,
          { status: 'needs_review', detail: 'retry limit reached' },
          ['interrupted'],
        )
        this.emitRun(updated)
        this.d.runner.drainQueues()
        return updated
      }

      const remote = await this.safeCheck(run)
      const verdict = classifyRun({
        writer: 'excluded',
        accepted: this.d.store.acceptedResult(run.id),
        remote,
      })

      if (verdict.verdict === 'done') {
        const updated = this.d.store.updateRun(
          run.id,
          {
            status: 'done',
            resultSha: verdict.sha,
            observedSha: verdict.sha,
            resultSource: 'agent',
            detail: null,
          },
          ['interrupted'],
        )
        this.emitRun(updated)
        this.d.runner.drainQueues()
        return updated
      }

      if (verdict.verdict === 'needs_review') {
        const updated = this.d.store.updateRun(
          run.id,
          {
            status: 'needs_review',
            detail: verdict.reason,
            observedSha: verdict.observedSha,
          },
          ['interrupted'],
        )
        this.emitRun(updated)
        this.d.runner.drainQueues()
        return updated
      }

      if (verdict.verdict === 'check_failed') {
        const updated = this.d.store.updateRun(
          run.id,
          {
            status: 'interrupted',
            detail: `remote check failed: ${verdict.message}`,
          },
          ['interrupted'],
        )
        this.emitRun(updated)
        return updated
      }

      try {
        await prepareWorktree({
          cwd: session.cwd,
          remote: run.remote,
          baseSha: run.baseSha,
          branch: run.branch,
          path: run.worktreePath,
        })
      } catch (err) {
        const updated =
          err instanceof WorktreeConflictError
            ? this.d.store.updateRun(
                run.id,
                { status: 'needs_review', detail: err.message },
                ['interrupted'],
              )
            : this.d.store.updateRun(
                run.id,
                {
                  status: 'interrupted',
                  detail: `worktree preparation failed: ${err instanceof Error ? err.message : String(err)}`,
                },
                ['interrupted'],
              )
        this.emitRun(updated)
        this.d.runner.drainQueues()
        return updated
      }

      const turnPromise = this.d.runner.runTurn({
        sessionId: run.sessionId,
        text: runBrief(run, true),
        mode: 'retry',
        runId: run.id,
      })

      const claimedRun = this.d.store.getRun(run.id)!
      if (claimedRun.status !== 'running') {
        await turnPromise
      }
      this.emitRun(claimedRun)

      const settleP = this.settleLoop(run.id, turnPromise)
      this.settling.add(settleP)
      void settleP.then(
        () => this.settling.delete(settleP),
        () => this.settling.delete(settleP),
      )

      return claimedRun
    } finally {
      this.inFlight.delete(runId)
    }
  }

  private async settleLoop(
    runId: string,
    initialTurnPromise: Promise<TurnOutcome>,
  ): Promise<RunRow> {
    let currentPromise = initialTurnPromise

    for (;;) {
      let outcome: TurnOutcome | null = null
      let launchFailed = false
      try {
        outcome = await currentPromise
      } catch (err) {
        if (err instanceof DispatchClosedError) {
          const updated = this.d.store.updateRun(
            runId,
            {
              status: 'interrupted',
              detail: 'host stopped before completion; safe to retry',
            },
            ['running'],
          )
          this.emitRun(updated)
          this.d.runner.drainQueues()
          return updated
        }
        launchFailed = true
      }

      const run = this.d.store.getRun(runId)!
      const turn = outcome ? outcome.turn : this.d.store.lastRunTurn(runId)!
      const writer = turn.writerCheck ?? 'excluded'
      const remote = await this.safeCheck(run)
      const verdict = classifyRun({
        writer,
        accepted: this.d.store.acceptedResult(runId),
        remote,
      })

      if (verdict.verdict === 'done') {
        const updated = this.d.store.updateRun(
          runId,
          {
            status: 'done',
            resultSha: verdict.sha,
            observedSha: verdict.sha,
            resultSource: 'agent',
            detail: null,
          },
          ['running'],
        )
        this.emitRun(updated)
        this.d.runner.drainQueues()
        return updated
      }

      if (verdict.verdict === 'needs_review') {
        const detail =
          turn.status === 'cancelled'
            ? `${verdict.reason}: cancelled while the agent was running`
            : verdict.reason
        const updated = this.d.store.updateRun(
          runId,
          {
            status: 'needs_review',
            detail,
            observedSha: verdict.observedSha,
          },
          ['running'],
        )
        if (verdict.freeze) {
          this.d.store.freezeSession(run.sessionId, detail)
          const sess = this.d.store.getSession(run.sessionId)
          if (sess) this.d.bus.emit('event', { type: 'session', session: sess })
        }
        this.emitRun(updated)
        this.d.runner.drainQueues()
        return updated
      }

      if (verdict.verdict === 'check_failed') {
        const updated = this.d.store.updateRun(
          runId,
          {
            status: 'interrupted',
            detail: `remote check failed: ${verdict.message}`,
          },
          ['running'],
        )
        this.emitRun(updated)
        this.d.runner.drainQueues()
        return updated
      }

      // verdict === 'retryable'
      if (launchFailed || turn.endReason === 'never_started') {
        const detail = !this.d.runner.isDispatchOpen()
          ? 'host stopped before completion; safe to retry'
          : 'turn failed to start; safe to retry'
        const updated = this.d.store.updateRun(
          runId,
          { status: 'interrupted', detail },
          ['running'],
        )
        this.emitRun(updated)
        this.d.runner.drainQueues()
        return updated
      }

      if (turn.exitCode === 0 && !this.d.runner.isDispatchOpen()) {
        const updated = this.d.store.updateRun(
          runId,
          {
            status: 'interrupted',
            detail: 'host stopped before completion; safe to retry',
          },
          ['running'],
        )
        this.emitRun(updated)
        this.d.runner.drainQueues()
        return updated
      }

      if (turn.exitCode === 0) {
        if (run.corrections < 2) {
          const updated = this.d.store.updateRun(
            runId,
            { corrections: run.corrections + 1 },
            ['running'],
          )
          this.emitRun(updated)
          const reason = this.d.store.lastRejectedError(runId) ?? 'no result submitted'
          currentPromise = this.d.runner.runTurn({
            sessionId: run.sessionId,
            text: correctionText(updated, reason),
            mode: 'correction',
            runId: run.id,
            previousTurnId: turn.id,
          })
          continue
        }
        const updated = this.d.store.updateRun(
          runId,
          {
            status: 'failed',
            detail: 'no accepted result after 2 corrections',
          },
          ['running'],
        )
        this.emitRun(updated)
        this.d.runner.drainQueues()
        return updated
      }

      const updated = this.d.store.updateRun(
        runId,
        { status: 'failed', detail: 'turn failed' },
        ['running'],
      )
      this.emitRun(updated)
      this.d.runner.drainQueues()
      return updated
    }
  }

  async resolve(runId: string, r: Resolution): Promise<RunRow> {
    if (this.inFlight.has(runId)) {
      throw new BusyError(`run ${runId} already has an operation in progress`)
    }
    this.inFlight.add(runId)
    try {
      const run = this.d.store.getRun(runId)
      if (!run) throw new Error(`unknown run ${runId}`)
      const session = this.d.store.getSession(run.sessionId)
      if (!session) throw new Error(`unknown session ${run.sessionId}`)
      if (session.frozenAt !== null) {
        throw new FrozenError(session.frozenReason ?? 'session is frozen')
      }
      if (run.status !== 'needs_review') {
        throw new StaleStateError(`run ${runId} is ${run.status}, expected needs_review`)
      }

      const now = new Date().toISOString()

      if (r.decision === 'accept') {
        const verify = await verifyRemoteAccept(run, r.sha)
        if ('absent' in verify) {
          this.d.store.updateRun(runId, { observedSha: null }, ['needs_review'])
          this.emitRun(this.d.store.getRun(runId)!)
          throw new StaleStateError('branch is absent on remote')
        }
        if (verify.tip !== r.sha) {
          const updated = this.d.store.updateRun(
            runId,
            { observedSha: verify.tip },
            ['needs_review'],
          )
          this.emitRun(updated)
          throw new StaleStateError(`remote tip moved to ${verify.tip}`)
        }

        const updated = this.d.store.updateRun(
          runId,
          {
            status: 'done',
            resultSha: r.sha,
            observedSha: r.sha,
            resultSource: 'user',
            detail: null,
            resolution: resolutionText('accept', r.note, r.sha.slice(0, 8), now),
          },
          ['needs_review'],
        )
        this.emitRun(updated)
        this.d.runner.drainQueues()
        return updated
      }

      if (r.decision === 'fail') {
        const seen = run.observedSha ? run.observedSha.slice(0, 8) : 'none'
        const updated = this.d.store.updateRun(
          runId,
          {
            status: 'failed',
            detail: null,
            resolution: resolutionText('fail', r.note, seen, now),
          },
          ['needs_review'],
        )
        this.emitRun(updated)
        this.d.runner.drainQueues()
        return updated
      }

      const check = await this.d.check({
        remote: run.remote,
        baseSha: run.baseSha,
        branch: run.branch,
        runId: run.id,
      })
      if (check.state !== 'absent') {
        throw new ConflictError('cannot allow retry while remote branch exists')
      }

      const updated = this.d.store.updateRun(
        runId,
        {
          status: 'interrupted',
          detail: 'allowed retry by user',
          resolution: resolutionText('allow_retry', r.note, 'none', now),
        },
        ['needs_review'],
      )
      this.emitRun(updated)
      this.d.runner.drainQueues()
      return updated
    } finally {
      this.inFlight.delete(runId)
    }
  }
}
