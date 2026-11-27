import { afterEach, describe, expect, it } from 'vitest'
import { EventEmitter } from 'node:events'
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { prepareCodexHome } from '../src/codex.ts'
import { checkRunBranch, prepareWorktree, resolveBaseSha } from '../src/git-effect.ts'
import { hostInstanceId } from '../src/ids.ts'
import { startHostMcp, type HostMcp } from '../src/mcp.ts'
import { DispatchClosedError, TurnRunner, type HostEvent } from '../src/runner.ts'
import { Store } from '../src/store.ts'
import { barrier, FAKE_CODEX, makeRepo, tempDir } from './helpers.ts'

const temps: string[] = []
const runners: TurnRunner[] = []
const mcps: HostMcp[] = []

function trackTemp(d: string): string {
  temps.push(d)
  return d
}

afterEach(async () => {
  while (runners.length > 0) {
    await runners.pop()!.shutdown()
  }
  while (mcps.length > 0) {
    await mcps.pop()!.close()
  }
  while (temps.length > 0) {
    rmSync(temps.pop()!, { recursive: true, force: true })
  }
})

type FakeLogEntry = {
  event: string
  turn: number
  resumed?: boolean
  threadId?: string
  prompt?: string
  cwd?: string
}

function readFakeLog(path: string): FakeLogEntry[] {
  if (!existsSync(path)) return []
  return readFileSync(path, 'utf8')
    .trim()
    .split('\n')
    .filter(Boolean)
    .map((l) => JSON.parse(l) as FakeLogEntry)
}

async function setupRunner(dir: string, script: Record<string, unknown>) {
  const { work, remote } = makeRepo(dir)
  const store = new Store(join(dir, 'polyclave.sqlite'))
  const inst = hostInstanceId()
  const mcp = await startHostMcp({ store, hostInstanceId: inst, check: checkRunBranch })
  mcps.push(mcp)

  const userCodex = join(dir, 'user-codex')
  mkdirSync(userCodex, { recursive: true })
  writeFileSync(join(userCodex, 'auth.json'), '{}')
  const codexHome = prepareCodexHome(dir, userCodex)

  const scriptPath = join(dir, 'script.json')
  const logPath = join(dir, 'fake.log')
  writeFileSync(scriptPath, JSON.stringify(script))

  const bus = new EventEmitter()
  const events: HostEvent[] = []
  bus.on('event', (e: HostEvent) => events.push(e))

  const runner = new TurnRunner({
    store,
    mcpUrl: mcp.url,
    codexCommand: FAKE_CODEX,
    codexHome,
    hostInstanceId: inst,
    extraEnv: {
      FAKE_CODEX_SCRIPT: scriptPath,
      FAKE_CODEX_LOG: logPath,
      FAKE_CODEX_BARRIER_DIR: dir,
    },
    bus,
  })
  runners.push(runner)
  const session = store.createSession({ name: 's1', cwd: work })
  return { work, remote, store, inst, mcp, runner, session, logPath, scriptPath, events, bus }
}

