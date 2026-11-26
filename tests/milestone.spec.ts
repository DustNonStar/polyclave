import { afterEach, describe, expect, it } from 'vitest'
import { execFileSync, spawn, type ChildProcessByStdio } from 'node:child_process'
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { DatabaseSync } from 'node:sqlite'
import type { Readable, Writable } from 'node:stream'
import { fileURLToPath } from 'node:url'
import { resolveBaseSha } from '../src/git-effect.ts'
import { groupLiveness, processStartTime } from '../src/proc.ts'
import type { RecoveryReport } from '../src/recovery.ts'
import { Store, type MessageRow, type RunRow, type SessionRow } from '../src/store.ts'
import { barrier, makeRepo, tempDir, waitFor } from './helpers.ts'

const HARNESS = fileURLToPath(new URL('./fixtures/host-harness.ts', import.meta.url))

const temps: string[] = []
const procs: ChildProcessByStdio<Writable | null, Readable, Readable>[] = []
const extraPids: number[] = []

function trackTemp(d: string): string {
  temps.push(d)
  return d
}

function isPidAlive(pid: number): boolean {
  try {
    process.kill(pid, 0)
    return true
  } catch {
    return false
  }
}

afterEach(async () => {
  while (procs.length > 0) {
    const p = procs.pop()!
    try {
      p.kill('SIGKILL')
    } catch {}
  }
  while (extraPids.length > 0) {
    const pid = extraPids.pop()!
    try {
      process.kill(-pid, 'SIGKILL')
    } catch {}
    try {
      process.kill(pid, 'SIGKILL')
    } catch {}
  }
  while (temps.length > 0) {
    rmSync(temps.pop()!, { recursive: true, force: true })
  }
})

type FakeLogEntry = {
  event: string
  turn?: number
  pid?: number
  prompt?: string
}

function readFakeLog(path: string): FakeLogEntry[] {
  if (!existsSync(path)) return []
  return readFileSync(path, 'utf8')
    .trim()
    .split('\n')
    .filter(Boolean)
    .map((l) => JSON.parse(l) as FakeLogEntry)
}

function readRefLog(path: string): string[] {
  if (!existsSync(path)) return []
  return readFileSync(path, 'utf8')
    .trim()
    .split('\n')
    .filter(Boolean)
}

function waitExit(
  child: ChildProcessByStdio<Writable | null, Readable, Readable>,
): Promise<void> {
  if (child.exitCode !== null || child.signalCode !== null) {
    return Promise.resolve()
  }
  return new Promise<void>((resolve) => {
    child.once('exit', () => resolve())
  })
}

async function killAndWait(
  child: ChildProcessByStdio<Writable | null, Readable, Readable>,
): Promise<void> {
  const exited = waitExit(child)
  child.kill('SIGKILL')
  await exited
}

function childPids(parentPid: number): number[] {
  try {
    const out = execFileSync('pgrep', ['-P', String(parentPid)], {
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'pipe'],
    }).trim()
    return out
      .split(/\s+/)
      .filter(Boolean)
      .map(Number)
      .filter((n) => Number.isInteger(n) && n > 0)
  } catch {
    return []
  }
}

function spawnHarness(
  stateDir: string,
  args: string[],
  env: Record<string, string | undefined>,
) {
  const child = spawn(process.execPath, [HARNESS, stateDir, ...args], {
    env: { ...process.env, ...env },
    stdio: ['pipe', 'pipe', 'pipe'],
  })
  procs.push(child)
  return child
}

async function runHarnessOnce<T>(
  stateDir: string,
  args: string[],
  env: Record<string, string | undefined>,
): Promise<{ code: number | null; stdout: string; stderr: string; json: T | null }> {
  const child = spawn(process.execPath, [HARNESS, stateDir, ...args], {
    env: { ...process.env, ...env },
    stdio: ['ignore', 'pipe', 'pipe'],
  })
  procs.push(child)
  let stdout = ''
  let stderr = ''
  child.stdout.on('data', (d) => {
    stdout += d.toString()
  })
  child.stderr.on('data', (d) => {
    stderr += d.toString()
  })
  const code = await new Promise<number | null>((resolve) => {
    child.on('exit', (c) => resolve(c))
  })
  let json: T | null = null
  const lines = stdout.trim().split('\n').filter(Boolean)
  const last = lines.at(-1)
  if (last) {
    try {
      json = JSON.parse(last) as T
    } catch {}
  }
  return { code, stdout, stderr, json }
}

