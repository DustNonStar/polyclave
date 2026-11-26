import { afterEach, describe, expect, it } from 'vitest'
import { EventEmitter } from 'node:events'
import { execFileSync, spawn } from 'node:child_process'
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { prepareCodexHome } from '../src/codex.ts'
import { checkRunBranch, resolveBaseSha } from '../src/git-effect.ts'
import { startHostMcp } from '../src/mcp.ts'
import { processStartTime } from '../src/proc.ts'
import { recover } from '../src/recovery.ts'
import { TurnRunner } from '../src/runner.ts'
import { Runs } from '../src/runs.ts'
import { Store } from '../src/store.ts'
import { FAKE_CODEX, makeRepo, tempDir, waitFor } from './helpers.ts'

const temps: string[] = []
const cleanupPids: number[] = []

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

afterEach(() => {
  while (cleanupPids.length > 0) {
    const pid = cleanupPids.pop()!
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

describe('recover', () => {
  it('a turn that was never authorized is safe: no freeze, run retryable', async () => {
    const dir = trackTemp(tempDir('polyclave-rec-1-'))
    const { work, remote } = makeRepo(dir)
    const baseSha = await resolveBaseSha(remote, 'main')
    const store = new Store(join(dir, 'polyclave.sqlite'))
    const session = store.createSession({ name: 's1', cwd: work })
    const msg = store.addMessage({
      sessionId: session.id,
      turnId: null,
      source: 'user',
      text: 'queued before crash',
      queued: true,
    })
    const { run } = store.acceptRun({
      sessionId: session.id,
      requestId: 'q1',
      instructions: 'work',
      remote,
      base: 'main',
      baseSha,
      worktreePath: (id) => join(dir, 'wt', id),
    })
    const turn = store.claimTurn({
      sessionId: session.id,
      mode: 'start',
      runId: run.id,
      hostInstanceId: 'dead-inst',
      bootTime: '100.0',
    })

    const report = await recover({
      store,
      hostInstanceId: 'new-inst',
      bootTime: '100.0',
      check: checkRunBranch,
    })
    expect(report.turns).toEqual([
      { id: turn.id, sessionId: session.id, outcome: 'never_started' },
    ])
    expect(report.frozenSessions).toEqual([])
    expect(store.getSession(session.id)?.frozenAt).toBeNull()
    expect(store.getTurn(turn.id)?.endReason).toBe('never_started')
    expect(store.getTurn(turn.id)?.writerCheck).toBe('excluded')
    expect(store.getRun(run.id)?.status).toBe('interrupted')
    expect(store.getRun(run.id)?.detail).toBe('not on the remote; safe to retry')
    expect(store.nextQueued(session.id)?.id).toBe(msg.id)
  })

  it('an authorized turn on the same boot freezes the session even when its group is gone', async () => {
    const dir = trackTemp(tempDir('polyclave-rec-2-'))
    const { work, remote } = makeRepo(dir)
    const baseSha = await resolveBaseSha(remote, 'main')
    const store = new Store(join(dir, 'polyclave.sqlite'))
    const session = store.createSession({ name: 's1', cwd: work })
    const { run } = store.acceptRun({
      sessionId: session.id,
      requestId: 'q1',
      instructions: 'work',
      remote,
      base: 'main',
      baseSha,
      worktreePath: (id) => join(dir, 'wt', id),
    })
    const turn = store.claimTurn({
      sessionId: session.id,
      mode: 'start',
      runId: run.id,
      hostInstanceId: 'dead-inst',
      bootTime: '100.0',
    })
    store.authorizeTurn(turn.id, 999999, 'Mon Jan  1 00:00:00 2020')

    const report = await recover({
      store,
      hostInstanceId: 'new-inst',
      bootTime: '100.0',
      check: checkRunBranch,
      liveness: () => 'gone',
    })
    expect(report.turns).toEqual([
      { id: turn.id, sessionId: session.id, outcome: 'already_gone' },
    ])
    expect(report.frozenSessions).toEqual([session.id])
    expect(store.getSession(session.id)?.frozenReason).toBe(
      `host stopped while an agent was running (turn ${turn.id})`,
    )
    expect(store.getRun(run.id)?.status).toBe('needs_review')
    expect(store.getRun(run.id)?.detail).toBe('an agent process may still be writing')
  })

  it('kills a live old group, still freezes', async () => {
    const dir = trackTemp(tempDir('polyclave-rec-3-'))
    const { work, remote } = makeRepo(dir)
    const baseSha = await resolveBaseSha(remote, 'main')
    const store = new Store(join(dir, 'polyclave.sqlite'))
    const session = store.createSession({ name: 's1', cwd: work })
    const { run } = store.acceptRun({
      sessionId: session.id,
      requestId: 'q1',
      instructions: 'work',
      remote,
      base: 'main',
      baseSha,
      worktreePath: (id) => join(dir, 'wt', id),
    })

    const ready = join(dir, 'live.ready')
    const child = spawn('sh', ['-c', `touch "${ready}"; exec sleep 30`], {
      detached: true,
      stdio: 'ignore',
    })
    const pgid = child.pid!
    cleanupPids.push(pgid)
    await waitFor(() => existsSync(ready))
    const startedAt = processStartTime(pgid)

    const turn = store.claimTurn({
      sessionId: session.id,
      mode: 'start',
      runId: run.id,
      hostInstanceId: 'dead-inst',
      bootTime: '100.0',
    })
    store.authorizeTurn(turn.id, pgid, startedAt)

    const report = await recover({
      store,
      hostInstanceId: 'new-inst',
      bootTime: '100.0',
      check: checkRunBranch,
    })
    expect(report.killedGroups).toEqual([pgid])
    expect(report.turns).toEqual([
      { id: turn.id, sessionId: session.id, outcome: 'killed' },
    ])
    expect(isPidAlive(pgid)).toBe(false)
    expect(store.getSession(session.id)?.frozenAt).toBeTruthy()
    expect(store.getRun(run.id)?.status).toBe('needs_review')
  })

  it('never kills when liveness is unverifiable', async () => {
    const dir = trackTemp(tempDir('polyclave-rec-4-'))
    const { work, remote } = makeRepo(dir)
    const baseSha = await resolveBaseSha(remote, 'main')
    const store = new Store(join(dir, 'polyclave.sqlite'))
    const session = store.createSession({ name: 's1', cwd: work })
    const { run } = store.acceptRun({
      sessionId: session.id,
      requestId: 'q1',
      instructions: 'work',
      remote,
      base: 'main',
      baseSha,
      worktreePath: (id) => join(dir, 'wt', id),
    })

    const memberPidFile = join(dir, 'member.pid')
    const ready = join(dir, 'leader.ready')
    const leader = spawn(
      'sh',
      ['-c', `sleep 30 & echo $! > "${memberPidFile}"; touch "${ready}"`],
      { detached: true, stdio: 'ignore' },
    )
    const pgid = leader.pid!
    cleanupPids.push(pgid)
    await waitFor(() => existsSync(ready) && !isPidAlive(pgid))
    const memberPid = Number(readFileSync(memberPidFile, 'utf8').trim())
    cleanupPids.push(memberPid)

    const turn = store.claimTurn({
      sessionId: session.id,
      mode: 'start',
      runId: run.id,
      hostInstanceId: 'dead-inst',
      bootTime: '100.0',
    })
    store.authorizeTurn(turn.id, pgid, 'Mon Jan  1 00:00:00 2020')

    const report = await recover({
      store,
      hostInstanceId: 'new-inst',
      bootTime: '100.0',
      check: checkRunBranch,
    })
    expect(report.unverifiableGroups).toEqual([pgid])
    expect(report.killedGroups).toEqual([])
    expect(report.turns).toEqual([
      { id: turn.id, sessionId: session.id, outcome: 'unverifiable' },
    ])
    expect(isPidAlive(memberPid)).toBe(true)
    expect(store.getSession(session.id)?.frozenAt).toBeTruthy()
    expect(store.getRun(run.id)?.status).toBe('needs_review')
  })

  it('a different boot time clears the writer', async () => {
    const dir = trackTemp(tempDir('polyclave-rec-5-'))
    const { work, remote } = makeRepo(dir)
    const baseSha = await resolveBaseSha(remote, 'main')
    const store = new Store(join(dir, 'polyclave.sqlite'))
    const session = store.createSession({ name: 's1', cwd: work })
    const { run } = store.acceptRun({
      sessionId: session.id,
      requestId: 'q1',
      instructions: 'work',
      remote,
      base: 'main',
      baseSha,
      worktreePath: (id) => join(dir, 'wt', id),
    })
    const turn = store.claimTurn({
      sessionId: session.id,
      mode: 'start',
      runId: run.id,
      hostInstanceId: 'dead-inst',
      bootTime: '100.0',
    })
    store.authorizeTurn(turn.id, 999999, 'Mon Jan  1 00:00:00 2020')

    const report = await recover({
      store,
      hostInstanceId: 'new-inst',
      bootTime: '200.0',
      check: checkRunBranch,
    })
    expect(report.frozenSessions).toEqual([])
    expect(store.getSession(session.id)?.frozenAt).toBeNull()
    expect(store.getTurn(turn.id)?.writerCheck).toBe('excluded')
    expect(store.getRun(run.id)?.status).toBe('interrupted')
    expect(store.getRun(run.id)?.detail).toBe('not on the remote; safe to retry')
  })

  it('owns turns by instance id, not pid', async () => {
    const dir = trackTemp(tempDir('polyclave-rec-6-'))
    const { work, remote } = makeRepo(dir)
    const baseSha = await resolveBaseSha(remote, 'main')
    const store = new Store(join(dir, 'polyclave.sqlite'))
    const session = store.createSession({ name: 's1', cwd: work })
    const turn = store.claimTurn({
      sessionId: session.id,
      mode: 'chat',
      hostInstanceId: 'old-inst-same-pid',
      bootTime: '100.0',
    })
    void baseSha
    const report = await recover({
      store,
      hostInstanceId: 'new-inst',
      bootTime: '100.0',
      check: checkRunBranch,
    })
    expect(report.turns).toEqual([
      { id: turn.id, sessionId: session.id, outcome: 'never_started' },
    ])
    expect(store.getTurn(turn.id)?.status).toBe('interrupted')
  })

  it('leaves turns of the current instance alone', async () => {
    const dir = trackTemp(tempDir('polyclave-rec-7-'))
    const { work, remote } = makeRepo(dir)
    const baseSha = await resolveBaseSha(remote, 'main')
    const store = new Store(join(dir, 'polyclave.sqlite'))
    const session = store.createSession({ name: 's1', cwd: work })
    const { run } = store.acceptRun({
      sessionId: session.id,
      requestId: 'q1',
      instructions: 'work',
      remote,
      base: 'main',
      baseSha,
      worktreePath: (id) => join(dir, 'wt', id),
    })
    const turn = store.claimTurn({
      sessionId: session.id,
      mode: 'start',
      runId: run.id,
      hostInstanceId: 'cur-inst',
      bootTime: '100.0',
    })

    const report = await recover({
      store,
      hostInstanceId: 'cur-inst',
      bootTime: '100.0',
      check: checkRunBranch,
    })
    expect(report.turns).toEqual([])
    expect(report.runs).toEqual([])
    expect(store.getTurn(turn.id)?.status).toBe('running')
    expect(store.getRun(run.id)?.status).toBe('running')
  })

  it('recovers a pending run that never got a turn', async () => {
    const dir = trackTemp(tempDir('polyclave-rec-8-'))
    const { work, remote } = makeRepo(dir)
    const baseSha = await resolveBaseSha(remote, 'main')
    const store = new Store(join(dir, 'polyclave.sqlite'))
    const session = store.createSession({ name: 's1', cwd: work })
    const { run } = store.acceptRun({
      sessionId: session.id,
      requestId: 'q-pending',
      instructions: 'work',
      remote,
      base: 'main',
      baseSha,
      worktreePath: (id) => join(dir, 'wt', id),
    })

    const report = await recover({
      store,
      hostInstanceId: 'new-inst',
      bootTime: '100.0',
      check: checkRunBranch,
    })
    expect(report.runs).toEqual([{ id: run.id, outcome: 'retryable' }])
    expect(store.getRun(run.id)?.status).toBe('interrupted')
    expect(
      store.replayRun({
        sessionId: session.id,
        requestId: 'q-pending',
        instructions: 'work',
        base: 'main',
      })?.id,
    ).toBe(run.id)
  })

  it('keeps a run interrupted when the bare repo cannot be read', async () => {
    const dir = trackTemp(tempDir('polyclave-rec-9-'))
    const { work, remote } = makeRepo(dir)
    const baseSha = await resolveBaseSha(remote, 'main')
    const store = new Store(join(dir, 'polyclave.sqlite'))
    const session = store.createSession({ name: 's1', cwd: work })
    const { run } = store.acceptRun({
      sessionId: session.id,
      requestId: 'q1',
      instructions: 'work',
      remote,
      base: 'main',
      baseSha,
      worktreePath: (id) => join(dir, 'wt', id),
    })
    rmSync(remote, { recursive: true, force: true })

    const report = await recover({
      store,
      hostInstanceId: 'new-inst',
      bootTime: '100.0',
      check: checkRunBranch,
    })
    expect(report.runs).toEqual([{ id: run.id, outcome: 'check_failed' }])
    expect(store.getRun(run.id)?.status).toBe('interrupted')
    expect(store.getRun(run.id)?.detail).toContain('remote check failed:')
  })

  it('confirms an accepted result left by the dead host when writers are excluded', async () => {
    const dir = trackTemp(tempDir('polyclave-rec-10-'))
    const { work, remote } = makeRepo(dir)
    const baseSha = await resolveBaseSha(remote, 'main')
    const store = new Store(join(dir, 'polyclave.sqlite'))
    const session = store.createSession({ name: 's1', cwd: work })
    const { run } = store.acceptRun({
      sessionId: session.id,
      requestId: 'q1',
      instructions: 'work',
      remote,
      base: 'main',
      baseSha,
      worktreePath: (id) => join(dir, 'wt', id),
    })

    // Turn 1: pushed & accepted
    const t1 = store.claimTurn({
      sessionId: session.id,
      mode: 'start',
      runId: run.id,
      hostInstanceId: 'dead-inst',
      bootTime: '100.0',
    })
    store.authorizeTurn(t1.id, 1111, 't')
    execFileSync('git', ['-C', work, 'checkout', '-b', run.branch])
    writeFileSync(join(work, 'ok.txt'), 'ok\n')
    execFileSync('git', ['-C', work, 'add', '.'])
    execFileSync('git', ['-C', work, 'commit', '-m', 'ok', '--trailer', `Run-Id: ${run.id}`])
    const sha = execFileSync('git', ['-C', work, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim()
    execFileSync('git', ['-C', work, 'push', remote, `refs/heads/${run.branch}:refs/heads/${run.branch}`])
    store.settleResult({
      token: t1.mcpToken,
      hostInstanceId: 'dead-inst',
      runId: run.id,
      branch: run.branch,
      sha,
    })
    store.endTurn(t1.id, 'completed', { exitCode: 0, endReason: 'exit', writerCheck: 'excluded' })

    // Turn 2: prepared only before host died (writer excluded)
    store.claimTurn({
      sessionId: session.id,
      mode: 'correction',
      runId: run.id,
      previousTurnId: t1.id,
      hostInstanceId: 'dead-inst',
      bootTime: '100.0',
    })

    const report = await recover({
      store,
      hostInstanceId: 'new-inst',
      bootTime: '100.0',
      check: checkRunBranch,
    })
    expect(report.runs).toEqual([{ id: run.id, outcome: 'done' }])
    expect(store.getRun(run.id)?.status).toBe('done')
    expect(store.getRun(run.id)?.resultSha).toBe(sha)
    expect(store.getRun(run.id)?.resultSource).toBe('agent')
  })

  it('running recovery twice changes nothing more', async () => {
    const dir = trackTemp(tempDir('polyclave-rec-11-'))
    const { work, remote } = makeRepo(dir)
    const baseSha = await resolveBaseSha(remote, 'main')
    const store = new Store(join(dir, 'polyclave.sqlite'))
    const session = store.createSession({ name: 's1', cwd: work })
    const { run } = store.acceptRun({
      sessionId: session.id,
      requestId: 'q1',
      instructions: 'work',
      remote,
      base: 'main',
      baseSha,
      worktreePath: (id) => join(dir, 'wt', id),
    })
    const turn = store.claimTurn({
      sessionId: session.id,
      mode: 'start',
      runId: run.id,
      hostInstanceId: 'dead-inst',
      bootTime: '100.0',
    })

    const first = await recover({
      store,
      hostInstanceId: 'new-inst',
      bootTime: '100.0',
      check: checkRunBranch,
    })
    expect(first.turns).toHaveLength(1)
    expect(first.runs).toEqual([{ id: run.id, outcome: 'retryable' }])
    const snapRun = store.getRun(run.id)!
    const snapTurn = store.getTurn(turn.id)!

    const second = await recover({
      store,
      hostInstanceId: 'new-inst',
      bootTime: '100.0',
      check: checkRunBranch,
    })
    expect(second.turns).toEqual([])
    expect(second.frozenSessions).toEqual([])
    expect(second.runs).toEqual([{ id: run.id, outcome: 'unchanged' }])
    expect(store.getRun(run.id)).toEqual(snapRun)
    expect(store.getTurn(turn.id)).toEqual(snapTurn)
  })

  it('a user allow_retry survives a restart', async () => {
    const dir = trackTemp(tempDir('polyclave-rec-12-'))
    const { work, remote } = makeRepo(dir)
    const baseSha = await resolveBaseSha(remote, 'main')
    const store = new Store(join(dir, 'polyclave.sqlite'))
    const session = store.createSession({ name: 's1', cwd: work })
    const { run } = store.acceptRun({
      sessionId: session.id,
      requestId: 'q1',
      instructions: 'work',
      remote,
      base: 'main',
      baseSha,
      worktreePath: (id) => join(dir, 'wt', id),
    })

    const turn = store.claimTurn({
      sessionId: session.id,
      mode: 'start',
      runId: run.id,
      hostInstanceId: 'dead-inst',
      bootTime: '100.0',
    })
    store.authorizeTurn(turn.id, 999999, 'Mon Jan  1 00:00:00 2020')

    // First recovery freezes session and puts run in needs_review
    await recover({
      store,
      hostInstanceId: 'inst-2',
      bootTime: '100.0',
      check: checkRunBranch,
      liveness: () => 'gone',
    })
    expect(store.getRun(run.id)?.status).toBe('needs_review')
    expect(store.getSession(session.id)?.frozenAt).toBeTruthy()

    // User resolves allow_retry and unfreezes session
    const userCodex = join(dir, 'user-codex')
    mkdirSync(userCodex, { recursive: true })
    writeFileSync(join(userCodex, 'auth.json'), '{}')
    const codexHome = prepareCodexHome(dir, userCodex)
    const mcp = await startHostMcp({ store, hostInstanceId: 'inst-2', check: checkRunBranch })
    const bus = new EventEmitter()
    const runner = new TurnRunner({
      store,
      mcpUrl: mcp.url,
      codexCommand: FAKE_CODEX,
      codexHome,
      hostInstanceId: 'inst-2',
      bus,
    })
    const runs = new Runs({ store, runner, stateDir: dir, check: checkRunBranch, bus })
    try {
      store.unfreezeSession(session.id)
      await runs.resolve(run.id, { decision: 'allow_retry', note: 'safe to retry' })
      store.addMessage({
        sessionId: session.id,
        turnId: null,
        source: 'host',
        text: 'Unfrozen by user: checked',
      })
      expect(store.getRun(run.id)?.status).toBe('interrupted')
      expect(store.getSession(session.id)?.frozenAt).toBeNull()

      // Next host start on same boot must keep allow_retry and not refreeze
      const report = await recover({
        store,
        hostInstanceId: 'inst-3',
        bootTime: '100.0',
        check: checkRunBranch,
      })
      expect(report.frozenSessions).toEqual([])
      expect(store.getRun(run.id)?.status).toBe('interrupted')
      expect(store.getSession(session.id)?.frozenAt).toBeNull()
    } finally {
      await runner.shutdown()
      await mcp.close()
    }
  })
})
