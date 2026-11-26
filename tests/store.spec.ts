import { afterEach, describe, expect, it } from 'vitest'
import { mkdtempSync, realpathSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { DatabaseSync } from 'node:sqlite'
import { pathToFileURL } from 'node:url'
import { Worker } from 'node:worker_threads'
import {
  BusyError,
  ConflictError,
  FrozenError,
  StaleStateError,
  Store,
} from '../src/store.ts'

const temps: string[] = []
function makeDbPath(): { dir: string; dbPath: string } {
  const dir = realpathSync(mkdtempSync(join(tmpdir(), 'polyclave-store-')))
  temps.push(dir)
  return { dir, dbPath: join(dir, 'polyclave.sqlite') }
}

afterEach(() => {
  while (temps.length > 0) {
    const d = temps.pop()!
    rmSync(d, { recursive: true, force: true })
  }
})

describe('Store', () => {
  it('refuses a second running turn on the same session', () => {
    const { dir, dbPath } = makeDbPath()
    const store = new Store(dbPath)
    const session = store.createSession({ name: 's1', cwd: dir })
    store.claimTurn({
      sessionId: session.id,
      mode: 'chat',
      hostInstanceId: 'inst1',
      bootTime: '100.0',
    })
    expect(() =>
      store.claimTurn({
        sessionId: session.id,
        mode: 'chat',
        hostInstanceId: 'inst1',
        bootTime: '100.0',
      }),
    ).toThrow(BusyError)
  })

  it('claims a correction only right after the previous turn of that run', () => {
    const { dir, dbPath } = makeDbPath()
    const store = new Store(dbPath)
    const session = store.createSession({ name: 's1', cwd: dir })
    const { run } = store.acceptRun({
      sessionId: session.id,
      requestId: 'req-1',
      instructions: 'do thing',
      remote: join(dir, 'remote.git'),
      base: 'main',
      baseSha: 'a'.repeat(40),
      worktreePath: (id) => join(dir, 'wt', id),
    })
    const t1 = store.claimTurn({
      sessionId: session.id,
      mode: 'start',
      runId: run.id,
      hostInstanceId: 'inst1',
      bootTime: '100.0',
    })
    store.endTurn(t1.id, 'completed', {
      exitCode: 0,
      endReason: 'exit',
      writerCheck: 'excluded',
    })

    // Stale previousTurnId fails
    expect(() =>
      store.claimTurn({
        sessionId: session.id,
        mode: 'correction',
        runId: run.id,
        previousTurnId: t1.id + 999,
        hostInstanceId: 'inst1',
        bootTime: '100.0',
      }),
    ).toThrow(StaleStateError)

    // Matching previousTurnId succeeds
    const t2 = store.claimTurn({
      sessionId: session.id,
      mode: 'correction',
      runId: run.id,
      previousTurnId: t1.id,
      hostInstanceId: 'inst1',
      bootTime: '100.0',
    })
    expect(t2.runId).toBe(run.id)
    store.endTurn(t2.id, 'completed', {
      exitCode: 0,
      endReason: 'exit',
      writerCheck: 'excluded',
    })

    // Using t1 again after t2 was claimed fails
    expect(() =>
      store.claimTurn({
        sessionId: session.id,
        mode: 'correction',
        runId: run.id,
        previousTurnId: t1.id,
        hostInstanceId: 'inst1',
        bootTime: '100.0',
      }),
    ).toThrow(StaleStateError)
  })

  it('claims a retry atomically and at most three times', () => {
    const { dir, dbPath } = makeDbPath()
    const store = new Store(dbPath)
    const session = store.createSession({ name: 's1', cwd: dir })
    const { run } = store.acceptRun({
      sessionId: session.id,
      requestId: 'req-retry',
      instructions: 'do thing',
      remote: join(dir, 'remote.git'),
      base: 'main',
      baseSha: 'a'.repeat(40),
      worktreePath: (id) => join(dir, 'wt', id),
    })
    const t0 = store.claimTurn({
      sessionId: session.id,
      mode: 'start',
      runId: run.id,
      hostInstanceId: 'inst1',
      bootTime: '100.0',
    })
    store.endTurn(t0.id, 'interrupted', {
      exitCode: null,
      endReason: 'never_started',
      writerCheck: 'excluded',
    })
    store.updateRun(run.id, { status: 'interrupted', retries: 2 }, ['running'])

    const tRetry = store.claimTurn({
      sessionId: session.id,
      mode: 'retry',
      runId: run.id,
      hostInstanceId: 'inst1',
      bootTime: '100.0',
    })
    expect(store.getRun(run.id)?.retries).toBe(3)
    expect(store.getRun(run.id)?.corrections).toBe(0)
    store.endTurn(tRetry.id, 'interrupted', {
      exitCode: null,
      endReason: 'never_started',
      writerCheck: 'excluded',
    })
    store.updateRun(run.id, { status: 'interrupted' }, ['running'])

    expect(() =>
      store.claimTurn({
        sessionId: session.id,
        mode: 'retry',
        runId: run.id,
        hostInstanceId: 'inst1',
        bootTime: '100.0',
      }),
    ).toThrow(StaleStateError)
  })

  it('endTurn does nothing once the turn has ended', () => {
    const { dir, dbPath } = makeDbPath()
    const store = new Store(dbPath)
    const session = store.createSession({ name: 's1', cwd: dir })
    const t = store.claimTurn({
      sessionId: session.id,
      mode: 'chat',
      hostInstanceId: 'inst1',
      bootTime: '100.0',
    })
    expect(
      store.endTurn(t.id, 'cancelled', {
        exitCode: null,
        endReason: 'cancel',
        writerCheck: 'possible',
        freezeReason: 'cancelled by user',
      }),
    ).toBe(true)
    expect(
      store.endTurn(t.id, 'failed', {
        exitCode: 1,
        endReason: 'exit',
        writerCheck: 'excluded',
      }),
    ).toBe(false)
    const row = store.getTurn(t.id)!
    expect(row.status).toBe('cancelled')
    expect(row.writerCheck).toBe('possible')
    expect(row.endReason).toBe('cancel')
  })

  it('a never-started turn gives its message back to the queue', () => {
    const { dir, dbPath } = makeDbPath()
    const store = new Store(dbPath)
    const session = store.createSession({ name: 's1', cwd: dir })
    const msg = store.addMessage({
      sessionId: session.id,
      turnId: null,
      source: 'user',
      text: 'hello',
      queued: true,
    })
    const t = store.claimTurn({
      sessionId: session.id,
      mode: 'chat',
      messageId: msg.id,
      hostInstanceId: 'inst1',
      bootTime: '100.0',
    })
    expect(store.nextQueued(session.id)).toBeUndefined()
    expect(
      store.endTurn(t.id, 'interrupted', {
        exitCode: null,
        endReason: 'never_started',
        writerCheck: 'excluded',
      }),
    ).toBe(true)
    const back = store.nextQueued(session.id)
    expect(back?.id).toBe(msg.id)
    expect(back?.queued).toBe(true)
    expect(back?.turnId).toBeNull()
  })

  it('replayRun finds the run on a frozen session and rejects changed input', () => {
    const { dir, dbPath } = makeDbPath()
    const store = new Store(dbPath)
    const session = store.createSession({ name: 's1', cwd: dir })
    const { run } = store.acceptRun({
      sessionId: session.id,
      requestId: 'req-replay',
      instructions: 'do work',
      remote: join(dir, 'remote.git'),
      base: 'main',
      baseSha: 'b'.repeat(40),
      worktreePath: (id) => join(dir, 'wt', id),
    })
    store.freezeSession(session.id, 'frozen for test')

    const found = store.replayRun({
      sessionId: session.id,
      requestId: 'req-replay',
      instructions: 'do work',
      base: 'main',
    })
    expect(found?.id).toBe(run.id)

    expect(() =>
      store.replayRun({
        sessionId: session.id,
        requestId: 'req-replay',
        instructions: 'different work',
        base: 'main',
      }),
    ).toThrow(ConflictError)

    expect(
      store.replayRun({
        sessionId: session.id,
        requestId: 'req-missing',
        instructions: 'do work',
        base: 'main',
      }),
    ).toBeUndefined()
  })

  it('accepts a run once per request id', () => {
    const { dir, dbPath } = makeDbPath()
    const store = new Store(dbPath)
    const session = store.createSession({ name: 's1', cwd: dir })
    const req = {
      sessionId: session.id,
      remote: join(dir, 'remote.git'),
      base: 'main',
      baseSha: 'c'.repeat(40),
      instructions: 'ship it',
      worktreePath: (id: string) => join(dir, 'wt', id),
    }
    const a = store.acceptRun({ ...req, requestId: 'q1' })
    const b = store.acceptRun({ ...req, requestId: 'q1' })
    expect(a.created).toBe(true)
    expect(b.created).toBe(false)
    expect(b.run.id).toBe(a.run.id)
    expect(() => store.acceptRun({ ...req, requestId: 'q1', instructions: 'other' })).toThrow(ConflictError)
  })

  it('lets two connections race on one request id and creates one run', async () => {
    const { dir, dbPath } = makeDbPath()
    const store = new Store(dbPath)
    const session = store.createSession({ name: 's1', cwd: dir })
    const storeUrl = pathToFileURL(join(process.cwd(), 'src/store.ts')).href
    const shared = new SharedArrayBuffer(4)
    const view = new Int32Array(shared)

    const workerCode = `
      const { parentPort, workerData } = require('node:worker_threads');
      (async () => {
        const { Store } = await import(workerData.storeUrl);
        const s = new Store(workerData.dbPath);
        const view = new Int32Array(workerData.shared);
        parentPort.postMessage('ready');
        while (Atomics.load(view, 0) === 0) {}
        const res = s.acceptRun({
          sessionId: workerData.sessionId,
          requestId: 'race-req',
          instructions: 'concurrent',
          remote: workerData.remote,
          base: 'main',
          baseSha: workerData.baseSha,
          worktreePath: (id) => workerData.wtBase + '/' + id,
        });
        parentPort.postMessage({ id: res.run.id, created: res.created });
      })().catch((err) => {
        parentPort.postMessage({ error: String(err) });
      });
    `

    const runWorker = () =>
      new Promise<{ id: string; created: boolean }>((resolve, reject) => {
        const w = new Worker(workerCode, {
          eval: true,
          workerData: {
            storeUrl,
            dbPath,
            shared,
            sessionId: session.id,
            remote: join(dir, 'remote.git'),
            baseSha: 'd'.repeat(40),
            wtBase: join(dir, 'wt'),
          },
        })
        w.on('message', (msg) => {
          if (msg === 'ready') return
          if (msg.error) reject(new Error(msg.error))
          else resolve(msg)
        })
        w.on('error', reject)
      })

    const w1 = new Promise<{ ready: () => void; done: Promise<{ id: string; created: boolean }> }>((resOuter) => {
      const w = new Worker(workerCode, {
        eval: true,
        workerData: {
          storeUrl,
          dbPath,
          shared,
          sessionId: session.id,
          remote: join(dir, 'remote.git'),
          baseSha: 'd'.repeat(40),
          wtBase: join(dir, 'wt'),
        },
      })
      let resolveDone!: (v: { id: string; created: boolean }) => void
      let rejectDone!: (e: Error) => void
      const done = new Promise<{ id: string; created: boolean }>((res, rej) => {
        resolveDone = res
        rejectDone = rej
      })
      w.on('message', (msg) => {
        if (msg === 'ready') resOuter({ ready: () => {}, done })
        else if (msg.error) rejectDone(new Error(msg.error))
        else resolveDone(msg)
      })
      w.on('error', rejectDone)
    })

    const w2 = new Promise<{ ready: () => void; done: Promise<{ id: string; created: boolean }> }>((resOuter) => {
      const w = new Worker(workerCode, {
        eval: true,
        workerData: {
          storeUrl,
          dbPath,
          shared,
          sessionId: session.id,
          remote: join(dir, 'remote.git'),
          baseSha: 'd'.repeat(40),
          wtBase: join(dir, 'wt'),
        },
      })
      let resolveDone!: (v: { id: string; created: boolean }) => void
      let rejectDone!: (e: Error) => void
      const done = new Promise<{ id: string; created: boolean }>((res, rej) => {
        resolveDone = res
        rejectDone = rej
      })
      w.on('message', (msg) => {
        if (msg === 'ready') resOuter({ ready: () => {}, done })
        else if (msg.error) rejectDone(new Error(msg.error))
        else resolveDone(msg)
      })
      w.on('error', rejectDone)
    })

    void runWorker
    const [aReady, bReady] = await Promise.all([w1, w2])
    Atomics.store(view, 0, 1)
    const [r1, r2] = await Promise.all([aReady.done, bReady.done])
    expect(r1.id).toBe(r2.id)
    expect([r1.created, r2.created].sort()).toEqual([false, true])
    expect(store.listRuns()).toHaveLength(1)
  })

  it('refuses a second active run on a session', () => {
    const { dir, dbPath } = makeDbPath()
    const store = new Store(dbPath)
    const session = store.createSession({ name: 's1', cwd: dir })
    store.acceptRun({
      sessionId: session.id,
      requestId: 'q1',
      instructions: 'first',
      remote: join(dir, 'remote.git'),
      base: 'main',
      baseSha: 'e'.repeat(40),
      worktreePath: (id) => join(dir, 'wt', id),
    })
    expect(() =>
      store.acceptRun({
        sessionId: session.id,
        requestId: 'q2',
        instructions: 'second',
        remote: join(dir, 'remote.git'),
        base: 'main',
        baseSha: 'e'.repeat(40),
        worktreePath: (id) => join(dir, 'wt', id),
      }),
    ).toThrow(BusyError)
  })

  it('refuses everything on a frozen session and keeps the freeze across reopen', () => {
    const { dir, dbPath } = makeDbPath()
    const store = new Store(dbPath)
    const session = store.createSession({ name: 's1', cwd: dir })
    store.freezeSession(session.id, 'writer unknown')

    expect(() =>
      store.acceptRun({
        sessionId: session.id,
        requestId: 'q1',
        instructions: 'run',
        remote: join(dir, 'remote.git'),
        base: 'main',
        baseSha: 'f'.repeat(40),
        worktreePath: (id) => join(dir, 'wt', id),
      }),
    ).toThrow(FrozenError)
    expect(() =>
      store.claimTurn({
        sessionId: session.id,
        mode: 'chat',
        hostInstanceId: 'inst1',
        bootTime: '100.0',
      }),
    ).toThrow(FrozenError)

    const reopened = new Store(dbPath)
    expect(reopened.getSession(session.id)?.frozenReason).toBe('writer unknown')
    expect(reopened.getSession(session.id)?.frozenAt).toBeTruthy()
  })

  it('binds a queued message to exactly one turn', () => {
    const { dir, dbPath } = makeDbPath()
    const store = new Store(dbPath)
    const session = store.createSession({ name: 's1', cwd: dir })
    const msg = store.addMessage({
      sessionId: session.id,
      turnId: null,
      source: 'user',
      text: 'queued text',
      queued: true,
    })
    const t1 = store.claimTurn({
      sessionId: session.id,
      mode: 'chat',
      messageId: msg.id,
      hostInstanceId: 'inst1',
      bootTime: '100.0',
    })
    const bound = store.listMessages(session.id).find((m) => m.id === msg.id)!
    expect(bound.turnId).toBe(t1.id)
    expect(bound.queued).toBe(false)
    store.endTurn(t1.id, 'completed', {
      exitCode: 0,
      endReason: 'exit',
      writerCheck: 'excluded',
    })

    expect(() =>
      store.claimTurn({
        sessionId: session.id,
        mode: 'chat',
        messageId: msg.id,
        hostInstanceId: 'inst1',
        bootTime: '100.0',
      }),
    ).toThrow(StaleStateError)
  })

  it('lists messages in receipt order', () => {
    const { dir, dbPath } = makeDbPath()
    const store = new Store(dbPath)
    const session = store.createSession({ name: 's1', cwd: dir })
    const m1 = store.addMessage({ sessionId: session.id, turnId: null, source: 'user', text: 'first', queued: true })
    const m2 = store.addMessage({ sessionId: session.id, turnId: null, source: 'user', text: 'second', queued: true })
    const t1 = store.claimTurn({
      sessionId: session.id,
      mode: 'chat',
      messageId: m1.id,
      hostInstanceId: 'inst1',
      bootTime: '100.0',
    })
    const m3 = store.addMessage({ sessionId: session.id, turnId: t1.id, source: 'agent', text: 'reply' })
    const msgs = store.listMessages(session.id)
    expect(msgs.map((m) => m.id)).toEqual([m1.id, m2.id, m3.id])
  })

  it('accepts a token only while its turn is running, authorized and owned by this instance', () => {
    const { dir, dbPath } = makeDbPath()
    const store = new Store(dbPath)
    const session = store.createSession({ name: 's1', cwd: dir })
    const t = store.claimTurn({
      sessionId: session.id,
      mode: 'chat',
      hostInstanceId: 'inst1',
      bootTime: '100.0',
    })
    expect(store.turnByToken(t.mcpToken, 'inst1')).toBeUndefined()
    expect(store.authorizeTurn(t.id, 1234, 'Mon Nov 25 00:00:00 2026')).toBe(true)
    expect(store.turnByToken(t.mcpToken, 'inst1')?.id).toBe(t.id)
    expect(store.turnByToken(t.mcpToken, 'other-inst')).toBeUndefined()
    store.endTurn(t.id, 'completed', {
      exitCode: 0,
      endReason: 'exit',
      writerCheck: 'excluded',
    })
    expect(store.turnByToken(t.mcpToken, 'inst1')).toBeUndefined()
  })

  it('settles one accepted result per run', () => {
    const { dir, dbPath } = makeDbPath()
    const store = new Store(dbPath)
    const session = store.createSession({ name: 's1', cwd: dir })
    const { run } = store.acceptRun({
      sessionId: session.id,
      requestId: 'req-settle',
      instructions: 'work',
      remote: join(dir, 'remote.git'),
      base: 'main',
      baseSha: 'a'.repeat(40),
      worktreePath: (id) => join(dir, 'wt', id),
    })
    const t = store.claimTurn({
      sessionId: session.id,
      mode: 'start',
      runId: run.id,
      hostInstanceId: 'inst1',
      bootTime: '100.0',
    })
    store.authorizeTurn(t.id, 4321, 'Mon Nov 25 00:00:00 2026')
    const sha1 = '1'.repeat(40)
    const sha2 = '2'.repeat(40)

    expect(
      store.settleResult({
        token: t.mcpToken,
        hostInstanceId: 'inst1',
        runId: run.id,
        branch: run.branch,
        sha: sha1,
      }),
    ).toBe('accepted')
    expect(
      store.settleResult({
        token: t.mcpToken,
        hostInstanceId: 'inst1',
        runId: run.id,
        branch: run.branch,
        sha: sha1,
      }),
    ).toBe('already_accepted')
    expect(
      store.settleResult({
        token: t.mcpToken,
        hostInstanceId: 'inst1',
        runId: run.id,
        branch: run.branch,
        sha: sha2,
      }),
    ).toBe('different_result_accepted')

    store.endTurn(t.id, 'completed', {
      exitCode: 0,
      endReason: 'exit',
      writerCheck: 'excluded',
    })
    expect(
      store.settleResult({
        token: t.mcpToken,
        hostInstanceId: 'inst1',
        runId: run.id,
        branch: run.branch,
        sha: sha1,
      }),
    ).toBe('turn_not_active')
    expect(store.acceptedResult(run.id)?.payload).toContain(sha1)
    const db = new DatabaseSync(dbPath)
    const row = db
      .prepare('SELECT COUNT(*) AS n FROM results WHERE run_id = ? AND accepted = 1')
      .get(run.id) as { n: number }
    expect(row.n).toBe(1)
    db.close()
  })

  it('maps the SQLite unique error by errcode 2067', () => {
    const { dir, dbPath } = makeDbPath()
    const store = new Store(dbPath)
    const session = store.createSession({ name: 's1', cwd: dir })
    store.claimTurn({
      sessionId: session.id,
      mode: 'chat',
      hostInstanceId: 'inst1',
      bootTime: '100.0',
    })
    let caught: unknown
    try {
      store.claimTurn({
        sessionId: session.id,
        mode: 'chat',
        hostInstanceId: 'inst1',
        bootTime: '100.0',
      })
    } catch (err) {
      caught = err
    }
    expect(caught).toBeInstanceOf(BusyError)

    const session2 = store.createSession({ name: 's2', cwd: dir })
    store.acceptRun({
      sessionId: session2.id,
      requestId: 'u1',
      instructions: 'one',
      remote: join(dir, 'remote.git'),
      base: 'main',
      baseSha: 'a'.repeat(40),
      worktreePath: (id) => join(dir, 'wt', id),
    })
    let caughtRun: unknown
    try {
      store.acceptRun({
        sessionId: session2.id,
        requestId: 'u2',
        instructions: 'two',
        remote: join(dir, 'remote.git'),
        base: 'main',
        baseSha: 'a'.repeat(40),
        worktreePath: (id) => join(dir, 'wt', id),
      })
    } catch (err) {
      caughtRun = err
    }
    expect(caughtRun).toBeInstanceOf(BusyError)
  })

  it('counts a retry when an interrupted run is claimed again', () => {
    const { dir, dbPath } = makeDbPath()
    const store = new Store(dbPath)
    const session = store.createSession({ name: 's1', cwd: dir })
    const { run } = store.acceptRun({
      sessionId: session.id,
      requestId: 'req-count',
      instructions: 'work',
      remote: join(dir, 'remote.git'),
      base: 'main',
      baseSha: 'a'.repeat(40),
      worktreePath: (id) => join(dir, 'wt', id),
    })
    store.updateRun(run.id, { status: 'interrupted' }, ['pending'])
    store.claimTurn({
      sessionId: session.id,
      mode: 'retry',
      runId: run.id,
      hostInstanceId: 'inst1',
      bootTime: '100.0',
    })
    const updated = store.getRun(run.id)!
    expect(updated.status).toBe('running')
    expect(updated.retries).toBe(1)
  })
})
