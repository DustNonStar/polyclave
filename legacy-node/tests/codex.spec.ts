import { afterEach, describe, expect, it } from 'vitest'
import { execFileSync } from 'node:child_process'
import {
  lstatSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readlinkSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from 'node:fs'
import { tmpdir } from 'node:os'
import { isAbsolute, join } from 'node:path'
import {
  buildCodexArgs,
  gitCommonDir,
  parseCodexLine,
  prepareCodexHome,
} from '../src/codex.ts'

const temps: string[] = []
function makeTemp(prefix: string): string {
  const d = realpathSync(mkdtempSync(join(tmpdir(), prefix)))
  temps.push(d)
  return d
}

afterEach(() => {
  while (temps.length > 0) {
    rmSync(temps.pop()!, { recursive: true, force: true })
  }
})

describe('codex helpers', () => {
  it('starts a new session with exec and the fixed safety flags', () => {
    const a = buildCodexArgs({
      agentSessionId: null,
      prompt: 'hi',
      mcpUrl: 'http://127.0.0.1:5/mcp',
      writableRoots: ['/repo/.git', '/r.git'],
    })
    expect(a[0]).toBe('exec')
    expect(a).toEqual(
      expect.arrayContaining(['--json', '--skip-git-repo-check', '--ignore-user-config']),
    )
    for (const v of [
      'approval_policy="never"',
      'sandbox_mode="workspace-write"',
      'sandbox_workspace_write.network_access=false',
      'sandbox_workspace_write.writable_roots=["/repo/.git","/r.git"]',
      'mcp_servers.host.url="http://127.0.0.1:5/mcp"',
      'mcp_servers.host.bearer_token_env_var="POLYCLAVE_MCP_TOKEN"',
      'mcp_servers.host.default_tools_approval_mode="approve"',
    ]) {
      expect(a).toContain(v)
    }
    expect(a.slice(-2)).toEqual(['--', 'hi'])
  })

  it('resumes with exec resume <id> <prompt>', () => {
    const a = buildCodexArgs({
      agentSessionId: '01a2',
      prompt: 'hi',
      mcpUrl: 'http://127.0.0.1:5/mcp',
      writableRoots: ['/repo/.git'],
    })
    expect(a.slice(0, 2)).toEqual(['exec', 'resume'])
    expect(a.slice(-3)).toEqual(['--', '01a2', 'hi'])
  })

  it('puts -- before a prompt that starts with a dash', () => {
    const a = buildCodexArgs({
      agentSessionId: null,
      prompt: '- fix login',
      mcpUrl: 'http://127.0.0.1:5/mcp',
      writableRoots: [],
    })
    expect(a.slice(-2)).toEqual(['--', '- fix login'])
  })

  it('gitCommonDir is absolute and has no symlink component', () => {
    const root = makeTemp('polyclave-gcd-')
    const repo = join(root, 'repo')
    mkdirSync(repo)
    execFileSync('git', ['init', '-b', 'main', repo])
    const common = gitCommonDir(repo)
    expect(isAbsolute(common)).toBe(true)
    expect(common).toBe(realpathSync(common))
    expect(common).toBe(join(repo, '.git'))
  })

  it('keeps paths with spaces and quotes intact', () => {
    const specialRoot = '/a b/"c"'
    const prompt = 'say "hello" and \'bye\''
    const a = buildCodexArgs({
      agentSessionId: null,
      prompt,
      mcpUrl: 'http://127.0.0.1:5/mcp',
      writableRoots: [specialRoot],
    })
    expect(a).toContain(
      `sandbox_workspace_write.writable_roots=${JSON.stringify([specialRoot])}`,
    )
    expect(a.at(-1)).toBe(prompt)
  })

  it('parses thread.started, agent_message, mcp_tool_call, command_execution, turn.completed, turn.failed', () => {
    const tStarted = parseCodexLine(
      JSON.stringify({ type: 'thread.started', thread_id: 'th-123' }),
    )
    expect(tStarted?.threadId).toBe('th-123')
    expect(tStarted?.type).toBe('thread.started')

    const msg = parseCodexLine(
      JSON.stringify({
        type: 'item.completed',
        item: { type: 'agent_message', text: 'all done' },
      }),
    )
    expect(msg?.event).toEqual({ kind: 'message.done', text: 'all done' })

    const mcpCall = parseCodexLine(
      JSON.stringify({
        type: 'item.started',
        item: {
          type: 'mcp_tool_call',
          server: 'host',
          tool: 'submit_result',
          arguments: { run_id: 'r1' },
        },
      }),
    )
    expect(mcpCall?.event).toEqual({
      kind: 'tool.call',
      name: 'submit_result',
      input: { run_id: 'r1' },
    })

    const mcpDone = parseCodexLine(
      JSON.stringify({
        type: 'item.completed',
        item: {
          type: 'mcp_tool_call',
          server: 'host',
          tool: 'submit_result',
          status: 'completed',
        },
      }),
    )
    expect(mcpDone?.event).toEqual({
      kind: 'tool.result',
      name: 'submit_result',
      ok: true,
    })

    const cmdStart = parseCodexLine(
      JSON.stringify({
        type: 'item.started',
        item: { type: 'command_execution', command: 'git status' },
      }),
    )
    expect(cmdStart?.event).toEqual({
      kind: 'tool.call',
      name: 'shell',
      input: { command: 'git status' },
    })

    const cmdOk = parseCodexLine(
      JSON.stringify({
        type: 'item.completed',
        item: { type: 'command_execution', command: 'git status', exit_code: 0 },
      }),
    )
    expect(cmdOk?.event).toEqual({
      kind: 'tool.result',
      name: 'shell',
      ok: true,
    })

    const cmdFail = parseCodexLine(
      JSON.stringify({
        type: 'item.completed',
        item: { type: 'command_execution', command: 'false', exit_code: 1 },
      }),
    )
    expect(cmdFail?.event).toEqual({
      kind: 'tool.result',
      name: 'shell',
      ok: false,
    })

    const turnDone = parseCodexLine(
      JSON.stringify({ type: 'turn.completed', usage: { input_tokens: 10 } }),
    )
    expect(turnDone?.event).toEqual({
      kind: 'turn.done',
      usage: { input_tokens: 10 },
    })

    const turnFailed = parseCodexLine(
      JSON.stringify({ type: 'turn.failed', error: { message: 'boom' } }),
    )
    expect(turnFailed?.event).toEqual({
      kind: 'error',
      message: 'boom',
    })
  })

  it('returns null for blank or non-JSON lines', () => {
    expect(parseCodexLine('')).toBeNull()
    expect(parseCodexLine('   \n')).toBeNull()
    expect(parseCodexLine('warn: x')).toBeNull()
  })

  it('prepares a codex home with auth.json and skills symlinks and nothing else', () => {
    const root = makeTemp('polyclave-chome-')
    const userHome = join(root, 'user-codex')
    const state1 = join(root, 'state1')
    mkdirSync(userHome)
    mkdirSync(state1)
    writeFileSync(join(userHome, 'auth.json'), '{"token":"secret"}')
    mkdirSync(join(userHome, 'skills'))
    writeFileSync(join(userHome, 'config.toml'), 'model = "x"')

    const out1 = prepareCodexHome(state1, userHome)
    expect(out1).toBe(realpathSync(out1))
    expect(readdirSync(out1).sort()).toEqual(['auth.json', 'skills'])
    expect(lstatSync(join(out1, 'auth.json')).isSymbolicLink()).toBe(true)
    expect(lstatSync(join(out1, 'skills')).isSymbolicLink()).toBe(true)
    expect(readlinkSync(join(out1, 'auth.json'))).toBe(realpathSync(join(userHome, 'auth.json')))
    expect(readlinkSync(join(out1, 'skills'))).toBe(realpathSync(join(userHome, 'skills')))

    // Idempotent second call
    const out2 = prepareCodexHome(state1, userHome)
    expect(out2).toBe(out1)
    expect(readdirSync(out2).sort()).toEqual(['auth.json', 'skills'])

    // No skills dir -> only auth.json
    const userHomeNoSkills = join(root, 'user-codex-2')
    const state2 = join(root, 'state2')
    mkdirSync(userHomeNoSkills)
    mkdirSync(state2)
    writeFileSync(join(userHomeNoSkills, 'auth.json'), '{}')
    const out3 = prepareCodexHome(state2, userHomeNoSkills)
    expect(readdirSync(out3)).toEqual(['auth.json'])
    expect(lstatSync(join(out3, 'auth.json')).isSymbolicLink()).toBe(true)
  })
})