class ServeClient {
  private readonly child: ChildProcessByStdio<Writable, Readable, Readable>
  private nextId = 1
  private readonly waiters = new Map<
    number,
    { resolve: (v: { ok: boolean; result?: unknown; error?: string; errorType?: string }) => void }
  >()
  ready!: { ready: true; mcpUrl: string; report: RecoveryReport }

  private constructor(child: ChildProcessByStdio<Writable, Readable, Readable>) {
    this.child = child
  }

  static async start(
    stateDir: string,
    env: Record<string, string | undefined>,
  ): Promise<ServeClient> {
    const child = spawn(process.execPath, [HARNESS, stateDir, 'serve'], {
      env: { ...process.env, ...env },
      stdio: ['pipe', 'pipe', 'pipe'],
    })
    procs.push(child)
    const client = new ServeClient(child)
    let buf = ''
    let resolveReady!: (v: ServeClient['ready']) => void
    let rejectReady!: (e: Error) => void
    const readyPromise = new Promise<ServeClient['ready']>((res, rej) => {
      resolveReady = res
      rejectReady = rej
    })
    let stderr = ''
    child.stderr.on('data', (d) => {
      stderr += d.toString()
    })
    child.on('exit', (c) => {
      rejectReady(new Error(`serve exited prematurely (code=${c}): ${stderr}`))
    })
    child.stdout.on('data', (d) => {
      buf += d.toString()
      let nl: number
      while ((nl = buf.indexOf('\n')) >= 0) {
        const line = buf.slice(0, nl).trim()
        buf = buf.slice(nl + 1)
        if (!line) continue
        const msg = JSON.parse(line) as Record<string, unknown>
        if (msg.ready === true) {
          resolveReady(msg as ServeClient['ready'])
        } else if (typeof msg.id === 'number') {
          const w = client.waiters.get(msg.id)
          if (w) {
            client.waiters.delete(msg.id)
            w.resolve(
              msg as { ok: boolean; result?: unknown; error?: string; errorType?: string },
            )
          }
        }
      }
    })
    client.ready = await readyPromise
    return client
  }

  call<T = Record<string, unknown>>(op: string, args: Record<string, unknown> = {}): Promise<T> {
    const id = this.nextId++
    return new Promise<T>((resolve, reject) => {
      this.waiters.set(id, {
        resolve: (msg) => {
          if (msg.ok) resolve(msg.result as T)
          else reject(new Error(`${msg.errorType ?? 'Error'}: ${msg.error}`))
        },
      })
      this.child.stdin.write(JSON.stringify({ id, op, ...args }) + '\n')
    })
  }

  kill(): void {
    this.child.kill('SIGKILL')
  }

  async killAndWait(): Promise<void> {
    await killAndWait(this.child)
  }

  async close(): Promise<void> {
    const exitP = new Promise<void>((resolve) => {
      this.child.on('exit', () => resolve())
    })
    this.child.stdin.end()
    await exitP
  }
}

function makeFixture(prefix: string, script: Record<string, unknown>) {
  const root = trackTemp(tempDir(prefix))
  const stateDir = join(root, 'state')
  const userCodex = join(root, 'user-codex')
  mkdirSync(stateDir, { recursive: true })
  mkdirSync(userCodex, { recursive: true })
  writeFileSync(join(userCodex, 'auth.json'), '{}')

  const { work, remote, refLog } = makeRepo(root)
  const scriptPath = join(root, 'script.json')
  const logPath = join(root, 'fake.log')
  writeFileSync(scriptPath, JSON.stringify(script))

  const env: Record<string, string | undefined> = {
    FAKE_CODEX_SCRIPT: scriptPath,
    FAKE_CODEX_LOG: logPath,
    FAKE_CODEX_BARRIER_DIR: root,
    POLYCLAVE_TEST_USER_CODEX_HOME: userCodex,
    POLYCLAVE_TEST_BOOT_TIME: '100.0',
  }

  return { root, stateDir, work, remote, refLog, scriptPath, logPath, env }
}

