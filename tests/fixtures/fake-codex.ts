import { execFileSync, spawn } from 'node:child_process'
import { randomBytes } from 'node:crypto'
import {
  appendFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  rmdirSync,
  writeFileSync,
} from 'node:fs'
import { dirname, isAbsolute, join } from 'node:path'

type Step =
  | { reply: string }
  | { sleepMs: number }
  | { wait: string }
  | { exit: number; stderr?: string }
  | { commit: { file: string; content: string; trailer: boolean } }
  | { push: true }
  | { submitPushed: true }
  | { submitRaw: Record<string, unknown> }
  | { detachedWriter: { barrier: string; file: string } }
  | { groupMember: { pidFile: string } }

function delay(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms))
}

function claimTurnNumber(lockBase: string): number {
  const lockDir = `${lockBase}.lock`
  const counterFile = `${lockBase}.counter`
  for (;;) {
    try {
      mkdirSync(lockDir)
      break
    } catch (err) {
      if ((err as { code?: string }).code !== 'EEXIST') throw err
      Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 5)
    }
  }
  try {
    let cur = 0
    if (existsSync(counterFile)) {
      cur = Number(readFileSync(counterFile, 'utf8').trim()) || 0
    }
    const next = cur + 1
    writeFileSync(counterFile, String(next))
    return next
  } finally {
    try {
      rmdirSync(lockDir)
    } catch {}
  }
}

function logEvent(logPath: string | undefined, entry: Record<string, unknown>): void {
  if (!logPath) return
  appendFileSync(logPath, JSON.stringify(entry) + '\n')
}

function parsePushCommand(prompt: string): { remote: string; refspec: string } | null {
  const idx = prompt.indexOf('git push ')
  if (idx < 0) return null
  let rest = prompt.slice(idx + 'git push '.length).trimStart()
  let remote = ''
  if (rest.startsWith("'")) {
    let i = 1
    while (i < rest.length) {
      if (rest.slice(i, i + 4) === "'\\''") {
        remote += "'"
        i += 4
      } else if (rest[i] === "'") {
        i += 1
        break
      } else {
        remote += rest[i]
        i += 1
      }
    }
    rest = rest.slice(i).trimStart()
  } else if (rest.startsWith('"')) {
    let i = 1
    while (i < rest.length) {
      if (rest[i] === '\\' && i + 1 < rest.length) {
        remote += rest[i + 1]
        i += 2
      } else if (rest[i] === '"') {
        i += 1
        break
      } else {
        remote += rest[i]
        i += 1
      }
    }
    rest = rest.slice(i).trimStart()
  } else {
    const m = /^(\S+)\s+(.*)$/s.exec(rest)
    if (!m) return null
    remote = m[1]!
    rest = m[2]!.trimStart()
  }
  const refMatch = /^(refs\/heads\/[^\s]+:refs\/heads\/[^\s]+)/.exec(rest)
  if (!refMatch) return null
  return { remote, refspec: refMatch[1]! }
}

function resolveBarrierDir(name: string, logPath: string | undefined, scriptPath: string | undefined): string {
  if (isAbsolute(name)) return name
  const base =
    process.env.FAKE_CODEX_BARRIER_DIR ??
    (logPath ? dirname(logPath) : scriptPath && !scriptPath.startsWith('{') ? dirname(scriptPath) : process.cwd())
  return join(base, name)
}

async function waitOnBarrier(bDir: string): Promise<void> {
  mkdirSync(bDir, { recursive: true })
  writeFileSync(join(bDir, 'reached'), '1')
  const releaseFile = join(bDir, 'release')
  while (!existsSync(releaseFile)) {
    await delay(20)
  }
}

async function callMcpSubmit(mcpUrl: string, args: unknown): Promise<{ ok: boolean; text: string }> {
  const token = process.env.POLYCLAVE_MCP_TOKEN ?? ''
  const headers: Record<string, string> = {
    'Content-Type': 'application/json',
    Accept: 'application/json, text/event-stream',
  }
  if (token) headers.Authorization = `Bearer ${token}`

  const initRes = await fetch(mcpUrl, {
    method: 'POST',
    headers,
    body: JSON.stringify({
      jsonrpc: '2.0',
      id: 1,
      method: 'initialize',
      params: {
        protocolVersion: '2025-06-18',
        capabilities: {},
        clientInfo: { name: 'fake-codex', version: '1.0.0' },
      },
    }),
  })
  if (!initRes.ok) {
    return { ok: false, text: `HTTP ${initRes.status}` }
  }

  const callRes = await fetch(mcpUrl, {
    method: 'POST',
    headers,
    body: JSON.stringify({
      jsonrpc: '2.0',
      id: 2,
      method: 'tools/call',
      params: { name: 'submit_result', arguments: args },
    }),
  })
  if (!callRes.ok) {
    return { ok: false, text: `HTTP ${callRes.status}` }
  }
  const json = (await callRes.json()) as {
    result?: { isError?: boolean; content?: { text?: string }[] }
    error?: { message?: string }
  }
  if (json.error) {
    return { ok: false, text: json.error.message ?? 'rpc error' }
  }
  const text = json.result?.content?.[0]?.text ?? ''
  return { ok: !json.result?.isError, text }
}