describe('TurnRunner', () => {
  it('starts a session thread, records events and the reply', async () => {
    const dir = trackTemp(tempDir('polyclave-run-1-'))
    const { store, runner, session, events } = await setupRunner(dir, {
      '1': [{ reply: 'hello back' }],
    })
    const m = runner.sendUser(session.id, 'hi there')
    expect(m.source).toBe('user')
    await runner.idle()

    const updated = store.getSession(session.id)!
    expect(updated.agentSessionId).toMatch(/^fake-thread-/)
    const msgs = store.listMessages(session.id)
    expect(msgs.map((x) => [x.source, x.text])).toEqual([
      ['user', 'hi there'],
      ['agent', 'hello back'],
    ])
    expect(events.some((e) => e.type === 'agent' && e.event.kind === 'message.done')).toBe(true)
  })

  it('resumes the session thread on the next message', async () => {
    const dir = trackTemp(tempDir('polyclave-run-2-'))
    const { store, runner, session, logPath } = await setupRunner(dir, {
      '1': [{ reply: 'first' }],
      '2': [{ reply: 'second' }],
    })
    runner.sendUser(session.id, 'm1')
    await runner.idle()
    const tid = store.getSession(session.id)!.agentSessionId!

    runner.sendUser(session.id, 'm2')
    await runner.idle()

    const entries = readFakeLog(logPath).filter((e) => e.event === 'prompt_started')
    expect(entries).toHaveLength(2)
    expect(entries[0]?.resumed).toBe(false)
    expect(entries[1]?.resumed).toBe(true)
    expect(entries[1]?.threadId).toBe(tid)
  })

  it('queues a message during a turn and delivers it after', async () => {
    const dir = trackTemp(tempDir('polyclave-run-3-'))
    const b1 = barrier(dir, 'b1')
    const { store, runner, session, logPath } = await setupRunner(dir, {
      '1': [{ wait: 'b1' }, { reply: 'one' }],
      '2': [{ reply: 'two' }],
    })

    runner.sendUser(session.id, 'first')
    await b1.waitReached()

    const m2 = runner.sendUser(session.id, 'second')
    expect(m2.queued).toBe(true)
    expect(store.nextQueued(session.id)?.id).toBe(m2.id)

    b1.release()
    await runner.idle()

    const entries = readFakeLog(logPath)
    const idxEnd1 = entries.findIndex((e) => e.event === 'turn_end' && e.turn === 1)
    const idxStart2 = entries.findIndex((e) => e.event === 'prompt_started' && e.turn === 2)
    expect(idxEnd1).toBeGreaterThan(-1)
    expect(idxStart2).toBeGreaterThan(idxEnd1)

    const msgs = store.listMessages(session.id)
    expect(msgs.map((m) => m.text)).toEqual(['first', 'second', 'one', 'two'])
    expect(msgs.every((m) => !m.queued)).toBe(true)
  })

  it('never starts a turn on a frozen session or with dispatch closed', async () => {
    const dir = trackTemp(tempDir('polyclave-run-4-'))
    const { store, runner, session, logPath } = await setupRunner(dir, {
      '1': [{ reply: 'should not run' }],
    })

    store.freezeSession(session.id, 'frozen test')
    const m1 = runner.sendUser(session.id, 'while frozen')
    await runner.idle()
    expect(store.nextQueued(session.id)?.id).toBe(m1.id)

    store.unfreezeSession(session.id)
    runner.setDispatch(false)
    const m2 = runner.sendUser(session.id, 'while closed')
    await runner.idle()
    expect(store.listMessages(session.id).filter((m) => m.queued).map((m) => m.id)).toEqual([
      m1.id,
      m2.id,
    ])
    await expect(
      runner.runTurn({ sessionId: session.id, text: 'direct', mode: 'chat' }),
    ).rejects.toThrow(DispatchClosedError)

    expect(() => readFileSync(logPath, 'utf8')).toThrow()
  })

  it('cancels a running turn: cancelled, writer possible, session frozen, queue held', async () => {
    const dir = trackTemp(tempDir('polyclave-run-5-'))
    const b1 = barrier(dir, 'b1')
    const { store, runner, session, logPath } = await setupRunner(dir, {
      '1': [{ wait: 'b1' }, { reply: 'late reply' }],
      '2': [{ reply: 'queued should stay' }],
    })

    runner.sendUser(session.id, 'start')
    await b1.waitReached()
    const running = store.runningTurn(session.id)!
    const queued = runner.sendUser(session.id, 'waiting')

    expect(await runner.cancel(session.id)).toBe(true)
    await runner.idle()

    const turnAfter = store.getTurn(running.id)!
    expect(turnAfter.status).toBe('cancelled')
    expect(turnAfter.writerCheck).toBe('possible')
    expect(turnAfter.endReason).toBe('cancel')
    expect(store.getSession(session.id)?.frozenAt).toBeTruthy()
    expect(store.nextQueued(session.id)?.id).toBe(queued.id)

    const entries = readFakeLog(logPath).filter((e) => e.event === 'prompt_started')
    expect(entries).toHaveLength(1)
  })

  it('cancels a prepared turn as never_started without freezing the session', async () => {
    const dir = trackTemp(tempDir('polyclave-run-cancel-prep-'))
    const { store, runner, session, logPath } = await setupRunner(dir, {
      '1': [{ reply: 'should not run' }],
    })
    const bAuth = barrier(dir, 'before-auth')
    const prev = process.env.POLYCLAVE_TEST_BARRIER_BEFORE_AUTHORIZE
    process.env.POLYCLAVE_TEST_BARRIER_BEFORE_AUTHORIZE = bAuth.dir
    try {
      const turnP = runner.runTurn({ sessionId: session.id, text: 'hi', mode: 'chat' })
      await bAuth.waitReached()
      const running = store.runningTurn(session.id)!
      expect(running.phase).toBe('prepared')

      expect(await runner.cancel(session.id)).toBe(true)
      const outcome = await turnP
      expect(outcome.selfExited).toBe(false)
      expect(outcome.turn.status).toBe('cancelled')
      expect(outcome.turn.endReason).toBe('never_started')
      expect(outcome.turn.writerCheck).toBe('excluded')
      expect(store.getSession(session.id)?.frozenAt).toBeNull()
      expect(readFakeLog(logPath)).toEqual([])
    } finally {
      if (prev === undefined) delete process.env.POLYCLAVE_TEST_BARRIER_BEFORE_AUTHORIZE
      else process.env.POLYCLAVE_TEST_BARRIER_BEFORE_AUTHORIZE = prev
    }
  })

  it('a failure before authorization ends the turn as never started and frees the session', async () => {
    const dir = trackTemp(tempDir('polyclave-run-6-'))
    const { store, runner, session, remote } = await setupRunner(dir, {
      '1': [{ reply: 'ok' }],
    })
    const baseSha = await resolveBaseSha(remote, 'main')
    const { run } = store.acceptRun({
      sessionId: session.id,
      requestId: 'q-missing-wt',
      instructions: 'do work',
      remote,
      base: 'main',
      baseSha,
      worktreePath: (id) => join(dir, 'missing-wt', id),
    })

    let caught: unknown
    try {
      await runner.runTurn({ sessionId: session.id, text: 'go', mode: 'start', runId: run.id })
    } catch (err) {
      caught = err
    }
    expect(caught).toBeInstanceOf(Error)
    expect((caught as { code?: string }).code).toBe('ENOENT')

    const t = store.lastTurn(session.id)!
    expect(t.status).toBe('interrupted')
    expect(t.endReason).toBe('never_started')
    expect(t.writerCheck).toBe('excluded')
    expect(store.runningTurn(session.id)).toBeUndefined()

    // A following turn on the freed session actually runs
    store.updateRun(run.id, { status: 'failed' }, ['running'])
    const nextOutcome = await runner.runTurn({ sessionId: session.id, text: 'follow up', mode: 'chat' })
    expect(nextOutcome.turn.status).toBe('completed')
    expect(nextOutcome.reply).toBe('ok')

    // Also verify queued message is re-queued if chat fails before authorization
    rmSync(session.cwd, { recursive: true, force: true })
    const m = runner.sendUser(session.id, 'recover me')
    await runner.idle()
    const back = store.nextQueued(session.id)
    expect(back?.id).toBe(m.id)
    expect(back?.queued).toBe(true)
    expect(back?.turnId).toBeNull()
  })

  it('fails the turn when codex exits immediately', async () => {
    const dir = trackTemp(tempDir('polyclave-run-7-'))
    const { store, runner, session } = await setupRunner(dir, {
      '1': [{ exit: 2, stderr: 'fatal error xyz' }],
    })

    const res = await runner.runTurn({
      sessionId: session.id,
      text: 'fail please',
      mode: 'chat',
    })
    expect(res.selfExited).toBe(true)
    expect(res.turn.status).toBe('failed')
    expect(res.turn.writerCheck).toBe('excluded')
    expect(res.turn.exitCode).toBe(2)

    const hostMsg = store.listMessages(session.id).find((m) => m.source === 'host')
    expect(hostMsg?.text).toContain('Codex failed:')
    expect(hostMsg?.text).toContain('fatal error xyz')
  })

  it('adds the chat interruption note exactly once, even after an unfreeze note', async () => {
    const dir = trackTemp(tempDir('polyclave-run-8-'))
    const b1 = barrier(dir, 'b1')
    const { store, runner, session, logPath } = await setupRunner(dir, {
      '1': [{ wait: 'b1' }],
      '2': [{ reply: 'resumed ok' }],
      '3': [{ reply: 'normal next' }],
    })

    runner.sendUser(session.id, 'first')
    await b1.waitReached()
    await runner.cancel(session.id)
    await runner.idle()

    store.unfreezeSession(session.id)
    store.addMessage({
      sessionId: session.id,
      turnId: null,
      source: 'host',
      text: 'Unfrozen by user: checked',
    })

    runner.sendUser(session.id, 'second')
    await runner.idle()

    runner.sendUser(session.id, 'third')
    await runner.idle()

    const prompts = readFakeLog(logPath).filter((e) => e.event === 'prompt_started')
    expect(prompts).toHaveLength(3)
    expect(prompts[1]?.prompt).toMatch(
      /^Note from the host: your previous turn was interrupted\. Check the current state before continuing\.\n\nsecond$/,
    )
    expect(prompts[2]?.prompt).toBe('third')

    const notes = store
      .listMessages(session.id)
      .filter((m) => m.source === 'host' && m.text.startsWith('Note from the host:'))
    expect(notes).toHaveLength(1)
  })

  it('uses the run worktree and run thread for run turns and the session cwd and thread for chat', async () => {
    const dir = trackTemp(tempDir('polyclave-run-9-'))
    const { store, runner, session, remote, work, logPath } = await setupRunner(dir, {
      '1': [{ reply: 'chat1' }],
      '2': [{ reply: 'run1' }],
      '3': [{ reply: 'chat2' }],
    })

    runner.sendUser(session.id, 'chat one')
    await runner.idle()

    const baseSha = await resolveBaseSha(remote, 'main')
    const { run } = store.acceptRun({
      sessionId: session.id,
      requestId: 'q-wt',
      instructions: 'work',
      remote,
      base: 'main',
      baseSha,
      worktreePath: (id) => join(dir, 'worktrees', id),
    })
    await prepareWorktree({
      cwd: work,
      remote,
      baseSha,
      branch: run.branch,
      path: run.worktreePath,
    })

    await runner.runTurn({ sessionId: session.id, text: 'run task', mode: 'start', runId: run.id })
    store.updateRun(run.id, { status: 'done' }, ['running'])

    runner.sendUser(session.id, 'chat two')
    await runner.idle()

    const entries = readFakeLog(logPath).filter((e) => e.event === 'prompt_started')
    expect(entries).toHaveLength(3)
    expect(entries[0]?.cwd).toBe(work)
    expect(entries[1]?.cwd).toBe(run.worktreePath)
    expect(entries[1]?.resumed).toBe(false)
    expect(entries[2]?.cwd).toBe(work)
    expect(entries[2]?.resumed).toBe(true)
    expect(entries[2]?.threadId).toBe(entries[0]?.threadId)
    expect(store.getRun(run.id)?.agentSessionId).toBe(entries[1]?.threadId)
  })

  it('does not pass GitHub or Polyclave secrets to codex', async () => {
    const dir = trackTemp(tempDir('polyclave-run-10-'))
    const envDump = join(dir, 'env.json')
    const { store, runner, session } = await setupRunner(dir, {})

    const prevGh = process.env.GH_TOKEN
    const prevGithub = process.env.GITHUB_TOKEN
    const prevPoly = process.env.POLYCLAVE_SECRET
    process.env.GH_TOKEN = 'ghp_secret'
    process.env.GITHUB_TOKEN = 'ghs_secret'
    process.env.POLYCLAVE_SECRET = 'poly_secret'

    try {
      const dumper = new TurnRunner({
        store,
        mcpUrl: 'http://127.0.0.1:9999/mcp',
        codexCommand: [
          process.execPath,
          '-e',
          `require('node:fs').writeFileSync(${JSON.stringify(envDump)}, JSON.stringify(process.env));`,
        ],
        codexHome: join(dir, 'codex-home'),
        hostInstanceId: 'inst-env',
        extraEnv: { POLYCLAVE_EXTRA: 'nope', GH_TOKEN: 'nope' },
        bus: new EventEmitter(),
      })
      runners.push(dumper)

      await dumper.runTurn({ sessionId: session.id, text: 'env', mode: 'chat' })
      const dumped = JSON.parse(readFileSync(envDump, 'utf8')) as Record<string, string>
      expect(dumped.GH_TOKEN).toBeUndefined()
      expect(dumped.GITHUB_TOKEN).toBeUndefined()
      expect(dumped.POLYCLAVE_SECRET).toBeUndefined()
      expect(dumped.POLYCLAVE_EXTRA).toBeUndefined()
      expect(dumped.POLYCLAVE_MCP_TOKEN).toMatch(/^[0-9a-f]{48}$/)
      expect(dumped.CODEX_HOME).toBe(join(dir, 'codex-home'))
    } finally {
      if (prevGh === undefined) delete process.env.GH_TOKEN
      else process.env.GH_TOKEN = prevGh
      if (prevGithub === undefined) delete process.env.GITHUB_TOKEN
      else process.env.GITHUB_TOKEN = prevGithub
      if (prevPoly === undefined) delete process.env.POLYCLAVE_SECRET
      else process.env.POLYCLAVE_SECRET = prevPoly
    }
    void runner
  })

  it('shutdown waits for running turns and starts nothing new', async () => {
    const dir = trackTemp(tempDir('polyclave-run-11-'))
    const b1 = barrier(dir, 'b1')
    const { store, runner, session, logPath } = await setupRunner(dir, {
      '1': [{ wait: 'b1' }, { reply: 'finished' }],
      '2': [{ reply: 'should not start' }],
    })

    runner.sendUser(session.id, 'first')
    await b1.waitReached()
    const queued = runner.sendUser(session.id, 'second')

    let shutDone = false
    const shutPromise = runner.shutdown().then(() => {
      shutDone = true
    })
    expect(runner.isDispatchOpen()).toBe(false)
    await new Promise((r) => setTimeout(r, 100))
    expect(shutDone).toBe(false)

    b1.release()
    await shutPromise
    expect(shutDone).toBe(true)

    const entries = readFakeLog(logPath).filter((e) => e.event === 'prompt_started')
    expect(entries).toHaveLength(1)
    expect(store.nextQueued(session.id)?.id).toBe(queued.id)
  })
})
