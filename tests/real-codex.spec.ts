import { execFileSync, spawn, type ChildProcess } from 'node:child_process'
import { once } from 'node:events'
import { existsSync, mkdirSync, realpathSync, rmSync } from 'node:fs'
import { homedir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { afterEach, describe, expect, it } from 'vitest'
import { Store } from '../src/store.ts'
import { makeRepo, tempDir, waitFor } from './helpers.ts'

const MAIN_PATH = fileURLToPath(new URL('../src/main.ts', import.meta.url))
const REAL_CODEX_ENABLED = process.env.POLYCLAVE_REAL_CODEX === '1'

const temps: string[] = []
const children: Array<{ child: ChildProcess; exited: Promise<unknown> }> = []

afterEach(async () => {
  while (children.length > 0) {
    const item = children.pop()!
    if (item.child.exitCode === null && item.child.signalCode === null) {
      item.child.kill('SIGTERM')
      const outcome = await Promise.race([
        item.exited.then(() => 'exited' as const),
        new Promise<'timeout'>((resolve) => setTimeout(() => resolve('timeout'), 5000)),
      ])
      if (outcome === 'timeout') {
        item.child.kill('SIGKILL')
        await item.exited
      }
    }
  }
  while (temps.length > 0) {
    rmSync(temps.pop()!, { recursive: true, force: true })
  }
})

describe.skipIf(!REAL_CODEX_ENABLED)('real codex acceptance', () => {
  it('real codex: chat, resume, a task delivered and verified', async () => {
    const dir = tempDir('polyclave-real-codex-')
    temps.push(dir)

    const repoDir = join(dir, 'repo')
    mkdirSync(repoDir, { recursive: true })
    const { work, remote } = makeRepo(repoDir)
    rmSync(join(remote, 'hooks', 'post-receive'), { force: true })

    const stateDir = join(dir, 'state')
    mkdirSync(stateDir, { recursive: true })

    const child = spawn(
      process.execPath,
      [MAIN_PATH, '--state-dir', stateDir, '--port', '0'],
      {
        stdio: ['ignore', 'pipe', 'pipe'],
      },
    )
    const exited = once(child, 'exit')
    children.push({ child, exited })

    let stdout = ''
    let stderr = ''
    child.stdout?.setEncoding('utf8')
    child.stderr?.setEncoding('utf8')
    child.stdout?.on('data', (chunk: string) => {
      stdout += chunk
    })
    child.stderr?.on('data', (chunk: string) => {
      stderr += chunk
    })

    let url = ''
    await waitFor(() => {
      const m = /polyclave listening on (http:\/\/127\.0\.0\.1:\d+)/.exec(stdout)
      if (m) {
        url = m[1]!
        return true
      }
      if (child.exitCode !== null || child.signalCode !== null) {
        throw new Error(`host exited early (${child.exitCode}/${child.signalCode}): ${stderr}`)
      }
      return false
    }, 15000)

    const userSkills = join(homedir(), '.codex', 'skills')
    if (existsSync(userSkills)) {
      const linkedSkills = join(realpathSync(stateDir), 'codex-home', 'skills')
      expect(existsSync(linkedSkills)).toBe(true)
      expect(realpathSync(linkedSkills)).toBe(realpathSync(userSkills))
    }

    const createSess = await fetch(`${url}/api/sessions`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ name: 'real', cwd: work }),
    })
    expect(createSess.status).toBe(201)
    const session = (await createSess.json()) as { id: string }

    const chat1 = await fetch(`${url}/api/sessions/${session.id}/messages`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ text: 'Reply with only the word PONG and nothing else.' }),
    })
    expect(chat1.status).toBe(202)

    let thread1: string | null = null
    await waitFor(async () => {
      const r = await fetch(`${url}/api/sessions`)
      const rows = (await r.json()) as Array<{
        id: string
        busy: boolean
        lastTurnStatus: string | null
        agentSessionId: string | null
        frozenReason: string | null
      }>
      const row = rows.find((s) => s.id === session.id)
      if (row?.frozenReason) {
        throw new Error(`session frozen unexpectedly after chat 1: ${row.frozenReason}`)
      }
      if (row && !row.busy && row.lastTurnStatus !== null && row.lastTurnStatus !== 'running') {
        expect(row.lastTurnStatus).toBe('completed')
        expect(row.agentSessionId).toBeTruthy()
        thread1 = row.agentSessionId
        return true
      }
      return false
    }, 120000)

    const chat2 = await fetch(`${url}/api/sessions/${session.id}/messages`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({
        text: 'What single uppercase word did I ask you to reply with in my previous message? Reply with only that word.',
      }),
    })
    expect(chat2.status).toBe(202)

    await waitFor(async () => {
      const r = await fetch(`${url}/api/sessions`)
      const rows = (await r.json()) as Array<{
        id: string
        busy: boolean
        lastTurnStatus: string | null
        agentSessionId: string | null
        frozenReason: string | null
      }>
      const row = rows.find((s) => s.id === session.id)
      if (row?.frozenReason) {
        throw new Error(`session frozen unexpectedly after chat 2: ${row.frozenReason}`)
      }
      if (row && !row.busy) {
        const turnsRes = await fetch(`${url}/api/sessions/${session.id}/turns`)
        const turns = (await turnsRes.json()) as Array<{ status: string }>
        if (turns.length >= 2 && turns[1]!.status !== 'running') {
          expect(turns[1]!.status).toBe('completed')
          expect(row.agentSessionId).toBe(thread1)
          return true
        }
      }
      return false
    }, 120000)

    const msgsRes = await fetch(`${url}/api/sessions/${session.id}/messages`)
    const msgs = (await msgsRes.json()) as Array<{ source: string; text: string }>
    const agentMsgs = msgs.filter((m) => m.source === 'agent')
    expect(agentMsgs.length).toBeGreaterThanOrEqual(2)
    expect(agentMsgs.at(-1)!.text.toUpperCase()).toContain('PONG')

    const runRes = await fetch(`${url}/api/runs`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({
        sessionId: session.id,
        requestId: 'real-codex-hello-1',
        instructions:
          'Add a file hello.txt at the repo root containing exactly hi with no trailing newline (for example using printf "hi" > hello.txt).',
      }),
    })
    expect(runRes.status).toBe(201)
    const createdRun = (await runRes.json()) as { id: string }

    let doneRun: { id: string; status: string; resultSha: string | null; detail: string | null } | null =
      null
    await waitFor(async () => {
      const r = await fetch(`${url}/api/runs`)
      const list = (await r.json()) as Array<{
        id: string
        status: string
        resultSha: string | null
        detail: string | null
      }>
      const found = list.find((row) => row.id === createdRun.id)
      if (!found) return false
      if (found.status === 'done') {
        doneRun = found
        return true
      }
      if (found.status === 'interrupted' || found.status === 'failed') {
        throw new Error(`run ${found.id} ended in ${found.status}: ${found.detail ?? ''}`)
      }
      return false
    }, 180000)

    expect(doneRun!.resultSha).toMatch(/^[0-9a-f]{40}$/)
    const fileContent = execFileSync(
      'git',
      ['-C', remote, 'show', `${doneRun!.resultSha!}:hello.txt`],
      { encoding: 'utf8' },
    )
    expect(fileContent).toBe('hi')

    child.kill('SIGTERM')
    await exited

    const store = new Store(join(realpathSync(stateDir), 'polyclave.sqlite'))
    try {
      const turns = store.listSessionTurns(session.id)
      expect(turns.length).toBeGreaterThanOrEqual(3)
      const mcpServersCalled: Array<string | undefined> = []
      for (const t of turns) {
        for (const evRow of store.listTurnEvents(t.id)) {
          const parsed = JSON.parse(evRow.json) as {
            item?: { type?: string; server?: string }
          }
          if (parsed?.item?.type === 'mcp_tool_call') {
            mcpServersCalled.push(parsed.item.server)
          }
        }
      }
      expect(mcpServersCalled.length).toBeGreaterThanOrEqual(1)
      expect(mcpServersCalled.every((s) => s === 'host')).toBe(true)
    } finally {
      store.close()
    }
  }, 300000)
})
