import { afterEach, describe, expect, it } from 'vitest'
import { execFileSync } from 'node:child_process'
import { rmSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { DatabaseSync } from 'node:sqlite'
import { checkRunBranch, resolveBaseSha } from '../src/git-effect.ts'
import { startHostMcp, type HostMcp } from '../src/mcp.ts'
import { Store } from '../src/store.ts'
import { makeRepo, tempDir } from './helpers.ts'

function countResults(dbPath: string, whereClause = ''): number {
  const db = new DatabaseSync(dbPath)
  try {
    const row = db
      .prepare(`SELECT COUNT(*) AS n FROM results ${whereClause}`)
      .get() as { n: number }
    return row.n
  } finally {
    db.close()
  }
}

const temps: string[] = []
const servers: HostMcp[] = []

function trackTemp(d: string): string {
  temps.push(d)
  return d
}

afterEach(async () => {
  while (servers.length > 0) {
    await servers.pop()!.close()
  }
  while (temps.length > 0) {
    rmSync(temps.pop()!, { recursive: true, force: true })
  }
})

async function rpc(url: string, token: string | null, method: string, params?: unknown) {
  const headers: Record<string, string> = { 'Content-Type': 'application/json' }
  if (token !== null) headers.Authorization = `Bearer ${token}`
  const res = await fetch(url, {
    method: 'POST',
    headers,
    body: JSON.stringify({ jsonrpc: '2.0', id: 1, method, params }),
  })
  const text = await res.text()
  return { status: res.status, body: text ? JSON.parse(text) : null }
}

describe('host MCP', () => {
  it('answers 401 to unknown, prepared-only, other-instance and ended tokens', async () => {
    const dir = trackTemp(tempDir('polyclave-mcp-'))
    const dbPath = join(dir, 'polyclave.sqlite')
    const store = new Store(dbPath)
    const mcp = await startHostMcp({ store, hostInstanceId: 'inst1', check: checkRunBranch })
    servers.push(mcp)

    const session = store.createSession({ name: 's1', cwd: dir })
    const t = store.claimTurn({
      sessionId: session.id,
      mode: 'chat',
      hostInstanceId: 'inst1',
      bootTime: '100.0',
    })
    const submitParams = {
      name: 'submit_result',
      arguments: { run_id: 'r11111111', branch: 'agent/r11111111', sha: 'a'.repeat(40) },
    }

    // Unknown token
    expect((await rpc(mcp.url, 'unknown-token', 'tools/call', submitParams)).status).toBe(401)
    // Prepared-only token
    expect((await rpc(mcp.url, t.mcpToken, 'tools/call', submitParams)).status).toBe(401)

    // Other-instance token
    const tOther = store.claimTurn({
      sessionId: store.createSession({ name: 's2', cwd: dir }).id,
      mode: 'chat',
      hostInstanceId: 'inst-other',
      bootTime: '100.0',
    })
    store.authorizeTurn(tOther.id, 222, 't')
    expect((await rpc(mcp.url, tOther.mcpToken, 'tools/call', submitParams)).status).toBe(401)

    // Ended token
    store.authorizeTurn(t.id, 111, 't')
    expect((await rpc(mcp.url, t.mcpToken, 'tools/list')).status).toBe(200)
    store.endTurn(t.id, 'completed', { exitCode: 0, endReason: 'exit', writerCheck: 'excluded' })
    expect((await rpc(mcp.url, t.mcpToken, 'tools/call', submitParams)).status).toBe(401)
    expect(countResults(dbPath)).toBe(0)
  })

  it('answers 503 while closed', async () => {
    const dir = trackTemp(tempDir('polyclave-mcp-503-'))
    const store = new Store(join(dir, 'polyclave.sqlite'))
    const mcp = await startHostMcp({ store, hostInstanceId: 'inst1', check: checkRunBranch })
    servers.push(mcp)

    const session = store.createSession({ name: 's1', cwd: dir })
    const t = store.claimTurn({ sessionId: session.id, mode: 'chat', hostInstanceId: 'inst1', bootTime: '100.0' })
    store.authorizeTurn(t.id, 111, 't')

    mcp.setOpen(false)
    expect((await rpc(mcp.url, t.mcpToken, 'tools/list')).status).toBe(503)
    mcp.setOpen(true)
    expect((await rpc(mcp.url, t.mcpToken, 'tools/list')).status).toBe(200)
  })

  it('rejects submit_result from a chat turn without recording anything', async () => {
    const dir = trackTemp(tempDir('polyclave-mcp-chat-'))
    const dbPath = join(dir, 'polyclave.sqlite')
    const store = new Store(dbPath)
    const mcp = await startHostMcp({ store, hostInstanceId: 'inst1', check: checkRunBranch })
    servers.push(mcp)

    const session = store.createSession({ name: 's1', cwd: dir })
    const t = store.claimTurn({ sessionId: session.id, mode: 'chat', hostInstanceId: 'inst1', bootTime: '100.0' })
    store.authorizeTurn(t.id, 111, 't')

    const res = await rpc(mcp.url, t.mcpToken, 'tools/call', {
      name: 'submit_result',
      arguments: { run_id: 'r11111111', branch: 'agent/r11111111', sha: 'a'.repeat(40) },
    })
    expect(res.status).toBe(200)
    expect(res.body.result.isError).toBe(true)
    expect(res.body.result.content[0].text).toBe('Rejected: this session has no active task')
    expect(countResults(dbPath)).toBe(0)
  })

  it('survives a git check that throws', async () => {
    const dir = trackTemp(tempDir('polyclave-mcp-err-'))
    const { work, remote } = makeRepo(dir)
    const baseSha = await resolveBaseSha(remote, 'main')
    const store = new Store(join(dir, 'polyclave.sqlite'))
    const mcp = await startHostMcp({ store, hostInstanceId: 'inst1', check: checkRunBranch })
    servers.push(mcp)

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
    const t = store.claimTurn({
      sessionId: session.id,
      mode: 'start',
      runId: run.id,
      hostInstanceId: 'inst1',
      bootTime: '100.0',
    })
    store.authorizeTurn(t.id, 111, 't')

    rmSync(remote, { recursive: true, force: true })
    const res1 = await rpc(mcp.url, t.mcpToken, 'tools/call', {
      name: 'submit_result',
      arguments: { run_id: run.id, branch: run.branch, sha: baseSha },
    })
    expect(res1.status).toBe(200)
    expect(res1.body.error).toBeDefined()

    const res2 = await rpc(mcp.url, t.mcpToken, 'ping')
    expect(res2.status).toBe(200)
    expect(res2.body.result).toEqual({})
  })

  it('accepts a result matching the bare repo', async () => {
    const dir = trackTemp(tempDir('polyclave-mcp-ok-'))
    const { work, remote } = makeRepo(dir)
    const baseSha = await resolveBaseSha(remote, 'main')
    const store = new Store(join(dir, 'polyclave.sqlite'))
    const mcp = await startHostMcp({ store, hostInstanceId: 'inst1', check: checkRunBranch })
    servers.push(mcp)

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
    const t = store.claimTurn({
      sessionId: session.id,
      mode: 'start',
      runId: run.id,
      hostInstanceId: 'inst1',
      bootTime: '100.0',
    })
    store.authorizeTurn(t.id, 111, 't')

    execFileSync('git', ['-C', work, 'checkout', '-b', run.branch])
    writeFileSync(join(work, 'ok.txt'), 'ok\n')
    execFileSync('git', ['-C', work, 'add', '.'])
    execFileSync('git', ['-C', work, 'commit', '-m', 'ok', '--trailer', `Run-Id: ${run.id}`])
    const sha = execFileSync('git', ['-C', work, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim()
    execFileSync('git', ['-C', work, 'push', remote, `refs/heads/${run.branch}:refs/heads/${run.branch}`])

    const res = await rpc(mcp.url, t.mcpToken, 'tools/call', {
      name: 'submit_result',
      arguments: { run_id: run.id, branch: run.branch, sha },
    })
    expect(res.status).toBe(200)
    expect(res.body.result.isError).toBe(false)
    expect(res.body.result.content[0].text).toBe('Accepted and stored by the host.')
    expect(store.acceptedResult(run.id)).toBeDefined()
  })

  it('rejects wrong run id, wrong branch, unknown sha and extra fields', async () => {
    const dir = trackTemp(tempDir('polyclave-mcp-rej-'))
    const { work, remote } = makeRepo(dir)
    const baseSha = await resolveBaseSha(remote, 'main')
    const store = new Store(join(dir, 'polyclave.sqlite'))
    const mcp = await startHostMcp({ store, hostInstanceId: 'inst1', check: checkRunBranch })
    servers.push(mcp)

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
    const t = store.claimTurn({
      sessionId: session.id,
      mode: 'start',
      runId: run.id,
      hostInstanceId: 'inst1',
      bootTime: '100.0',
    })
    store.authorizeTurn(t.id, 111, 't')

    execFileSync('git', ['-C', work, 'checkout', '-b', run.branch])
    writeFileSync(join(work, 'ok.txt'), 'ok\n')
    execFileSync('git', ['-C', work, 'add', '.'])
    execFileSync('git', ['-C', work, 'commit', '-m', 'ok', '--trailer', `Run-Id: ${run.id}`])
    const sha = execFileSync('git', ['-C', work, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim()
    execFileSync('git', ['-C', work, 'push', remote, `refs/heads/${run.branch}:refs/heads/${run.branch}`])

    const wrongRun = await rpc(mcp.url, t.mcpToken, 'tools/call', {
      name: 'submit_result',
      arguments: { run_id: 'r00000000', branch: run.branch, sha },
    })
    expect(wrongRun.body.result.isError).toBe(true)
    expect(wrongRun.body.result.content[0].text).toContain('is not the active task of this session')

    const wrongBranch = await rpc(mcp.url, t.mcpToken, 'tools/call', {
      name: 'submit_result',
      arguments: { run_id: run.id, branch: 'agent/other', sha },
    })
    expect(wrongBranch.body.result.isError).toBe(true)
    expect(wrongBranch.body.result.content[0].text).toContain(`branch must be ${run.branch}`)

    const wrongSha = await rpc(mcp.url, t.mcpToken, 'tools/call', {
      name: 'submit_result',
      arguments: { run_id: run.id, branch: run.branch, sha: '0'.repeat(40) },
    })
    expect(wrongSha.body.result.isError).toBe(true)
    expect(wrongSha.body.result.content[0].text).toContain('Rejected:')

    const extraField = await rpc(mcp.url, t.mcpToken, 'tools/call', {
      name: 'submit_result',
      arguments: { run_id: run.id, branch: run.branch, sha, extra: 1 },
    })
    expect(extraField.body.result.isError).toBe(true)
    expect(extraField.body.result.content[0].text).toContain('Rejected:')
    expect(store.acceptedResult(run.id)).toBeUndefined()
  })

  it('cancel during the git check wins', async () => {
    const dir = trackTemp(tempDir('polyclave-mcp-cancel-'))
    const { work, remote } = makeRepo(dir)
    const baseSha = await resolveBaseSha(remote, 'main')
    const store = new Store(join(dir, 'polyclave.sqlite'))

    let enteredCheck!: () => void
    const entered = new Promise<void>((r) => {
      enteredCheck = r
    })
    let releaseCheck!: () => void
    const released = new Promise<void>((r) => {
      releaseCheck = r
    })

    const mcp = await startHostMcp({
      store,
      hostInstanceId: 'inst1',
      async check(i) {
        enteredCheck()
        await released
        return checkRunBranch(i)
      },
    })
    servers.push(mcp)

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
    const t = store.claimTurn({
      sessionId: session.id,
      mode: 'start',
      runId: run.id,
      hostInstanceId: 'inst1',
      bootTime: '100.0',
    })
    store.authorizeTurn(t.id, 111, 't')

    execFileSync('git', ['-C', work, 'checkout', '-b', run.branch])
    writeFileSync(join(work, 'ok.txt'), 'ok\n')
    execFileSync('git', ['-C', work, 'add', '.'])
    execFileSync('git', ['-C', work, 'commit', '-m', 'ok', '--trailer', `Run-Id: ${run.id}`])
    const sha = execFileSync('git', ['-C', work, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim()
    execFileSync('git', ['-C', work, 'push', remote, `refs/heads/${run.branch}:refs/heads/${run.branch}`])

    const callPromise = rpc(mcp.url, t.mcpToken, 'tools/call', {
      name: 'submit_result',
      arguments: { run_id: run.id, branch: run.branch, sha },
    })
    await entered
    store.endTurn(t.id, 'cancelled', {
      exitCode: null,
      endReason: 'cancel',
      writerCheck: 'possible',
      freezeReason: 'cancelled',
    })
    releaseCheck()

    const res = await callPromise
    expect(res.status).toBe(401)
    expect(store.acceptedResult(run.id)).toBeUndefined()
  })

  it('two identical concurrent submissions give one acceptance', async () => {
    const dir = trackTemp(tempDir('polyclave-mcp-conc-'))
    const { work, remote } = makeRepo(dir)
    const baseSha = await resolveBaseSha(remote, 'main')
    const dbPath = join(dir, 'polyclave.sqlite')
    const store = new Store(dbPath)
    const mcp = await startHostMcp({ store, hostInstanceId: 'inst1', check: checkRunBranch })
    servers.push(mcp)

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
    const t = store.claimTurn({
      sessionId: session.id,
      mode: 'start',
      runId: run.id,
      hostInstanceId: 'inst1',
      bootTime: '100.0',
    })
    store.authorizeTurn(t.id, 111, 't')

    execFileSync('git', ['-C', work, 'checkout', '-b', run.branch])
    writeFileSync(join(work, 'ok.txt'), 'ok\n')
    execFileSync('git', ['-C', work, 'add', '.'])
    execFileSync('git', ['-C', work, 'commit', '-m', 'ok', '--trailer', `Run-Id: ${run.id}`])
    const sha = execFileSync('git', ['-C', work, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim()
    execFileSync('git', ['-C', work, 'push', remote, `refs/heads/${run.branch}:refs/heads/${run.branch}`])

    const [a, b] = await Promise.all([
      rpc(mcp.url, t.mcpToken, 'tools/call', {
        name: 'submit_result',
        arguments: { run_id: run.id, branch: run.branch, sha },
      }),
      rpc(mcp.url, t.mcpToken, 'tools/call', {
        name: 'submit_result',
        arguments: { run_id: run.id, branch: run.branch, sha },
      }),
    ])

    const texts = [a.body.result.content[0].text, b.body.result.content[0].text].sort()
    expect(texts).toEqual(['Accepted and stored by the host.', 'Already accepted.'])
    expect(store.acceptedResult(run.id)).toBeDefined()
    expect(countResults(dbPath, 'WHERE accepted = 1')).toBe(1)
  })

  it('a different sha after acceptance is rejected and the first stays', async () => {
    const dir = trackTemp(tempDir('polyclave-mcp-diff-'))
    const { work, remote } = makeRepo(dir)
    const baseSha = await resolveBaseSha(remote, 'main')
    const store = new Store(join(dir, 'polyclave.sqlite'))
    const mcp = await startHostMcp({ store, hostInstanceId: 'inst1', check: checkRunBranch })
    servers.push(mcp)

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
    const t = store.claimTurn({
      sessionId: session.id,
      mode: 'start',
      runId: run.id,
      hostInstanceId: 'inst1',
      bootTime: '100.0',
    })
    store.authorizeTurn(t.id, 111, 't')

    execFileSync('git', ['-C', work, 'checkout', '-b', run.branch])
    writeFileSync(join(work, '1.txt'), '1\n')
    execFileSync('git', ['-C', work, 'add', '.'])
    execFileSync('git', ['-C', work, 'commit', '-m', 'c1', '--trailer', `Run-Id: ${run.id}`])
    const sha1 = execFileSync('git', ['-C', work, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim()
    execFileSync('git', ['-C', work, 'push', remote, `refs/heads/${run.branch}:refs/heads/${run.branch}`])

    const first = await rpc(mcp.url, t.mcpToken, 'tools/call', {
      name: 'submit_result',
      arguments: { run_id: run.id, branch: run.branch, sha: sha1 },
    })
    expect(first.body.result.content[0].text).toBe('Accepted and stored by the host.')

    writeFileSync(join(work, '2.txt'), '2\n')
    execFileSync('git', ['-C', work, 'add', '.'])
    execFileSync('git', ['-C', work, 'commit', '-m', 'c2', '--trailer', `Run-Id: ${run.id}`])
    const sha2 = execFileSync('git', ['-C', work, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim()
    execFileSync('git', ['-C', work, 'push', remote, `refs/heads/${run.branch}:refs/heads/${run.branch}`])

    const second = await rpc(mcp.url, t.mcpToken, 'tools/call', {
      name: 'submit_result',
      arguments: { run_id: run.id, branch: run.branch, sha: sha2 },
    })
    expect(second.body.result.isError).toBe(true)
    expect(second.body.result.content[0].text).toBe(
      'Rejected: a different result was already accepted for this task',
    )
    expect(JSON.parse(store.acceptedResult(run.id)!.payload).sha).toBe(sha1)
  })
})
