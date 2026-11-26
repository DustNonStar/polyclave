import { afterEach, describe, expect, it } from 'vitest'
import { execFileSync } from 'node:child_process'
import { EventEmitter } from 'node:events'
import { existsSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { prepareCodexHome } from '../src/codex.ts'
import { checkRunBranch } from '../src/git-effect.ts'
import { hostInstanceId } from '../src/ids.ts'
import { startHostMcp, type HostMcp } from '../src/mcp.ts'
import { TurnRunner } from '../src/runner.ts'
import { Runs } from '../src/runs.ts'
import { BusyError, FrozenError, StaleStateError, Store } from '../src/store.ts'
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

async function setupRuns(dir: string, script: Record<string, unknown>) {
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

  const runs = new Runs({
    store,
    runner,
    stateDir: dir,
    check: checkRunBranch,
    bus,
  })
  const session = store.createSession({ name: 's1', cwd: work })
  return { work, remote, store, inst, mcp, runner, runs, session, logPath, scriptPath, bus }
}

describe('Runs', () => {
  it('completes when the agent pushes and submits', async () => {
    const dir = trackTemp(tempDir('polyclave-runs-1-'))
    const { remote, runs, session, logPath } = await setupRuns(dir, {
      '1': [
        { commit: { file: 'hello.txt', content: 'hi\n', trailer: true } },
        { push: true },
        { submitPushed: true },
        { reply: 'done' },
      ],
    })

    const { run, created } = await runs.create({
      sessionId: session.id,
      requestId: 'req-1',
      instructions: 'add hello.txt',
    })
    expect(created).toBe(true)
    const finished = await runs.start(run.id)
    const tip = execFileSync('git', ['-C', remote, 'rev-parse', `refs/heads/${run.branch}`], {
      encoding: 'utf8',
    }).trim()

    expect(finished.status).toBe('done')
    expect(finished.resultSha).toBe(tip)
    expect(finished.resultSource).toBe('agent')

    const prompts = readFakeLog(logPath).filter((e) => e.event === 'prompt_started')
    expect(prompts).toHaveLength(1)
    expect(prompts[0]?.cwd).toBe(run.worktreePath)
  })

  it('replays a request id without a second run or worktree', async () => {
    const dir = trackTemp(tempDir('polyclave-runs-2-'))
    const { runs, session } = await setupRuns(dir, {
      '1': [
        { commit: { file: 'a.txt', content: 'a\n', trailer: true } },
        { push: true },
        { submitPushed: true },
        { reply: 'done' },
      ],
    })
    const a = await runs.create({
      sessionId: session.id,
      requestId: 'req-replay',
      instructions: 'do a',
    })
    const b = await runs.create({
      sessionId: session.id,
      requestId: 'req-replay',
      instructions: 'do a',
    })
    expect(a.created).toBe(true)
    expect(b.created).toBe(false)
    expect(b.run.id).toBe(a.run.id)
    await runs.start(a.run.id)

    const c = await runs.create({
      sessionId: session.id,
      requestId: 'req-replay',
      instructions: 'do a',
    })
    expect(c.created).toBe(false)
    expect(c.run.id).toBe(a.run.id)
    expect(readdirSync(join(dir, 'worktrees'))).toEqual([a.run.id])
  })

  it('two sessions on one repo work in separate worktrees', async () => {
    const dir = trackTemp(tempDir('polyclave-runs-3-'))
    const { work, remote, store, runs, session } = await setupRuns(dir, {
      '1': [
        { commit: { file: 'shared.txt', content: 'from-1\n', trailer: true } },
        { push: true },
        { submitPushed: true },
        { reply: 'ok1' },
      ],
      '2': [
        { commit: { file: 'shared.txt', content: 'from-2\n', trailer: true } },
        { push: true },
        { submitPushed: true },
        { reply: 'ok2' },
      ],
    })
    const userHead = execFileSync('git', ['-C', work, 'rev-parse', 'HEAD'], {
      encoding: 'utf8',
    }).trim()
    writeFileSync(join(work, 'dirty.txt'), 'keep me\n')

    const session2 = store.createSession({ name: 's2', cwd: work })
    const r1 = await runs.create({ sessionId: session.id, requestId: 'q1', instructions: 'one' })
    const r2 = await runs.create({ sessionId: session2.id, requestId: 'q2', instructions: 'two' })

    const [f1, f2] = await Promise.all([runs.start(r1.run.id), runs.start(r2.run.id)])
    expect(f1.status).toBe('done')
    expect(f2.status).toBe('done')
    expect(f1.worktreePath).not.toBe(f2.worktreePath)

    const c1 = execFileSync('git', ['-C', remote, 'show', `${f1.resultSha}:shared.txt`], {
      encoding: 'utf8',
    })
    const c2 = execFileSync('git', ['-C', remote, 'show', `${f2.resultSha}:shared.txt`], {
      encoding: 'utf8',
    })
    expect([c1, c2].sort()).toEqual(['from-1\n', 'from-2\n'])
    expect(execFileSync('git', ['-C', work, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim()).toBe(
      userHead,
    )
    expect(readFileSync(join(work, 'dirty.txt'), 'utf8')).toBe('keep me\n')
  })

  it('pushed, accepted, then non-zero exit is done', async () => {
    const dir = trackTemp(tempDir('polyclave-runs-4-'))
    const { runs, session } = await setupRuns(dir, {
      '1': [
        { commit: { file: 'x.txt', content: 'x\n', trailer: true } },
        { push: true },
        { submitPushed: true },
        { exit: 1, stderr: 'late crash' },
      ],
    })
    const { run } = await runs.create({ sessionId: session.id, requestId: 'q1', instructions: 'x' })
    const finished = await runs.start(run.id)
    expect(finished.status).toBe('done')
  })

  it('pushed but never submitted: needs_review at once, no correction, branch kept', async () => {
    const dir = trackTemp(tempDir('polyclave-runs-5-'))
    const { remote, runs, session, logPath } = await setupRuns(dir, {
      '1': [
        { commit: { file: 'p.txt', content: 'p\n', trailer: true } },
        { push: true },
        { reply: 'forgot submit' },
      ],
      '2': [{ reply: 'should not run' }],
    })
    const { run } = await runs.create({ sessionId: session.id, requestId: 'q1', instructions: 'p' })
    const finished = await runs.start(run.id)
    expect(finished.status).toBe('needs_review')
    expect(finished.detail).toBe('branch pushed but no accepted result')
    expect(finished.observedSha).toMatch(/^[0-9a-f]{40}$/)
    expect(readFakeLog(logPath).filter((e) => e.event === 'prompt_started')).toHaveLength(1)
    expect(
      execFileSync('git', ['-C', remote, 'rev-parse', `refs/heads/${run.branch}`], {
        encoding: 'utf8',
      }).trim(),
    ).toBe(finished.observedSha)
  })

  it('sends up to two corrections when nothing was pushed, then fails', async () => {
    const dir = trackTemp(tempDir('polyclave-runs-6-'))
    const { runs, session, logPath } = await setupRuns(dir, {
      '1': [{ reply: 'did nothing' }],
      '2': [{ reply: 'still nothing' }],
      '3': [{ reply: 'gave up' }],
    })
    const { run } = await runs.create({ sessionId: session.id, requestId: 'q1', instructions: 'x' })
    const finished = await runs.start(run.id)
    expect(finished.status).toBe('failed')
    expect(finished.detail).toBe('no accepted result after 2 corrections')
    expect(finished.corrections).toBe(2)
    expect(readFakeLog(logPath).filter((e) => e.event === 'prompt_started')).toHaveLength(3)
  })

  it('completes after one correction', async () => {
    const dir = trackTemp(tempDir('polyclave-runs-7-'))
    const { runs, session, logPath } = await setupRuns(dir, {
      '1': [{ reply: 'oops forgot' }],
      '2': [
        { commit: { file: 'c.txt', content: 'c\n', trailer: true } },
        { push: true },
        { submitPushed: true },
        { reply: 'fixed' },
      ],
    })
    const { run } = await runs.create({ sessionId: session.id, requestId: 'q1', instructions: 'c' })
    const finished = await runs.start(run.id)
    expect(finished.status).toBe('done')
    expect(finished.corrections).toBe(1)
    expect(readFakeLog(logPath).filter((e) => e.event === 'prompt_started')).toHaveLength(2)
  })

  it('fails the run when codex exits immediately', async () => {
    const dir = trackTemp(tempDir('polyclave-runs-8-'))
    const { runs, session } = await setupRuns(dir, {
      '1': [{ exit: 3, stderr: 'boom' }],
    })
    const { run } = await runs.create({ sessionId: session.id, requestId: 'q1', instructions: 'x' })
    const finished = await runs.start(run.id)
    expect(finished.status).toBe('failed')
    expect(finished.detail).toBe('turn failed')
  })

  it('cancel makes the run needs_review, freezes the session, no correction', async () => {
    const dir = trackTemp(tempDir('polyclave-runs-9-'))
    const b1 = barrier(dir, 'b1')
    const { store, runner, runs, session, logPath } = await setupRuns(dir, {
      '1': [{ wait: 'b1' }, { reply: 'never' }],
      '2': [{ reply: 'never' }],
    })
    const { run } = await runs.create({ sessionId: session.id, requestId: 'q1', instructions: 'x' })
    const startP = runs.start(run.id)
    await b1.waitReached()
    await runner.cancel(session.id)
    const finished = await startP

    expect(finished.status).toBe('needs_review')
    expect(finished.detail).toContain('cancelled while the agent was running')
    expect(store.getSession(session.id)?.frozenReason).toBeTruthy()
    expect(readFakeLog(logPath).filter((e) => e.event === 'prompt_started')).toHaveLength(1)
  })

  it('submitted, then pushed again: needs_review because the tip moved', async () => {
    const dir = trackTemp(tempDir('polyclave-runs-10-'))
    const { runs, session } = await setupRuns(dir, {
      '1': [
        { commit: { file: '1.txt', content: '1\n', trailer: true } },
        { push: true },
        { submitPushed: true },
        { commit: { file: '2.txt', content: '2\n', trailer: true } },
        { push: true },
        { reply: 'pushed again' },
      ],
    })
    const { run } = await runs.create({ sessionId: session.id, requestId: 'q1', instructions: 'x' })
    const finished = await runs.start(run.id)
    expect(finished.status).toBe('needs_review')
    expect(finished.detail).toContain('differs from accepted')
  })

  it('retry reuses the worktree and its local commit', async () => {
    const dir = trackTemp(tempDir('polyclave-runs-11-'))
    const { work, remote, store, runs, session, logPath } = await setupRuns(dir, {
      '1': [{ push: true }, { submitPushed: true }, { reply: 'pushed existing' }],
    })
    const { run } = await runs.create({ sessionId: session.id, requestId: 'q1', instructions: 'x' })

    // Create worktree and local commit X, simulate interrupted run
    execFileSync('git', ['-C', work, 'worktree', 'add', '-b', run.branch, run.worktreePath, run.baseSha])
    writeFileSync(join(run.worktreePath, 'local.txt'), 'local\n')
    execFileSync('git', ['-C', run.worktreePath, 'add', '.'])
    execFileSync('git', ['-C', run.worktreePath, 'commit', '-m', 'local', '--trailer', `Run-Id: ${run.id}`])
    const shaX = execFileSync('git', ['-C', run.worktreePath, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim()
    store.updateRun(run.id, { status: 'interrupted' }, ['pending'])

    await runs.retry(run.id)
    await runs.idle()

    const after = store.getRun(run.id)!
    expect(after.status).toBe('done')
    expect(after.resultSha).toBe(shaX)
    expect(
      execFileSync('git', ['-C', remote, 'rev-parse', `refs/heads/${run.branch}`], {
        encoding: 'utf8',
      }).trim(),
    ).toBe(shaX)
    expect(readFakeLog(logPath).filter((e) => e.event === 'push_attempt')).toHaveLength(1)
  })

  it('stops after three retries', async () => {
    const dir = trackTemp(tempDir('polyclave-runs-12-'))
    const { store, runs, session } = await setupRuns(dir, {})
    const { run } = await runs.create({ sessionId: session.id, requestId: 'q1', instructions: 'x' })
    store.updateRun(run.id, { status: 'interrupted', retries: 3 }, ['pending'])

    const out = await runs.retry(run.id)
    expect(out.status).toBe('needs_review')
    expect(out.detail).toBe('retry limit reached')
  })

  it('resolve accept records the user decision and the seen sha', async () => {
    const dir = trackTemp(tempDir('polyclave-runs-13-'))
    const { runs, session } = await setupRuns(dir, {
      '1': [
        { commit: { file: 'r.txt', content: 'r\n', trailer: true } },
        { push: true },
        { reply: 'no submit' },
      ],
    })
    const { run } = await runs.create({ sessionId: session.id, requestId: 'q1', instructions: 'r' })
    const reviewRun = await runs.start(run.id)
    expect(reviewRun.status).toBe('needs_review')
    const sha = reviewRun.observedSha!

    const resolved = await runs.resolve(run.id, {
      decision: 'accept',
      sha,
      note: 'verified manually',
    })
    expect(resolved.status).toBe('done')
    expect(resolved.resultSha).toBe(sha)
    expect(resolved.resultSource).toBe('user')
    expect(resolved.detail).toBeNull()
    expect(resolved.resolution).toContain(`accept: verified manually (seen ${sha.slice(0, 8)} at `)
  })

  it('refuses to resolve a run while its session is frozen', async () => {
    const dir = trackTemp(tempDir('polyclave-runs-resolve-frozen-'))
    const { store, runs, session } = await setupRuns(dir, {
      '1': [
        { commit: { file: 'f.txt', content: 'f\n', trailer: true } },
        { push: true },
        { reply: 'pushed, no submit' },
      ],
    })
    const { run } = await runs.create({
      sessionId: session.id,
      requestId: 'req-frozen-resolve',
      instructions: 'f',
    })
    const reviewRun = await runs.start(run.id)
    expect(reviewRun.status).toBe('needs_review')
    const sha = reviewRun.observedSha!
    expect(sha).toBeTruthy()

    store.freezeSession(session.id, 'host stopped while an agent was running')
    const before = store.getRun(run.id)!

    for (const decision of ['accept', 'fail', 'allow_retry'] as const) {
      await expect(runs.resolve(run.id, { decision, sha, note: 'while frozen' })).rejects.toThrow(
        FrozenError,
      )
    }
    expect(store.getRun(run.id)).toEqual(before)

    store.unfreezeSession(session.id)
    const resolved = await runs.resolve(run.id, { decision: 'accept', sha, note: 'after unlock' })
    expect(resolved.status).toBe('done')
    expect(resolved.resultSha).toBe(sha)
  })

  it('resolve accept refuses when the tip moved after the user looked', async () => {
    const dir = trackTemp(tempDir('polyclave-runs-14-'))
    const { remote, store, runs, session } = await setupRuns(dir, {
      '1': [
        { commit: { file: 'r.txt', content: '1\n', trailer: true } },
        { push: true },
        { reply: 'no submit' },
      ],
    })
    const { run } = await runs.create({ sessionId: session.id, requestId: 'q1', instructions: 'r' })
    const reviewRun = await runs.start(run.id)
    const shaX = reviewRun.observedSha!

    // Remote moves to Y
    writeFileSync(join(run.worktreePath, 'r2.txt'), '2\n')
    execFileSync('git', ['-C', run.worktreePath, 'add', '.'])
    execFileSync('git', ['-C', run.worktreePath, 'commit', '-m', 'second', '--trailer', `Run-Id: ${run.id}`])
    const shaY = execFileSync('git', ['-C', run.worktreePath, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim()
    execFileSync('git', ['-C', run.worktreePath, 'push', remote, `refs/heads/${run.branch}:refs/heads/${run.branch}`])

    await expect(
      runs.resolve(run.id, { decision: 'accept', sha: shaX, note: 'old sha' }),
    ).rejects.toThrow(StaleStateError)

    const updated = store.getRun(run.id)!
    expect(updated.status).toBe('needs_review')
    expect(updated.observedSha).toBe(shaY)
  })

  it('two concurrent retries start one turn', async () => {
    const dir = trackTemp(tempDir('polyclave-runs-15-'))
    const { store, runs, session, logPath } = await setupRuns(dir, {
      '1': [
        { commit: { file: 'ok.txt', content: 'ok\n', trailer: true } },
        { push: true },
        { submitPushed: true },
        { reply: 'ok' },
      ],
    })
    const { run } = await runs.create({ sessionId: session.id, requestId: 'q1', instructions: 'ok' })
    store.updateRun(run.id, { status: 'interrupted' }, ['pending'])

    const [a, b] = await Promise.allSettled([runs.retry(run.id), runs.retry(run.id)])
    await runs.idle()

    const fulfilled = [a, b].filter((r) => r.status === 'fulfilled')
    const rejected = [a, b].filter((r) => r.status === 'rejected')
    expect(fulfilled).toHaveLength(1)
    expect(rejected).toHaveLength(1)
    expect((rejected[0] as PromiseRejectedResult).reason).toBeInstanceOf(BusyError)
    expect(readFakeLog(logPath).filter((e) => e.event === 'prompt_started')).toHaveLength(1)
  })

  it('a retry after a crash before the worktree existed creates it', async () => {
    const dir = trackTemp(tempDir('polyclave-runs-16-'))
    const { store, runs, session } = await setupRuns(dir, {
      '1': [
        { commit: { file: 'new.txt', content: 'new\n', trailer: true } },
        { push: true },
        { submitPushed: true },
        { reply: 'done' },
      ],
    })
    const { run } = await runs.create({ sessionId: session.id, requestId: 'q1', instructions: 'x' })
    expect(existsSync(run.worktreePath)).toBe(false)
    store.updateRun(run.id, { status: 'interrupted' }, ['pending'])

    await runs.retry(run.id)
    await runs.idle()

    expect(existsSync(run.worktreePath)).toBe(true)
    expect(store.getRun(run.id)?.status).toBe('done')
  })

  it('shutdown during a run that pushed nothing leaves it interrupted, not corrected', async () => {
    const dir = trackTemp(tempDir('polyclave-runs-17-'))
    const b1 = barrier(dir, 'b1')
    const { store, runner, runs, session, logPath } = await setupRuns(dir, {
      '1': [{ wait: 'b1' }, { reply: 'finished cleanly without push' }],
      '2': [{ reply: 'should not run correction' }],
    })
    const { run } = await runs.create({ sessionId: session.id, requestId: 'q1', instructions: 'x' })
    const startP = runs.start(run.id)
    await b1.waitReached()

    const shutP = runner.shutdown()
    b1.release()
    await shutP
    const finished = await startP

    expect(finished.status).toBe('interrupted')
    expect(finished.detail).toBe('host stopped before completion; safe to retry')
    expect(finished.corrections).toBe(0)
    expect(store.getRun(run.id)?.corrections).toBe(0)
    expect(readFakeLog(logPath).filter((e) => e.event === 'prompt_started')).toHaveLength(1)
  })

  it('does not deliver a user message between the turns of a run', async () => {
    const dir = trackTemp(tempDir('polyclave-runs-18-'))
    const b1 = barrier(dir, 'b1')
    const { runner, runs, session, logPath } = await setupRuns(dir, {
      '1': [{ wait: 'b1' }, { reply: 'no submit yet' }],
      '2': [
        { commit: { file: 'f.txt', content: 'f\n', trailer: true } },
        { push: true },
        { submitPushed: true },
        { reply: 'done in corr' },
      ],
      '3': [{ reply: 'chat reply' }],
    })
    const { run } = await runs.create({ sessionId: session.id, requestId: 'q1', instructions: 'f' })
    const startP = runs.start(run.id)
    await b1.waitReached()

    const userMsg = runner.sendUser(session.id, 'user queued during run')
    expect(userMsg.queued).toBe(true)
    b1.release()

    const finished = await startP
    expect(finished.status).toBe('done')
    await runner.idle()

    const prompts = readFakeLog(logPath).filter((e) => e.event === 'prompt_started')
    expect(prompts).toHaveLength(3)
    expect(prompts[0]?.prompt).toContain(`Task ${run.id}.`)
    expect(prompts[1]?.prompt).toContain(`Task ${run.id}: the host has no accepted result yet`)
    expect(prompts[2]?.prompt).toBe('user queued during run')
  })

  it('resolve accept and fail clear stale detail', async () => {
    const dir = trackTemp(tempDir('polyclave-runs-19-'))
    const b2 = barrier(dir, 'b2')
    const { store, runner, runs, session } = await setupRuns(dir, {
      '1': [
        { commit: { file: 'a.txt', content: 'a\n', trailer: true } },
        { push: true },
        { exit: 1 },
      ],
      '2': [{ wait: 'b2' }, { reply: 'never finished' }],
    })

    const { run: runAccept } = await runs.create({
      sessionId: session.id,
      requestId: 'q-accept',
      instructions: 'accept case',
    })
    const reviewAccept = await runs.start(runAccept.id)
    expect(reviewAccept.status).toBe('needs_review')
    expect(reviewAccept.detail).toBeTruthy()

    const accepted = await runs.resolve(runAccept.id, {
      decision: 'accept',
      sha: reviewAccept.observedSha!,
      note: 'verified clean',
    })
    expect(accepted.status).toBe('done')
    expect(accepted.detail).toBeNull()
    expect(store.getRun(runAccept.id)?.detail).toBeNull()

    store.unfreezeSession(session.id)

    const { run: runFail } = await runs.create({
      sessionId: session.id,
      requestId: 'q-fail',
      instructions: 'fail case',
    })
    const failP = runs.start(runFail.id)
    await b2.waitReached()
    await runner.cancel(session.id)
    const reviewFail = await failP
    expect(reviewFail.status).toBe('needs_review')
    expect(reviewFail.detail).toContain('an agent process may still be writing')

    store.unfreezeSession(session.id)
    const failed = await runs.resolve(runFail.id, {
      decision: 'fail',
      note: 'confirmed broken',
    })
    expect(failed.status).toBe('failed')
    expect(failed.detail).toBeNull()
    expect(store.getRun(runFail.id)?.detail).toBeNull()
  })
})
