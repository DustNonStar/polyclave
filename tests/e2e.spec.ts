import { spawn, type ChildProcess } from 'node:child_process'
import { once } from 'node:events'
import { mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { afterEach, describe, expect, it } from 'vitest'
import { barrier, type Barrier, FAKE_CODEX, makeRepo, tempDir, waitFor } from './helpers.ts'

const MAIN_PATH = fileURLToPath(new URL('../src/main.ts', import.meta.url))

interface HostProc {
  child: ChildProcess
  url: string
  stdout: string
  stderr: string
  exited: Promise<{ code: number | null; signal: NodeJS.Signals | null }>
}

const temps: string[] = []
const hosts: HostProc[] = []
const barriers: Barrier[] = []

function trackTemp(dir: string): string {
  temps.push(dir)
  return dir
}

function trackBarrier(b: Barrier): Barrier {
  barriers.push(b)
  return b
}

afterEach(async () => {
  while (barriers.length > 0) {
    try {
      barriers.pop()!.release()
    } catch {}
  }
  while (hosts.length > 0) {
    const h = hosts.pop()!
    if (h.child.exitCode === null && h.child.signalCode === null) {
      h.child.kill('SIGTERM')
      const res = await Promise.race([
        h.exited.then(() => 'exited' as const),
        new Promise<'timeout'>((resolve) => setTimeout(() => resolve('timeout'), 4000)),
      ])
      if (res === 'timeout') {
        h.child.kill('SIGKILL')
        await h.exited
      }
    }
  }
  while (temps.length > 0) {
    rmSync(temps.pop()!, { recursive: true, force: true })
  }
})

async function startHost(opts: {
  stateDir: string
  userCodex: string
  scriptPath: string
  logPath: string
  barrierDir: string
}): Promise<HostProc> {
  const child = spawn(
    process.execPath,
    [MAIN_PATH, '--state-dir', opts.stateDir, '--port', '0', '--codex', ...FAKE_CODEX],
    {
      env: {
        ...process.env,
        POLYCLAVE_TEST_USER_CODEX_HOME: opts.userCodex,
        FAKE_CODEX_SCRIPT: opts.scriptPath,
        FAKE_CODEX_LOG: opts.logPath,
        FAKE_CODEX_BARRIER_DIR: opts.barrierDir,
      },
      stdio: ['ignore', 'pipe', 'pipe'],
    },
  )

  const exited = once(child, 'exit').then(([code, signal]) => ({
    code: code as number | null,
    signal: signal as NodeJS.Signals | null,
  }))

  const host: HostProc = {
    child,
    url: '',
    stdout: '',
    stderr: '',
    exited,
  }
  hosts.push(host)

  child.stdout?.setEncoding('utf8')
  child.stderr?.setEncoding('utf8')
  child.stdout?.on('data', (chunk: string) => {
    host.stdout += chunk
  })
  child.stderr?.on('data', (chunk: string) => {
    host.stderr += chunk
  })

  await waitFor(() => {
    const m = /polyclave listening on (http:\/\/127\.0\.0\.1:\d+)/.exec(host.stdout)
    if (m) {
      host.url = m[1]!
      return true
    }
    if (child.exitCode !== null || child.signalCode !== null) {
      throw new Error(`host exited early (${child.exitCode}/${child.signalCode}): ${host.stderr}`)
    }
    return false
  }, 15000)

  return host
}

function setupFixture(dir: string, script: Record<string, unknown>) {
  const repoDir = join(dir, 'repo')
  mkdirSync(repoDir, { recursive: true })
  const { work, remote } = makeRepo(repoDir)

  const stateDir = join(dir, 'state')
  mkdirSync(stateDir, { recursive: true })

  const userCodex = join(dir, 'user-codex')
  mkdirSync(userCodex, { recursive: true })
  writeFileSync(join(userCodex, 'auth.json'), '{}')

  const scriptPath = join(dir, 'script.json')
  const logPath = join(dir, 'fake.log')
  writeFileSync(scriptPath, JSON.stringify(script))

  return { work, remote, stateDir, userCodex, scriptPath, logPath }
}

function readFakeLog(logPath: string): Array<Record<string, unknown>> {
  return readFileSync(logPath, 'utf8')
    .split('\n')
    .filter((line) => line.trim().length > 0)
    .map((line) => JSON.parse(line) as Record<string, unknown>)
}

async function jsonReq<T>(url: string, init?: RequestInit): Promise<{ status: number; body: T }> {
  const res = await fetch(url, init)
  const body = (await res.json()) as T
  return { status: res.status, body }
}

describe('e2e via main.ts', () => {
  it('host killed during a run: restart shows the frozen session; unfreeze and resolve work over HTTP', async () => {
    const dir = trackTemp(tempDir('polyclave-e2e-kill-run-'))
    const runBarrier = trackBarrier(barrier(dir, 'run-barrier'))
    const fix = setupFixture(dir, {
      '1': [{ wait: 'run-barrier' }, { reply: 'never finished' }],
    })

    const host1 = await startHost({ ...fix, barrierDir: dir })

    const createdSess = await jsonReq<{ id: string }>(`${host1.url}/api/sessions`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ name: 'app', cwd: fix.work }),
    })
    expect(createdSess.status).toBe(201)
    const sessionId = createdSess.body.id

    const createdRun = await jsonReq<{ id: string }>(`${host1.url}/api/runs`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({
        sessionId,
        requestId: 'req-e2e-crash',
        instructions: 'Add change',
      }),
    })
    expect(createdRun.status).toBe(201)
    const runId = createdRun.body.id

    await runBarrier.waitReached()
    await waitFor(async () => {
      const turns = await jsonReq<Array<{ phase: string; agentPgid: number | null }>>(
        `${host1.url}/api/sessions/${sessionId}/turns`,
      )
      return turns.body.length === 1 && turns.body[0]!.agentPgid !== null
    })

    host1.child.kill('SIGKILL')
    const exitInfo = await host1.exited
    expect(exitInfo.signal).toBe('SIGKILL')

    const host2 = await startHost({ ...fix, barrierDir: dir })

    const sessAfterRestart = await jsonReq<Array<{ id: string; frozenReason: string | null }>>(
      `${host2.url}/api/sessions`,
    )
    expect(sessAfterRestart.status).toBe(200)
    const s = sessAfterRestart.body.find((row) => row.id === sessionId)
    expect(s?.frozenReason).toBeTruthy()
    expect(s?.frozenReason).toContain('host stopped while an agent was running')

    const runsAfterRestart = await jsonReq<Array<{ id: string; status: string; detail: string | null }>>(
      `${host2.url}/api/runs`,
    )
    expect(runsAfterRestart.status).toBe(200)
    const recoveredRun = runsAfterRestart.body.find((r) => r.id === runId)
    expect(recoveredRun?.status).toBe('needs_review')
    expect(recoveredRun?.detail).toContain('an agent process may still be writing')

    const lockedRunRes = await jsonReq<{ error: string }>(`${host2.url}/api/runs`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({
        sessionId,
        requestId: 'req-blocked-while-frozen',
        instructions: 'blocked while frozen',
      }),
    })
    expect(lockedRunRes.status).toBe(409)

    const unfreezeRes = await jsonReq<{ id: string; frozenReason: string | null }>(
      `${host2.url}/api/sessions/${sessionId}/unfreeze`,
      {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ note: 'checked no orphan writers remain' }),
      },
    )
    expect(unfreezeRes.status).toBe(200)
    expect(unfreezeRes.body.frozenReason).toBeNull()

    const msgsAfterUnfreeze = await jsonReq<Array<{ source: string; text: string }>>(
      `${host2.url}/api/sessions/${sessionId}/messages`,
    )
    expect(
      msgsAfterUnfreeze.body.some(
        (m) => m.source === 'host' && m.text.includes('checked no orphan writers remain'),
      ),
    ).toBe(true)

    const resolveRes = await jsonReq<{ id: string; status: string; resolution: string | null }>(
      `${host2.url}/api/runs/${runId}/resolve`,
      {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({
          decision: 'fail',
          note: 'marked failed after host crash recovery',
        }),
      },
    )
    expect(resolveRes.status).toBe(200)
    expect(resolveRes.body.status).toBe('failed')
    expect(resolveRes.body.resolution).toContain('marked failed after host crash recovery')
  })

  it('chat finished, then restart: same thread, no interruption note', async () => {
    const dir = trackTemp(tempDir('polyclave-e2e-clean-restart-'))
    const fix = setupFixture(dir, {
      '1': [{ reply: 'reply one' }],
      '2': [{ reply: 'reply two' }],
    })

    const host1 = await startHost({ ...fix, barrierDir: dir })
    const created = await jsonReq<{ id: string }>(`${host1.url}/api/sessions`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ name: 'chat', cwd: fix.work }),
    })
    const sessionId = created.body.id

    const post1 = await jsonReq<{ message: { id: number } }>(
      `${host1.url}/api/sessions/${sessionId}/messages`,
      {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ text: 'hello turn 1' }),
      },
    )
    expect(post1.status).toBe(202)

    let thread1: string | null = null
    await waitFor(async () => {
      const list = await jsonReq<
        Array<{
          id: string
          busy: boolean
          lastTurnStatus: string | null
          agentSessionId: string | null
        }>
      >(`${host1.url}/api/sessions`)
      const row = list.body.find((s) => s.id === sessionId)
      if (row && !row.busy && row.lastTurnStatus === 'completed' && row.agentSessionId) {
        thread1 = row.agentSessionId
        return true
      }
      return false
    })

    host1.child.kill('SIGTERM')
    const stopRes = await host1.exited
    expect(stopRes.code).toBe(0)

    const host2 = await startHost({ ...fix, barrierDir: dir })
    const listAfter = await jsonReq<
      Array<{
        id: string
        frozenReason: string | null
        agentSessionId: string | null
      }>
    >(`${host2.url}/api/sessions`)
    const rowAfter = listAfter.body.find((s) => s.id === sessionId)
    expect(rowAfter?.frozenReason).toBeNull()
    expect(rowAfter?.agentSessionId).toBe(thread1)

    const post2 = await jsonReq<{ message: { id: number } }>(
      `${host2.url}/api/sessions/${sessionId}/messages`,
      {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ text: 'hello turn 2' }),
      },
    )
    expect(post2.status).toBe(202)

    await waitFor(async () => {
      const msgs = await jsonReq<Array<{ source: string; text: string }>>(
        `${host2.url}/api/sessions/${sessionId}/messages`,
      )
      return msgs.body.some((m) => m.source === 'agent' && m.text === 'reply two')
    })

    const starts = readFakeLog(fix.logPath).filter((e) => e.event === 'prompt_started')
    expect(starts).toHaveLength(2)
    expect(starts[1]!.resumed).toBe(true)
    expect(starts[1]!.threadId).toBe(thread1)
    expect(String(starts[1]!.prompt)).not.toContain('interrupted')
  })

  it('chat killed after thread.started and before the turn ended: frozen; after unfreeze the next message resumes the same thread with the note once', async () => {
    const dir = trackTemp(tempDir('polyclave-e2e-kill-chat-'))
    const chatBarrier = trackBarrier(barrier(dir, 'chat-barrier'))
    const fix = setupFixture(dir, {
      '1': [{ wait: 'chat-barrier' }, { reply: 'never finished' }],
      '2': [{ reply: 'reply after unfreeze' }],
      '3': [{ reply: 'reply three' }],
    })

    const host1 = await startHost({ ...fix, barrierDir: dir })
    const created = await jsonReq<{ id: string }>(`${host1.url}/api/sessions`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ name: 'chat-kill', cwd: fix.work }),
    })
    const sessionId = created.body.id

    const post1 = await jsonReq(`${host1.url}/api/sessions/${sessionId}/messages`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ text: 'first interrupted prompt' }),
    })
    expect(post1.status).toBe(202)

    await chatBarrier.waitReached()
    let thread1: string | null = null
    await waitFor(async () => {
      const list = await jsonReq<Array<{ id: string; agentSessionId: string | null }>>(
        `${host1.url}/api/sessions`,
      )
      const row = list.body.find((s) => s.id === sessionId)
      if (row?.agentSessionId) {
        thread1 = row.agentSessionId
        return true
      }
      return false
    })

    host1.child.kill('SIGKILL')
    await host1.exited

    const host2 = await startHost({ ...fix, barrierDir: dir })
    const listAfter = await jsonReq<
      Array<{ id: string; frozenReason: string | null; agentSessionId: string | null }>
    >(`${host2.url}/api/sessions`)
    const rowAfter = listAfter.body.find((s) => s.id === sessionId)
    expect(rowAfter?.frozenReason).toBeTruthy()
    expect(rowAfter?.agentSessionId).toBe(thread1)

    const unfreeze = await jsonReq(`${host2.url}/api/sessions/${sessionId}/unfreeze`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ note: 'clean slate verified' }),
    })
    expect(unfreeze.status).toBe(200)

    const post2 = await jsonReq(`${host2.url}/api/sessions/${sessionId}/messages`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ text: 'resume after crash' }),
    })
    expect(post2.status).toBe(202)

    await waitFor(async () => {
      const msgs = await jsonReq<Array<{ source: string; text: string }>>(
        `${host2.url}/api/sessions/${sessionId}/messages`,
      )
      return msgs.body.some((m) => m.source === 'agent' && m.text === 'reply after unfreeze')
    })

    const post3 = await jsonReq(`${host2.url}/api/sessions/${sessionId}/messages`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ text: 'third turn normal' }),
    })
    expect(post3.status).toBe(202)

    await waitFor(async () => {
      const msgs = await jsonReq<Array<{ source: string; text: string }>>(
        `${host2.url}/api/sessions/${sessionId}/messages`,
      )
      return msgs.body.some((m) => m.source === 'agent' && m.text === 'reply three')
    })

    const starts = readFakeLog(fix.logPath).filter((e) => e.event === 'prompt_started')
    expect(starts).toHaveLength(3)
    expect(starts[1]!.resumed).toBe(true)
    expect(starts[1]!.threadId).toBe(thread1)
    expect(String(starts[1]!.prompt)).toContain('interrupted')
    expect(starts[2]!.resumed).toBe(true)
    expect(starts[2]!.threadId).toBe(thread1)
    expect(String(starts[2]!.prompt)).not.toContain('interrupted')
  })

  it('a second host on the same state dir refuses to start', async () => {
    const dir = trackTemp(tempDir('polyclave-e2e-second-host-'))
    const fix = setupFixture(dir, {})

    const host1 = await startHost({ ...fix, barrierDir: dir })

    const second = spawn(
      process.execPath,
      [MAIN_PATH, '--state-dir', fix.stateDir, '--port', '0', '--codex', ...FAKE_CODEX],
      {
        env: {
          ...process.env,
          POLYCLAVE_TEST_USER_CODEX_HOME: fix.userCodex,
        },
        stdio: ['ignore', 'pipe', 'pipe'],
      },
    )

    let secondStderr = ''
    second.stderr?.setEncoding('utf8')
    second.stderr?.on('data', (chunk: string) => {
      secondStderr += chunk
    })

    const [code] = (await once(second, 'exit')) as [number | null, NodeJS.Signals | null]
    expect(code).toBe(1)
    expect(secondStderr).toMatch(/already running|host/i)

    const stillUp = await jsonReq<unknown[]>(`${host1.url}/api/sessions`)
    expect(stillUp.status).toBe(200)
  })
})