function emit(obj: Record<string, unknown>): void {
  process.stdout.write(JSON.stringify(obj) + '\n')
}

async function main(): Promise<void> {
  const argv = process.argv.slice(2)
  const resumed = argv[0] === 'exec' && argv[1] === 'resume'
  const dashIdx = argv.indexOf('--')
  let threadId: string
  let prompt: string
  if (dashIdx >= 0) {
    if (resumed) {
      threadId = argv[dashIdx + 1] ?? ''
      prompt = argv[dashIdx + 2] ?? ''
    } else {
      threadId = `fake-thread-${randomBytes(4).toString('hex')}`
      prompt = argv[dashIdx + 1] ?? ''
    }
  } else if (resumed) {
    threadId = argv.at(-2) ?? ''
    prompt = argv.at(-1) ?? ''
  } else {
    threadId = `fake-thread-${randomBytes(4).toString('hex')}`
    prompt = argv.at(-1) ?? ''
  }

  let mcpUrl = ''
  for (let i = 0; i < argv.length - 1; i++) {
    if (argv[i] === '-c' && argv[i + 1]?.startsWith('mcp_servers.host.url=')) {
      const raw = argv[i + 1]!.slice('mcp_servers.host.url='.length)
      try {
        mcpUrl = JSON.parse(raw) as string
      } catch {
        mcpUrl = raw.replace(/^"|"$/g, '')
      }
    }
  }

  const scriptEnv = process.env.FAKE_CODEX_SCRIPT
  const logPath = process.env.FAKE_CODEX_LOG
  const lockBase =
    scriptEnv && !scriptEnv.trim().startsWith('{') && !scriptEnv.trim().startsWith('[')
      ? scriptEnv
      : join(logPath ? dirname(logPath) : process.cwd(), 'fake-codex')

  const turn = claimTurnNumber(lockBase)

  logEvent(logPath, {
    event: 'prompt_started',
    turn,
    resumed,
    threadId,
    prompt,
    cwd: process.cwd(),
  })

  emit({ type: 'thread.started', thread_id: threadId })
  emit({ type: 'turn.started' })

  let scriptMap: Record<string, Step[]> = {}
  if (scriptEnv) {
    const raw =
      scriptEnv.trim().startsWith('{') || scriptEnv.trim().startsWith('[')
        ? scriptEnv
        : readFileSync(scriptEnv, 'utf8')
    const parsed = JSON.parse(raw) as Step[] | Record<string, Step[]>
    if (Array.isArray(parsed)) {
      scriptMap = { [String(turn)]: parsed }
    } else {
      scriptMap = parsed
    }
  }

  const steps = scriptMap[String(turn)] ?? scriptMap.default ?? [{ reply: 'ok' }]
  const runMatch = /\bTask (r[0-9a-f]{8})\b/.exec(prompt)
  const runId = runMatch?.[1]

  for (const step of steps) {
    if ('sleepMs' in step) {
      await delay(step.sleepMs)
    } else if ('wait' in step) {
      await waitOnBarrier(resolveBarrierDir(step.wait, logPath, scriptEnv))
    } else if ('exit' in step) {
      if (step.stderr) process.stderr.write(step.stderr)
      process.exit(step.exit)
    } else if ('reply' in step) {
      emit({
        type: 'item.completed',
        item: { type: 'agent_message', text: step.reply },
      })
    } else if ('commit' in step) {
      const target = join(process.cwd(), step.commit.file)
      mkdirSync(dirname(target), { recursive: true })
      writeFileSync(target, step.commit.content)
      execFileSync('git', ['add', step.commit.file], { cwd: process.cwd() })
      const args = ['commit', '-m', `work for ${runId ?? 'task'}`]
      if (step.commit.trailer && runId) {
        args.push('--trailer', `Run-Id: ${runId}`)
      }
      execFileSync('git', args, { cwd: process.cwd() })
      emit({
        type: 'item.started',
        item: { type: 'command_execution', command: `git ${args.join(' ')}` },
      })
      emit({
        type: 'item.completed',
        item: { type: 'command_execution', command: `git ${args.join(' ')}`, exit_code: 0 },
      })
    } else if ('push' in step) {
      logEvent(logPath, { event: 'push_attempt', turn })
      const parsedPush = parsePushCommand(prompt)
      if (!parsedPush) throw new Error('no push command found in prompt')
      emit({
        type: 'item.started',
        item: { type: 'command_execution', command: `git push ${parsedPush.remote} ${parsedPush.refspec}` },
      })
      execFileSync('git', ['push', parsedPush.remote, parsedPush.refspec], {
        cwd: process.cwd(),
      })
      emit({
        type: 'item.completed',
        item: {
          type: 'command_execution',
          command: `git push ${parsedPush.remote} ${parsedPush.refspec}`,
          exit_code: 0,
        },
      })
    } else if ('submitPushed' in step) {
      const branch = execFileSync('git', ['rev-parse', '--abbrev-ref', 'HEAD'], {
        cwd: process.cwd(),
        encoding: 'utf8',
      }).trim()
      const sha = execFileSync('git', ['rev-parse', 'HEAD'], {
        cwd: process.cwd(),
        encoding: 'utf8',
      }).trim()
      const args = { run_id: runId ?? '', branch, sha }
      emit({
        type: 'item.started',
        item: { type: 'mcp_tool_call', server: 'host', tool: 'submit_result', arguments: args },
      })
      const res = await callMcpSubmit(mcpUrl, args)
      emit({
        type: 'item.completed',
        item: {
          type: 'mcp_tool_call',
          server: 'host',
          tool: 'submit_result',
          status: res.ok ? 'completed' : 'failed',
          error: res.ok ? null : res.text,
        },
      })
    } else if ('submitRaw' in step) {
      emit({
        type: 'item.started',
        item: {
          type: 'mcp_tool_call',
          server: 'host',
          tool: 'submit_result',
          arguments: step.submitRaw,
        },
      })
      const res = await callMcpSubmit(mcpUrl, step.submitRaw)
      emit({
        type: 'item.completed',
        item: {
          type: 'mcp_tool_call',
          server: 'host',
          tool: 'submit_result',
          status: res.ok ? 'completed' : 'failed',
          error: res.ok ? null : res.text,
        },
      })
    } else if ('detachedWriter' in step) {
      const bDir = resolveBarrierDir(step.detachedWriter.barrier, logPath, scriptEnv)
      const parsedPush = parsePushCommand(prompt)
      const file = step.detachedWriter.file
      const childCode = `
        const { execFileSync } = require('node:child_process');
        const { existsSync, mkdirSync, writeFileSync } = require('node:fs');
        const { dirname, join } = require('node:path');
        const bDir = ${JSON.stringify(bDir)};
        const file = ${JSON.stringify(file)};
        const cwd = ${JSON.stringify(process.cwd())};
        const runId = ${JSON.stringify(runId ?? '')};
        const parsedPush = ${JSON.stringify(parsedPush)};
        mkdirSync(bDir, { recursive: true });
        writeFileSync(join(bDir, 'reached'), '1');
        const sab = new Int32Array(new SharedArrayBuffer(4));
        while (!existsSync(join(bDir, 'release'))) {
          Atomics.wait(sab, 0, 0, 20);
        }
        const target = join(cwd, file);
        mkdirSync(dirname(target), { recursive: true });
        writeFileSync(target, 'detached write\\n');
        try {
          execFileSync('git', ['add', file], { cwd });
          const args = ['commit', '-m', 'detached write'];
          if (runId) args.push('--trailer', 'Run-Id: ' + runId);
          execFileSync('git', args, { cwd });
          if (parsedPush) {
            execFileSync('git', ['push', parsedPush.remote, parsedPush.refspec], { cwd });
          }
        } catch {}
      `
      const child = spawn(process.execPath, ['-e', childCode], {
        cwd: process.cwd(),
        detached: true,
        stdio: 'ignore',
      })
      child.unref()
      logEvent(logPath, { event: 'detached_writer', turn, pid: child.pid })
    } else if ('groupMember' in step) {
      const child = spawn('sleep', ['30'], {
        cwd: process.cwd(),
        detached: false,
        stdio: 'ignore',
      })
      child.unref()
      writeFileSync(step.groupMember.pidFile, String(child.pid))
    }
  }

  emit({ type: 'turn.completed', usage: {} })
  logEvent(logPath, { event: 'turn_end', turn })
}

main().catch((err) => {
  process.stderr.write(String(err) + '\n')
  process.exit(1)
})
