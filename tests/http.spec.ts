import { afterEach, describe, expect, it } from 'vitest'
import { execFileSync } from 'node:child_process'
import { EventEmitter } from 'node:events'
import { mkdirSync, rmSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { prepareCodexHome } from '../src/codex.ts'
import { checkRunBranch, type CommitSummary } from '../src/git-effect.ts'
import { startHttpServer, type HttpServer } from '../src/http.ts'
import { hostInstanceId } from '../src/ids.ts'
import { startHostMcp, type HostMcp } from '../src/mcp.ts'
import { TurnRunner } from '../src/runner.ts'
import { Runs } from '../src/runs.ts'
import { Store, type MessageRow, type RunRow, type SessionRow } from '../src/store.ts'
import { barrier, FAKE_CODEX, makeRepo, tempDir, waitFor } from './helpers.ts'

const temps: string[] = []
const httpServers: HttpServer[] = []
const runners: TurnRunner[] = []
const mcps: HostMcp[] = []

function trackTemp(d: string): string {
  temps.push(d)
  return d
}

afterEach(async () => {
  while (httpServers.length > 0) {
    await httpServers.pop()!.close()
  }
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

async function setupHttp(dir: string, script: Record<string, unknown>) {
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

  const http = await startHttpServer({
    store,
    runner,
    runs,
    bus,
    hostInstanceId: inst,
    port: 0,
  })
  httpServers.push(http)

  return { work, remote, store, inst, mcp, runner, runs, bus, http, logPath }
}

describe('HTTP API and SSE', () => {
  it('never exposes MCP tokens', async () => {
    const dir = trackTemp(tempDir('polyclave-http-1-'))
    const b1 = barrier(dir, 'b1')
    const { work, store, http } = await setupHttp(dir, {
      '1': [{ wait: 'b1' }, { reply: 'hi' }],
    })

    const createRes = await fetch(`${http.url}/api/sessions`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ name: 's1', cwd: work }),
    })
    expect(createRes.status).toBe(201)
    const session = (await createRes.json()) as SessionRow

    const sseAbort = new AbortController()
    const sseChunks: string[] = []
    const sseRes = await fetch(`${http.url}/api/stream`, { signal: sseAbort.signal })
    const reader = sseRes.body!.getReader()
    const pump = (async () => {
      const dec = new TextDecoder()
      try {
        for (;;) {
          const { done, value } = await reader.read()
          if (done) break
          sseChunks.push(dec.decode(value, { stream: true }))
        }
      } catch {}
    })()

    await fetch(`${http.url}/api/sessions/${session.id}/messages`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ text: 'hello' }),
    })
    await b1.waitReached()

    const token = store.runningTurn(session.id)!.mcpToken
    b1.release()

    await waitFor(() => !store.runningTurn(session.id))
    sseAbort.abort()
    await pump

    const sessionsBody = await (await fetch(`${http.url}/api/sessions`)).text()
    const messagesBody = await (await fetch(`${http.url}/api/sessions/${session.id}/messages`)).text()
    const turnsBody = await (await fetch(`${http.url}/api/sessions/${session.id}/turns`)).text()
    const runsBody = await (await fetch(`${http.url}/api/runs`)).text()
    const sseBody = sseChunks.join('')

    for (const body of [sessionsBody, messagesBody, turnsBody, runsBody, sseBody]) {
      expect(body).not.toContain(token)
      expect(body).not.toContain('mcpToken')
      expect(body).not.toContain('mcp_token')
    }
  })

  it('stores POSTed messages with source user', async () => {
    const dir = trackTemp(tempDir('polyclave-http-2-'))
    const { work, http, runner } = await setupHttp(dir, {
      '1': [{ reply: 'reply one' }],
    })

    const session = (await (
      await fetch(`${http.url}/api/sessions`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name: 's1', cwd: work }),
      })
    ).json()) as SessionRow

    const postRes = await fetch(`${http.url}/api/sessions/${session.id}/messages`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ text: 'user prompt' }),
    })
    expect(postRes.status).toBe(202)
    const { message } = (await postRes.json()) as { message: MessageRow }
    expect(message.source).toBe('user')
    expect(message.text).toBe('user prompt')

    await runner.idle()
    const msgs = (await (
      await fetch(`${http.url}/api/sessions/${session.id}/messages`)
    ).json()) as MessageRow[]
    expect(msgs.some((m) => m.id === message.id && m.source === 'user')).toBe(true)
  })

  it('replays a run request id and rejects a changed one', async () => {
    const dir = trackTemp(tempDir('polyclave-http-3-'))
    const { work, http, runs } = await setupHttp(dir, {
      '1': [
        { commit: { file: 'a.txt', content: 'a\n', trailer: true } },
        { push: true },
        { submitPushed: true },
        { reply: 'done' },
      ],
    })

    const session = (await (
      await fetch(`${http.url}/api/sessions`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name: 's1', cwd: work }),
      })
    ).json()) as SessionRow

    const res1 = await fetch(`${http.url}/api/runs`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ sessionId: session.id, requestId: 'req-1', instructions: 'do A' }),
    })
    expect(res1.status).toBe(201)
    const run1 = (await res1.json()) as RunRow

    const res2 = await fetch(`${http.url}/api/runs`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ sessionId: session.id, requestId: 'req-1', instructions: 'do A' }),
    })
    expect(res2.status).toBe(200)
    const run2 = (await res2.json()) as RunRow
    expect(run2.id).toBe(run1.id)

    const res3 = await fetch(`${http.url}/api/runs`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ sessionId: session.id, requestId: 'req-1', instructions: 'other' }),
    })
    expect(res3.status).toBe(409)
    await runs.idle()
  })

  it('returns 409 for a frozen session and accepts unfreeze with a note', async () => {
    const dir = trackTemp(tempDir('polyclave-http-4-'))
    const { work, store, http } = await setupHttp(dir, {})

    const session = (await (
      await fetch(`${http.url}/api/sessions`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name: 's1', cwd: work }),
      })
    ).json()) as SessionRow
    store.freezeSession(session.id, 'locked for test')

    const runRes = await fetch(`${http.url}/api/runs`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ sessionId: session.id, requestId: 'req-f', instructions: 'do' }),
    })
    expect(runRes.status).toBe(409)

    const unfRes = await fetch(`${http.url}/api/sessions/${session.id}/unfreeze`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ note: 'checked processes' }),
    })
    expect(unfRes.status).toBe(200)
    const unf = (await unfRes.json()) as SessionRow
    expect(unf.frozenReason).toBeNull()

    const msgs = (await (
      await fetch(`${http.url}/api/sessions/${session.id}/messages`)
    ).json()) as MessageRow[]
    expect(msgs.some((m) => m.source === 'host' && m.text === 'Unfrozen by user: checked processes')).toBe(true)
  })

  it('returns 409 when resolving a run of a frozen session', async () => {
    const dir = trackTemp(tempDir('polyclave-http-resolve-frozen-'))
    const { work, store, http } = await setupHttp(dir, {
      '1': [
        { commit: { file: 'f.txt', content: 'f\n', trailer: true } },
        { push: true },
        { reply: 'pushed, no submit' },
      ],
    })

    const session = (await (
      await fetch(`${http.url}/api/sessions`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name: 's1', cwd: work }),
      })
    ).json()) as SessionRow

    const runRes = await fetch(`${http.url}/api/runs`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ sessionId: session.id, requestId: 'req-rf', instructions: 'f' }),
    })
    expect(runRes.status).toBe(201)
    const run = (await runRes.json()) as RunRow

    const review = await waitFor(async () => {
      const res = await fetch(`${http.url}/api/runs/${run.id}`)
      const body = (await res.json()) as { run: RunRow }
      return body.run.status === 'needs_review' ? body.run : null
    })
    const sha = review.observedSha!
    expect(sha).toBeTruthy()

    store.freezeSession(session.id, 'host stopped while an agent was running')

    for (const decision of ['accept', 'fail', 'allow_retry'] as const) {
      const res = await fetch(`${http.url}/api/runs/${run.id}/resolve`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ decision, sha, note: 'while frozen' }),
      })
      expect(res.status).toBe(409)
    }
    expect(store.getRun(run.id)?.status).toBe('needs_review')

    store.unfreezeSession(session.id)
    const okRes = await fetch(`${http.url}/api/runs/${run.id}/resolve`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ decision: 'accept', sha, note: 'after unlock' }),
    })
    expect(okRes.status).toBe(200)
    const done = (await okRes.json()) as RunRow
    expect(done.status).toBe('done')
    expect(done.resultSha).toBe(sha)
  })

  it('returns 409 when unfreezing a session that is not frozen and writes nothing', async () => {
    const dir = trackTemp(tempDir('polyclave-http-unfreeze-not-frozen-'))
    const { work, http } = await setupHttp(dir, {})

    const session = (await (
      await fetch(`${http.url}/api/sessions`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name: 's1', cwd: work }),
      })
    ).json()) as SessionRow

    const res = await fetch(`${http.url}/api/sessions/${session.id}/unfreeze`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ note: 'stray note' }),
    })
    expect(res.status).toBe(409)

    const msgs = (await (
      await fetch(`${http.url}/api/sessions/${session.id}/messages`)
    ).json()) as MessageRow[]
    expect(msgs).toHaveLength(0)
  })

  it('maps busy and frozen to 409, never 500', async () => {
    const dir = trackTemp(tempDir('polyclave-http-5-'))
    const b1 = barrier(dir, 'b1')
    const { work, store, http, runs } = await setupHttp(dir, {
      '1': [
        { wait: 'b1' },
        { commit: { file: 'x.txt', content: 'x\n', trailer: true } },
        { push: true },
        { submitPushed: true },
        { reply: 'ok' },
      ],
    })

    const session = (await (
      await fetch(`${http.url}/api/sessions`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name: 's1', cwd: work }),
      })
    ).json()) as SessionRow

    const r1 = await fetch(`${http.url}/api/runs`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ sessionId: session.id, requestId: 'req-b1', instructions: 'one' }),
    })
    expect(r1.status).toBe(201)
    const run1 = (await r1.json()) as RunRow
    await b1.waitReached()

    // Second run while busy -> 409
    const r2 = await fetch(`${http.url}/api/runs`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ sessionId: session.id, requestId: 'req-b2', instructions: 'two' }),
    })
    expect(r2.status).toBe(409)

    // Retry / resolve while running -> 409
    expect(
      (await fetch(`${http.url}/api/runs/${run1.id}/retry`, { method: 'POST' })).status,
    ).toBe(409)
    expect(
      (
        await fetch(`${http.url}/api/runs/${run1.id}/resolve`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ decision: 'fail', note: 'too early' }),
        })
      ).status,
    ).toBe(409)

    b1.release()
    await runs.idle()

    // Frozen retry -> 409
    store.updateRun(run1.id, { status: 'interrupted' }, ['done'])
    store.freezeSession(session.id, 'frozen')
    expect(
      (await fetch(`${http.url}/api/runs/${run1.id}/retry`, { method: 'POST' })).status,
    ).toBe(409)
  })

  it('POST /api/runs returns before the run finishes', async () => {
    const dir = trackTemp(tempDir('polyclave-http-6-'))
    const b1 = barrier(dir, 'b1')
    const { work, store, http, runs } = await setupHttp(dir, {
      '1': [
        { wait: 'b1' },
        { commit: { file: 'a.txt', content: 'a\n', trailer: true } },
        { push: true },
        { submitPushed: true },
        { reply: 'done' },
      ],
    })

    const session = (await (
      await fetch(`${http.url}/api/sessions`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name: 's1', cwd: work }),
      })
    ).json()) as SessionRow

    const res = await fetch(`${http.url}/api/runs`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ sessionId: session.id, requestId: 'req-async', instructions: 'wait' }),
    })
    expect(res.status).toBe(201)
    const created = (await res.json()) as RunRow
    expect(['pending', 'running']).toContain(created.status)

    await b1.waitReached()
    expect(store.getRun(created.id)?.status).toBe('running')
    b1.release()
    await runs.idle()
    expect(store.getRun(created.id)?.status).toBe('done')
  })

  it('unfreeze delivers waiting messages', async () => {
    const dir = trackTemp(tempDir('polyclave-http-7-'))
    const { work, store, http, runner } = await setupHttp(dir, {
      '1': [{ reply: 'delivered after unfreeze' }],
    })

    const session = (await (
      await fetch(`${http.url}/api/sessions`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name: 's1', cwd: work }),
      })
    ).json()) as SessionRow

    store.freezeSession(session.id, 'frozen before message')
    await fetch(`${http.url}/api/sessions/${session.id}/messages`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ text: 'waiting msg' }),
    })
    expect(store.nextQueued(session.id)?.text).toBe('waiting msg')

    const unf = await fetch(`${http.url}/api/sessions/${session.id}/unfreeze`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ note: 'ok now' }),
    })
    expect(unf.status).toBe(200)

    await runner.idle()
    const msgs = (await (
      await fetch(`${http.url}/api/sessions/${session.id}/messages`)
    ).json()) as MessageRow[]
    expect(msgs.some((m) => m.source === 'agent' && m.text === 'delivered after unfreeze')).toBe(true)
    expect(store.nextQueued(session.id)).toBeUndefined()
  })

  it('streams events over SSE', async () => {
    const dir = trackTemp(tempDir('polyclave-http-8-'))
    const { work, http, runner } = await setupHttp(dir, {
      '1': [{ reply: 'streamed reply' }],
    })

    const session = (await (
      await fetch(`${http.url}/api/sessions`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name: 's1', cwd: work }),
      })
    ).json()) as SessionRow

    const sseAbort = new AbortController()
    const sseRes = await fetch(`${http.url}/api/stream`, { signal: sseAbort.signal })
    expect(sseRes.status).toBe(200)
    expect(sseRes.headers.get('content-type')).toContain('text/event-stream')

    const reader = sseRes.body!.getReader()
    let collected = ''
    const dec = new TextDecoder()
    const pump = (async () => {
      try {
        for (;;) {
          const { done, value } = await reader.read()
          if (done) break
          collected += dec.decode(value, { stream: true })
        }
      } catch {}
    })()

    await fetch(`${http.url}/api/sessions/${session.id}/messages`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ text: 'trigger sse' }),
    })
    await runner.idle()

    await waitFor(() => collected.includes('streamed reply') && collected.includes('"type":"turn"'))
    sseAbort.abort()
    await pump
    expect(collected).toContain('"type":"message"')
    expect(collected).toContain('"type":"agent"')
  })

  it('returns 404 for an unknown session', async () => {
    const dir = trackTemp(tempDir('polyclave-http-9-'))
    const { http } = await setupHttp(dir, {})

    expect((await fetch(`${http.url}/api/sessions/smissing00/messages`)).status).toBe(404)
    expect(
      (
        await fetch(`${http.url}/api/sessions/smissing00/messages`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ text: 'hi' }),
        })
      ).status,
    ).toBe(404)
    expect(
      (
        await fetch(`${http.url}/api/sessions/smissing00/unfreeze`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ note: 'hi' }),
        })
      ).status,
    ).toBe(404)
    expect(
      (
        await fetch(`${http.url}/api/sessions/smissing00/cancel`, {
          method: 'POST',
        })
      ).status,
    ).toBe(404)
    expect(
      (
        await fetch(`${http.url}/api/runs`, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({
            sessionId: 'smissing00',
            requestId: 'q1',
            instructions: 'do',
          }),
        })
      ).status,
    ).toBe(404)
  })

  it('serves the page', async () => {
    const dir = trackTemp(tempDir('polyclave-http-page-'))
    const { http } = await setupHttp(dir, {})

    const res = await fetch(`${http.url}/`)
    expect(res.status).toBe(200)
    expect(res.headers.get('content-type')).toContain('text/html')
    const html = await res.text()
    expect(html).toContain('<script src="/app.js"></script>')

    const jsRes = await fetch(`${http.url}/app.js`)
    expect(jsRes.status).toBe(200)
  })

  it('run detail includes the commit subject and changed files', async () => {
    const dir = trackTemp(tempDir('polyclave-http-commit-'))
    const { work, remote, http } = await setupHttp(dir, {
      '1': [
        { commit: { file: 'f.txt', content: 'hello\n', trailer: true } },
        { push: true },
        { reply: 'pushed, no submit' },
      ],
    })

    const session = (await (
      await fetch(`${http.url}/api/sessions`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name: 's1', cwd: work }),
      })
    ).json()) as SessionRow

    const runRes = await fetch(`${http.url}/api/runs`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ sessionId: session.id, requestId: 'req-commit', instructions: 'add f' }),
    })
    expect(runRes.status).toBe(201)
    const run = (await runRes.json()) as RunRow

    const detail = await waitFor(async () => {
      const res = await fetch(`${http.url}/api/runs/${run.id}`)
      const body = (await res.json()) as { run: RunRow; commit: CommitSummary | null }
      return body.run.status === 'needs_review' ? body : null
    })

    const sha = detail.run.observedSha!
    expect(sha).toBeTruthy()
    const expectedSubject = execFileSync('git', ['-C', remote, 'log', '-1', '--format=%s', sha], {
      encoding: 'utf8',
    }).trim()

    expect(detail.commit).not.toBeNull()
    expect(detail.commit!.sha).toBe(sha)
    expect(detail.commit!.subject).toBe(expectedSubject)
    expect(detail.commit!.files).toContainEqual({ path: 'f.txt', added: 1, deleted: 0 })
    expect(detail.commit!.filesTotal).toBe(1)
  })

  it('run detail commit is null when there is no sha', async () => {
    const dir = trackTemp(tempDir('polyclave-http-nocommit-'))
    const { work, http } = await setupHttp(dir, {
      '1': [{ exit: 1 }],
    })

    const session = (await (
      await fetch(`${http.url}/api/sessions`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ name: 's1', cwd: work }),
      })
    ).json()) as SessionRow

    const runRes = await fetch(`${http.url}/api/runs`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ sessionId: session.id, requestId: 'req-nocommit', instructions: 'fail' }),
    })
    expect(runRes.status).toBe(201)
    const run = (await runRes.json()) as RunRow
    const { status, detail } = await waitFor(async () => {
      const res = await fetch(`${http.url}/api/runs/${run.id}`)
      if (res.status !== 200) return null
      const detail = (await res.json()) as { run: RunRow; commit: CommitSummary | null }
      switch (detail.run.status) {
        case 'done':
        case 'failed':
        case 'needs_review':
        case 'interrupted':
          return { status: res.status, detail }
        default:
          return null
      }
    })
    expect(status).toBe(200)
    expect(detail.run.observedSha).toBeNull()
    expect(detail.run.resultSha).toBeNull()
    expect(detail.commit).toBeNull()
  })
})