describe('milestone crash experiments', () => {
  it('same request twice, lost response, and a kill before the first turn', async () => {
    const { root, stateDir, work, logPath, env } = makeFixture('ms-1-', {
      '1': [
        { commit: { file: 'done.txt', content: 'ok\n', trailer: true } },
        { push: true },
        { submitPushed: true },
        { reply: 'done' },
      ],
    })

    const s1 = await ServeClient.start(stateDir, env)
    const { session } = await s1.call<{ session: SessionRow }>('create-session', {
      cwd: work,
      name: 's1',
    })
    const [r1, r2] = await Promise.all([
      s1.call<{ run: RunRow; created: boolean }>('create-run', {
        sessionId: session.id,
        requestId: 'dup-req',
        instructions: 'do A',
      }),
      s1.call<{ run: RunRow; created: boolean }>('create-run', {
        sessionId: session.id,
        requestId: 'dup-req',
        instructions: 'do A',
      }),
    ])
    expect(r1.run.id).toBe(r2.run.id)
    expect([r1.created, r2.created].sort()).toEqual([false, true])
    await s1.close()

    // Now test kill at POLYCLAVE_TEST_BARRIER_AFTER_ACCEPT on a fresh request
    const bAfter = barrier(root, 'after-accept')
    const s2 = await ServeClient.start(stateDir, {
      ...env,
      POLYCLAVE_TEST_BARRIER_AFTER_ACCEPT: bAfter.dir,
    })
    const createP = s2.call<{ run: RunRow; created: boolean }>('create-run', {
      sessionId: session.id,
      requestId: 'lost-req',
      instructions: 'do B',
    })
    createP.catch(() => {})
    await bAfter.waitReached()
    await s2.killAndWait()

    // Restart and resend lost-req
    const s3 = await ServeClient.start(stateDir, env)
    expect(s3.ready.report.frozenSessions).toEqual([])

    const resend = await s3.call<{ run: RunRow; created: boolean }>('create-run', {
      sessionId: session.id,
      requestId: 'lost-req',
      instructions: 'do B',
    })
    expect(resend.created).toBe(false)
    expect(resend.run.status).toBe('interrupted')
    expect(existsSync(resend.run.worktreePath)).toBe(false)
    expect(readFakeLog(logPath).filter((e) => e.event === 'prompt_started')).toHaveLength(0)

    // Two concurrent retries -> only one turn starts
    const [t1, t2] = await Promise.allSettled([
      s3.call<{ run: RunRow }>('retry', { runId: resend.run.id }),
      s3.call<{ run: RunRow }>('retry', { runId: resend.run.id }),
    ])
    expect([t1, t2].filter((x) => x.status === 'fulfilled')).toHaveLength(1)
    expect([t1, t2].filter((x) => x.status === 'rejected')).toHaveLength(1)

    await s3.call('idle')
    const state = await s3.call<{ runs: RunRow[] }>('get-state')
    const finalRun = state.runs.find((r) => r.id === resend.run.id)!
    expect(finalRun.status).toBe('done')
    expect(existsSync(finalRun.worktreePath)).toBe(true)
    expect(readFakeLog(logPath).filter((e) => e.event === 'prompt_started')).toHaveLength(1)
    await s3.close()
  })

  it('a detached tool writes after the host and the group are gone', async () => {
    const { root, stateDir, work, remote, refLog, logPath, env } = makeFixture('ms-2-', {
      '1': [
        { detachedWriter: { barrier: 'det-b', file: 'sneaky.txt' } },
        { wait: 'hold-b' },
      ],
      '2': [{ reply: 'should never run' }],
    })
    const detBarrier = barrier(root, 'det-b')
    const holdBarrier = barrier(root, 'hold-b')

    const harness = spawnHarness(stateDir, ['start-run', work, 'req-det', 'write later'], env)
    await waitFor(() => detBarrier.reachedPath && existsSync(detBarrier.reachedPath))
    await holdBarrier.waitReached()

    const store = new Store(join(stateDir, 'polyclave.sqlite'))
    const turn = store.runningTurns()[0]!
    const pgid = turn.agentPgid!
    const startedAt = turn.agentStartedAt!
    const detPid = readFakeLog(logPath).find((e) => e.event === 'detached_writer')!.pid!
    extraPids.push(detPid)
    store.close()

    await killAndWait(harness)
    // Supervisor loses owner and terminates the group; confirm group is empty
    await waitFor(() => groupLiveness(pgid, startedAt) === 'gone')

    const rec = await runHarnessOnce<{
      report: RecoveryReport
      runs: RunRow[]
      sessions: SessionRow[]
    }>(stateDir, ['recover'], env)
    expect(rec.code).toBe(0)
    expect(rec.json!.sessions[0]?.frozenAt).toBeTruthy()
    expect(rec.json!.runs[0]?.status).toBe('needs_review')
    expect(rec.json!.runs[0]?.observedSha).toBeNull()
    expect(rec.json!.runs[0]?.detail).toBe('an agent process may still be writing')
    expect(readFakeLog(logPath).filter((e) => e.event === 'prompt_started')).toHaveLength(1)

    // Release detached writer AFTER group is gone and recovery already froze session
    detBarrier.release()
    const wtFile = join(rec.json!.runs[0]!.worktreePath, 'sneaky.txt')
    await waitFor(() => existsSync(wtFile) && readRefLog(refLog).length === 1)
    expect(readFileSync(wtFile, 'utf8')).toBe('detached write\n')
    expect(
      execFileSync('git', ['-C', remote, 'rev-parse', '--verify', `refs/heads/${rec.json!.runs[0]!.branch}`], {
        encoding: 'utf8',
      }).trim(),
    ).toMatch(/^[0-9a-f]{40}$/)

    // Second recovery still runs zero prompts and keeps observedSha and detail unchanged
    const rec2 = await runHarnessOnce<{
      report: RecoveryReport
      runs: RunRow[]
    }>(stateDir, ['recover'], env)
    expect(rec2.json!.runs[0]?.status).toBe('needs_review')
    expect(rec2.json!.runs[0]?.observedSha).toBe(rec.json!.runs[0]?.observedSha)
    expect(rec2.json!.runs[0]?.detail).toBe(rec.json!.runs[0]?.detail)
    expect(readFakeLog(logPath).filter((e) => e.event === 'prompt_started')).toHaveLength(1)
  })

  it('supervisor killed, a member left behind', async () => {
    const memberPidFile = join(tempDir('ms-3-pid-'), 'member.pid')
    temps.push(join(memberPidFile, '..'))
    const { root, stateDir, work, env } = makeFixture('ms-3-', {
      '1': [{ groupMember: { pidFile: memberPidFile } }, { wait: 'hold-sup' }],
    })
    const bHold = barrier(root, 'hold-sup')

    const harness = spawnHarness(stateDir, ['start-run', work, 'req-sup', 'leave member'], env)
    await bHold.waitReached()
    await waitFor(() => existsSync(memberPidFile) && readFileSync(memberPidFile, 'utf8').trim() !== '')
    const memberPid = Number(readFileSync(memberPidFile, 'utf8').trim())
    extraPids.push(memberPid)

    const store = new Store(join(stateDir, 'polyclave.sqlite'))
    const turn = store.runningTurns()[0]!
    const pgid = turn.agentPgid!
    const startedAt = turn.agentStartedAt!
    store.close()
    extraPids.push(pgid)

    await killAndWait(harness)
    try {
      process.kill(pgid, 'SIGKILL')
    } catch {}
    await waitFor(() => !isPidAlive(pgid))
    expect(isPidAlive(memberPid)).toBe(true)
    expect(groupLiveness(pgid, startedAt)).toBe('unverifiable')

    const rec = await runHarnessOnce<{
      report: RecoveryReport
      runs: RunRow[]
      sessions: SessionRow[]
    }>(stateDir, ['recover'], env)
    expect(rec.json!.report.unverifiableGroups).toEqual([pgid])
    expect(rec.json!.report.killedGroups).toEqual([])
    expect(rec.json!.report.turns[0]?.outcome).toBe('unverifiable')
    expect(isPidAlive(memberPid)).toBe(true)
    expect(rec.json!.sessions[0]?.frozenAt).toBeTruthy()
    expect(rec.json!.runs[0]?.status).toBe('needs_review')
  })

  it('half done, pushed, host killed', async () => {
    const { root, stateDir, work, remote, refLog, logPath, env } = makeFixture('ms-4-', {
      '1': [
        { commit: { file: 'half.txt', content: 'half\n', trailer: true } },
        { push: true },
        { wait: 'after-push' },
      ],
    })
    const bAfterPush = barrier(root, 'after-push')
    const harness = spawnHarness(stateDir, ['start-run', work, 'req-half', 'half done'], env)
    await bAfterPush.waitReached()

    harness.kill('SIGKILL')
    await waitFor(() => readRefLog(refLog).length === 1)

    const rec1 = await runHarnessOnce<{ runs: RunRow[]; sessions: SessionRow[] }>(
      stateDir,
      ['recover'],
      env,
    )
    const rec2 = await runHarnessOnce<{ runs: RunRow[]; sessions: SessionRow[] }>(
      stateDir,
      ['recover'],
      env,
    )

    const run = rec2.json!.runs[0]!
    const tip = execFileSync('git', ['-C', remote, 'rev-parse', `refs/heads/${run.branch}`], {
      encoding: 'utf8',
    }).trim()
    expect(run.status).toBe('needs_review')
    expect(run.observedSha).toBe(tip)
    expect(run.corrections).toBe(0)
    expect(rec1.json!.sessions[0]?.frozenAt).toBeTruthy()
    expect(readFakeLog(logPath).filter((e) => e.event === 'push_attempt')).toHaveLength(1)
    expect(readFakeLog(logPath).filter((e) => e.event === 'prompt_started')).toHaveLength(1)
    expect(readRefLog(refLog)).toHaveLength(1)
  })

  it('killed before authorization', async () => {
    const { root, stateDir, work, remote, logPath, env } = makeFixture('ms-5-', {
      '1': [
        { commit: { file: 'clean.txt', content: 'clean\n', trailer: true } },
        { push: true },
        { submitPushed: true },
        { reply: 'done' },
      ],
    })
    const bBeforeAuth = barrier(root, 'before-auth')
    const harness = spawnHarness(stateDir, ['start-run', work, 'req-preauth', 'do clean'], {
      ...env,
      POLYCLAVE_TEST_BARRIER_BEFORE_AUTHORIZE: bBeforeAuth.dir,
    })
    await bBeforeAuth.waitReached()
    const supPids = childPids(harness.pid!)
    expect(supPids.length).toBeGreaterThan(0)
    await killAndWait(harness)
    for (const pid of supPids) {
      await waitFor(() => !isPidAlive(pid) && groupLiveness(pid, null) === 'gone')
    }

    expect(readFakeLog(logPath).filter((e) => e.event === 'prompt_started')).toHaveLength(0)

    const rec = await runHarnessOnce<{
      report: RecoveryReport
      runs: RunRow[]
      sessions: SessionRow[]
    }>(stateDir, ['recover'], env)
    expect(rec.json!.report.turns).toEqual([
      { id: expect.any(Number), sessionId: rec.json!.sessions[0]!.id, outcome: 'never_started' },
    ])
    expect(rec.json!.report.runs).toEqual([
      { id: rec.json!.runs[0]!.id, outcome: 'retryable' },
    ])
    expect(rec.json!.sessions[0]?.frozenAt).toBeNull()
    expect(rec.json!.runs[0]?.status).toBe('interrupted')

    const retried = await runHarnessOnce<RunRow>(
      stateDir,
      ['retry', rec.json!.runs[0]!.id],
      env,
    )
    expect(retried.json!.status).toBe('done')
    const count = execFileSync(
      'git',
      ['-C', remote, 'rev-list', '--count', `${retried.json!.baseSha}..${retried.json!.resultSha}`],
      { encoding: 'utf8' },
    ).trim()
    expect(Number(count)).toBe(1)
  })

  it('killed after authorization, unknown writer', async () => {
    const { root, stateDir, work, logPath, env } = makeFixture('ms-6-', {
      '1': [{ wait: 'hold-auth' }],
      '2': [
        { commit: { file: 'ok.txt', content: 'ok\n', trailer: true } },
        { push: true },
        { submitPushed: true },
        { reply: 'done' },
      ],
    })
    const bHold = barrier(root, 'hold-auth')
    const harness = spawnHarness(stateDir, ['start-run', work, 'req-auth', 'work'], env)
    await bHold.waitReached()
    const storeBefore = new Store(join(stateDir, 'polyclave.sqlite'))
    const turnBefore = storeBefore.runningTurns()[0]!
    const pgid = turnBefore.agentPgid!
    const startedAt = turnBefore.agentStartedAt!
    storeBefore.close()

    await killAndWait(harness)
    await waitFor(() => groupLiveness(pgid, startedAt) === 'gone')

    const srv = await ServeClient.start(stateDir, env)
    const state1 = await srv.call<{ sessions: SessionRow[]; runs: RunRow[] }>('get-state')
    const sessionId = state1.sessions[0]!.id
    const runId = state1.runs[0]!.id
    expect(state1.sessions[0]!.frozenAt).toBeTruthy()
    expect(state1.runs[0]!.status).toBe('needs_review')

    // retry & queue refused while frozen
    await expect(srv.call('retry', { runId })).rejects.toThrow()
    const queuedMsg = await srv.call<{ message: MessageRow }>('send', {
      sessionId,
      text: 'held while frozen',
    })
    expect(queuedMsg.message.queued).toBe(true)
    await srv.call('idle')
    expect(readFakeLog(logPath).filter((e) => e.event === 'prompt_started')).toHaveLength(1)

    // Allow retry + unfreeze (make sure chat script exists for queued message or resolve first then unfreeze)
    // Put a chat reply at turn 2 and task completion at turn 3
    writeFileSync(
      join(root, 'script.json'),
      JSON.stringify({
        '2': [{ reply: 'chat ok' }],
        '3': [
          { commit: { file: 'ok.txt', content: 'ok\n', trailer: true } },
          { push: true },
          { submitPushed: true },
          { reply: 'done' },
        ],
      }),
    )

    await srv.call('unfreeze', { sessionId, note: 'user checked' })
    await srv.call('resolve', { runId, decision: 'allow_retry', note: 'safe after check' })
    await srv.call('idle')

    await srv.call('retry', { runId })
    await srv.call('idle')

    const state2 = await srv.call<{ runs: RunRow[] }>('get-state')
    expect(state2.runs[0]!.status).toBe('done')
    await srv.close()
  })

  it('local commit exists, never pushed, writer excluded', async () => {
    const { stateDir, work, remote, logPath, env } = makeFixture('ms-7-', {
      '1': [{ push: true }, { submitPushed: true }, { reply: 'pushed X' }],
    })

    // Initialize store with old boot time 100.0
    const s1 = await ServeClient.start(stateDir, env)
    const { session } = await s1.call<{ session: SessionRow }>('create-session', {
      cwd: work,
      name: 's1',
    })
    const { run } = await s1.call<{ run: RunRow }>('create-run', {
      sessionId: session.id,
      requestId: 'req-local',
      instructions: 'reuse X',
    })
    await s1.close()

    execFileSync('git', ['-C', work, 'worktree', 'add', '-b', run.branch, run.worktreePath, run.baseSha])
    writeFileSync(join(run.worktreePath, 'x.txt'), 'X\n')
    execFileSync('git', ['-C', run.worktreePath, 'add', '.'])
    execFileSync('git', ['-C', run.worktreePath, 'commit', '-m', 'local X', '--trailer', `Run-Id: ${run.id}`])
    const shaX = execFileSync('git', ['-C', run.worktreePath, 'rev-parse', 'HEAD'], {
      encoding: 'utf8',
    }).trim()

    const store = new Store(join(stateDir, 'polyclave.sqlite'))
    const t = store.claimTurn({
      sessionId: session.id,
      mode: 'start',
      runId: run.id,
      hostInstanceId: 'dead-inst',
      bootTime: '100.0',
    })
    store.authorizeTurn(t.id, 999999, 'Mon Jan  1 00:00:00 2020')
    store.close()

    // Reboot: boot time 200.0 excludes writer
    const rebootEnv = { ...env, POLYCLAVE_TEST_BOOT_TIME: '200.0' }
    const rec = await runHarnessOnce<{ runs: RunRow[]; sessions: SessionRow[] }>(
      stateDir,
      ['recover'],
      rebootEnv,
    )
    expect(rec.json!.sessions[0]?.frozenAt).toBeNull()
    expect(rec.json!.runs[0]?.status).toBe('interrupted')

    const retried = await runHarnessOnce<RunRow>(stateDir, ['retry', run.id], rebootEnv)
    expect(retried.json!.status).toBe('done')
    expect(retried.json!.resultSha).toBe(shaX)
    expect(
      execFileSync('git', ['-C', remote, 'rev-parse', `refs/heads/${run.branch}`], {
        encoding: 'utf8',
      }).trim(),
    ).toBe(shaX)
    expect(readFakeLog(logPath).filter((e) => e.event === 'push_attempt')).toHaveLength(1)
  })

  it('accepted, then host killed before done was written', async () => {
    // Case A: reboot confirms done with resultSha = X
    {
      const { root, stateDir, work, refLog, env } = makeFixture('ms-8a-', {
        '1': [
          { commit: { file: 'a.txt', content: 'a\n', trailer: true } },
          { push: true },
          { submitPushed: true },
          { wait: 'after-submit' },
        ],
      })
      const bAfterSubmit = barrier(root, 'after-submit')
      const harness = spawnHarness(stateDir, ['start-run', work, 'req-acc', 'ship a'], env)
      await bAfterSubmit.waitReached()
      const storeBefore = new Store(join(stateDir, 'polyclave.sqlite'))
      const turnBefore = storeBefore.runningTurns()[0]!
      const pgid = turnBefore.agentPgid!
      const startedAt = turnBefore.agentStartedAt!
      storeBefore.close()

      await killAndWait(harness)
      await waitFor(() => groupLiveness(pgid, startedAt) === 'gone')

      const rec = await runHarnessOnce<{ runs: RunRow[]; sessions: SessionRow[] }>(
        stateDir,
        ['recover'],
        { ...env, POLYCLAVE_TEST_BOOT_TIME: '200.0' },
      )
      const finished = rec.json!.runs[0]!
      const refEntry = readRefLog(refLog).find((line) =>
        line.startsWith(`refs/heads/${finished.branch} `),
      )
      expect(refEntry).toBeDefined()
      const shaX = refEntry!.split(/\s+/)[2]!
      expect(shaX).toMatch(/^[0-9a-f]{40}$/)
      expect(finished.status).toBe('done')
      expect(finished.resultSha).toBe(shaX)

      const db = new DatabaseSync(join(stateDir, 'polyclave.sqlite'))
      const row = db
        .prepare('SELECT COUNT(*) AS n FROM results WHERE accepted = 1')
        .get() as { n: number }
      expect(row.n).toBe(1)
      db.close()
    }

    // Case B: remote moved to Y before recovery -> needs_review keeping Y
    {
      const { root, stateDir, work, remote, env } = makeFixture('ms-8b-', {
        '1': [
          { commit: { file: 'a.txt', content: 'a\n', trailer: true } },
          { push: true },
          { submitPushed: true },
          { wait: 'after-submit' },
        ],
      })
      const bAfterSubmit = barrier(root, 'after-submit')
      const harness = spawnHarness(stateDir, ['start-run', work, 'req-acc-b', 'ship b'], env)
      await bAfterSubmit.waitReached()
      const storeBefore = new Store(join(stateDir, 'polyclave.sqlite'))
      const turnBefore = storeBefore.runningTurns()[0]!
      const pgid = turnBefore.agentPgid!
      const startedAt = turnBefore.agentStartedAt!
      storeBefore.close()

      await killAndWait(harness)
      await waitFor(() => groupLiveness(pgid, startedAt) === 'gone')

      const store = new Store(join(stateDir, 'polyclave.sqlite'))
      const run = store.listRuns()[0]!
      store.close()

      // Push commit Y onto the same branch in remote
      writeFileSync(join(run.worktreePath, 'y.txt'), 'y\n')
      execFileSync('git', ['-C', run.worktreePath, 'add', '.'])
      execFileSync('git', ['-C', run.worktreePath, 'commit', '-m', 'move to Y', '--trailer', `Run-Id: ${run.id}`])
      const shaY = execFileSync('git', ['-C', run.worktreePath, 'rev-parse', 'HEAD'], {
        encoding: 'utf8',
      }).trim()
      execFileSync('git', ['-C', run.worktreePath, 'push', remote, `refs/heads/${run.branch}:refs/heads/${run.branch}`])

      const rec = await runHarnessOnce<{ runs: RunRow[] }>(
        stateDir,
        ['recover'],
        { ...env, POLYCLAVE_TEST_BOOT_TIME: '200.0' },
      )
      expect(rec.json!.runs[0]?.status).toBe('needs_review')
      expect(rec.json!.runs[0]?.observedSha).toBe(shaY)
    }
  })

  it('second host refused while the first lives; old token refused after restart', async () => {
    const { root, stateDir, work, env } = makeFixture('ms-9-', {
      '1': [{ wait: 'hold-tok' }],
    })
    const bHold = barrier(root, 'hold-tok')

    const s1 = await ServeClient.start(stateDir, env)
    const { session } = await s1.call<{ session: SessionRow }>('create-session', {
      cwd: work,
      name: 's1',
    })
    const { run } = await s1.call<{ run: RunRow }>('create-run', {
      sessionId: session.id,
      requestId: 'req-tok',
      instructions: 'tok test',
    })
    const startP = s1.call('start-run', { runId: run.id })
    startP.catch(() => {})
    await bHold.waitReached()
    await waitFor(() => {
      const db = new DatabaseSync(join(stateDir, 'polyclave.sqlite'))
      try {
        const row = db
          .prepare('SELECT agent_session_id FROM runs WHERE id = ?')
          .get(run.id) as { agent_session_id: string | null } | undefined
        return Boolean(row?.agent_session_id)
      } finally {
        db.close()
      }
    })

    const dbBefore = new DatabaseSync(join(stateDir, 'polyclave.sqlite'))
    const snapRuns = dbBefore.prepare('SELECT * FROM runs ORDER BY id').all()
    const snapTurns = dbBefore.prepare('SELECT * FROM turns ORDER BY id').all()
    const snapSessions = dbBefore.prepare('SELECT * FROM sessions ORDER BY id').all()
    dbBefore.close()

    // Second host refused while s1 lives
    const second = await runHarnessOnce(stateDir, ['recover'], env)
    expect(second.code).not.toBe(0)

    const dbAfter = new DatabaseSync(join(stateDir, 'polyclave.sqlite'))
    expect(dbAfter.prepare('SELECT * FROM runs ORDER BY id').all()).toEqual(snapRuns)
    expect(dbAfter.prepare('SELECT * FROM turns ORDER BY id').all()).toEqual(snapTurns)
    expect(dbAfter.prepare('SELECT * FROM sessions ORDER BY id').all()).toEqual(snapSessions)
    dbAfter.close()

    const store = new Store(join(stateDir, 'polyclave.sqlite'))
    const runningTurn = store.runningTurns()[0]!
    const oldToken = runningTurn.mcpToken
    const pgid = runningTurn.agentPgid!
    const startedAt = runningTurn.agentStartedAt!
    store.close()

    await s1.killAndWait()
    await waitFor(() => groupLiveness(pgid, startedAt) === 'gone')

    const s2 = await ServeClient.start(stateDir, env)
    const res = await fetch(s2.ready.mcpUrl, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        Authorization: `Bearer ${oldToken}`,
      },
      body: JSON.stringify({
        jsonrpc: '2.0',
        id: 1,
        method: 'tools/call',
        params: {
          name: 'submit_result',
          arguments: { run_id: run.id, branch: run.branch, sha: 'a'.repeat(40) },
        },
      }),
    })
    expect(res.status).toBe(401)

    const storeAfter = new Store(join(stateDir, 'polyclave.sqlite'))
    expect(storeAfter.acceptedResult(run.id)).toBeUndefined()
    storeAfter.close()

    await s2.close()
  })
})
