import { execFileSync } from 'node:child_process'
import {
  chmodSync,
  existsSync,
  lstatSync,
  mkdirSync,
  readlinkSync,
  realpathSync,
  rmSync,
  symlinkSync,
} from 'node:fs'
import { homedir } from 'node:os'
import { join } from 'node:path'

export type AgentEvent =
  | { kind: 'message.done'; text: string }
  | { kind: 'tool.call'; name: string; input: unknown }
  | { kind: 'tool.result'; name: string; ok: boolean }
  | { kind: 'turn.done'; usage?: unknown }
  | { kind: 'error'; message: string }

export interface CodexArgsInput {
  agentSessionId: string | null
  prompt: string
  mcpUrl: string
  writableRoots: string[]
}

export function buildCodexArgs(i: CodexArgsInput): string[] {
  const head = i.agentSessionId === null ? ['exec'] : ['exec', 'resume']
  const flags = [
    '--json',
    '--skip-git-repo-check',
    '--ignore-user-config',
    '-c',
    'approval_policy="never"',
    '-c',
    'sandbox_mode="workspace-write"',
    '-c',
    'sandbox_workspace_write.network_access=false',
    '-c',
    `sandbox_workspace_write.writable_roots=${JSON.stringify(i.writableRoots)}`,
    '-c',
    `mcp_servers.host.url=${JSON.stringify(i.mcpUrl)}`,
    '-c',
    'mcp_servers.host.bearer_token_env_var="POLYCLAVE_MCP_TOKEN"',
    '-c',
    'mcp_servers.host.default_tools_approval_mode="approve"',
  ]
  const tail =
    i.agentSessionId === null ? ['--', i.prompt] : ['--', i.agentSessionId, i.prompt]
  return [...head, ...flags, ...tail]
}

export function parseCodexLine(
  line: string,
): { threadId?: string; event?: AgentEvent; type?: string } | null {
  const trimmed = line.trim()
  if (!trimmed) return null
  let parsed: unknown
  try {
    parsed = JSON.parse(trimmed)
  } catch {
    return null
  }
  if (!parsed || typeof parsed !== 'object') return null

  const ev = parsed as Record<string, unknown>
  const type = typeof ev.type === 'string' ? ev.type : undefined

  if (type === 'thread.started') {
    const threadId = typeof ev.thread_id === 'string' ? ev.thread_id : undefined
    return { type, threadId }
  }

  const item =
    ev.item && typeof ev.item === 'object'
      ? (ev.item as Record<string, unknown>)
      : undefined

  if (type === 'item.started' && item) {
    if (item.type === 'command_execution') {
      return {
        type,
        event: {
          kind: 'tool.call',
          name: 'shell',
          input: { command: item.command },
        },
      }
    }
    if (item.type === 'mcp_tool_call') {
      const name =
        typeof item.tool === 'string'
          ? item.tool
          : typeof item.name === 'string'
            ? item.name
            : 'mcp'
      const input = item.arguments ?? item.input ?? {}
      return {
        type,
        event: { kind: 'tool.call', name, input },
      }
    }
  }

  if (type === 'item.completed' && item) {
    if (item.type === 'agent_message') {
      return {
        type,
        event: {
          kind: 'message.done',
          text: typeof item.text === 'string' ? item.text : '',
        },
      }
    }
    if (item.type === 'command_execution') {
      return {
        type,
        event: {
          kind: 'tool.result',
          name: 'shell',
          ok: item.exit_code === 0,
        },
      }
    }
    if (item.type === 'mcp_tool_call') {
      const name =
        typeof item.tool === 'string'
          ? item.tool
          : typeof item.name === 'string'
            ? item.name
            : 'mcp'
      const ok =
        item.status !== undefined
          ? item.status === 'completed' && item.error == null
          : item.error == null
      return {
        type,
        event: { kind: 'tool.result', name, ok },
      }
    }
  }

  if (type === 'turn.completed') {
    return {
      type,
      event: { kind: 'turn.done', usage: ev.usage },
    }
  }

  if (type === 'turn.failed' || type === 'error') {
    const errObj =
      ev.error && typeof ev.error === 'object'
        ? (ev.error as Record<string, unknown>)
        : undefined
    const message =
      typeof errObj?.message === 'string'
        ? errObj.message
        : typeof ev.message === 'string'
          ? ev.message
          : 'turn failed'
    return {
      type,
      event: { kind: 'error', message },
    }
  }

  return type ? { type } : {}
}

export function prepareCodexHome(stateDir: string, userCodexHome?: string): string {
  const realState = realpathSync(stateDir)
  const codexHome = join(realState, 'codex-home')
  mkdirSync(codexHome, { recursive: true, mode: 0o700 })
  chmodSync(codexHome, 0o700)

  const srcHome = userCodexHome ?? process.env.CODEX_HOME ?? join(homedir(), '.codex')
  for (const name of ['auth.json', 'skills'] as const) {
    const srcPath = join(srcHome, name)
    const dstPath = join(codexHome, name)
    const existing = lstatSync(dstPath, { throwIfNoEntry: false })
    if (existsSync(srcPath)) {
      const target = realpathSync(srcPath)
      if (existing) {
        if (existing.isSymbolicLink()) {
          try {
            if (readlinkSync(dstPath) === target) continue
          } catch {}
        }
        rmSync(dstPath, { recursive: true, force: true })
      }
      symlinkSync(target, dstPath)
    } else if (existing) {
      rmSync(dstPath, { recursive: true, force: true })
    }
  }

  return realpathSync(codexHome)
}

export function gitCommonDir(cwd: string): string {
  const realCwd = realpathSync(cwd)
  const out = execFileSync(
    'git',
    ['-C', realCwd, 'rev-parse', '--path-format=absolute', '--git-common-dir'],
    { encoding: 'utf8' },
  ).trim()
  return realpathSync(out)
}
