import { execFileSync, spawn, type ChildProcessByStdio } from 'node:child_process'
import {
  closeSync,
  openSync,
  readFileSync,
  realpathSync,
  renameSync,
  unlinkSync,
  writeFileSync,
  writeSync,
} from 'node:fs'
import { join } from 'node:path'
import type { Duplex, Readable } from 'node:stream'
import { fileURLToPath } from 'node:url'
import { hostInstanceId as makeHostInstanceId } from './ids.ts'

export interface Supervised {
  child: ChildProcessByStdio<null, Readable, Readable>
  pgid: number
  go(): void
  abort(): void
}

export type Liveness = 'live' | 'gone' | 'unverifiable'

export class HostRunningError extends Error {
  pid: number
  constructor(pid: number) {
    super(`host already running (pid ${pid})`)
    this.pid = pid
  }
}

const SUPERVISOR_PATH = fileURLToPath(new URL('./supervisor.ts', import.meta.url))

export function spawnSupervised(
  command: string[],
  o: { cwd: string; env: NodeJS.ProcessEnv; ownerPid?: number },
): Promise<Supervised> {
  const realCwd = realpathSync(o.cwd)
  const ownerPid = o.ownerPid ?? process.pid
  const child = spawn(
    process.execPath,
    [SUPERVISOR_PATH, String(ownerPid), '--', ...command],
    {
      cwd: realCwd,
      env: o.env,
      detached: true,
      stdio: ['ignore', 'pipe', 'pipe', 'pipe'],
    },
  )

  const ctrl = child.stdio[3] as Duplex | null | undefined
  if (!ctrl) {
    try {
      child.kill('SIGKILL')
    } catch {}
    return Promise.reject(new Error('missing supervisor control channel'))
  }

  return new Promise<Supervised>((resolve, reject) => {
    let settled = false
    let buf = ''

    const fail = (err: Error) => {
      if (settled) return
      settled = true
      ctrl.off('data', onData)
      ctrl.off('close', onCtrlClose)
      child.off('error', onError)
      child.off('exit', onExit)
      reject(err)
    }

    const onError = (err: Error) => fail(err)
    const onExit = (code: number | null, signal: NodeJS.Signals | null) => {
      fail(new Error(`supervisor exited before ready (code=${code}, signal=${signal})`))
    }
    const onCtrlClose = () => {
      fail(new Error('supervisor control channel closed before ready'))
    }

    const onData = (chunk: Buffer) => {
      buf += chunk.toString('utf8')
      const nl = buf.indexOf('\n')
      if (nl < 0) return
      const line = buf.slice(0, nl).trim()
      if (line !== 'ready') {
        fail(new Error(`unexpected supervisor handshake: ${line}`))
        return
      }
      if (settled) return
      settled = true
      ctrl.off('data', onData)
      ctrl.off('close', onCtrlClose)
      child.off('error', onError)
      child.off('exit', onExit)

      const pgid = child.pid!
      resolve({
        child: child as unknown as ChildProcessByStdio<null, Readable, Readable>,
        pgid,
        go() {
          try {
            if (!ctrl.destroyed && ctrl.writable) {
              ctrl.end('go\n')
            }
          } catch {}
        },
        abort() {
          try {
            if (!ctrl.destroyed && ctrl.writable) {
              ctrl.end('abort\n')
            }
          } catch {}
        },
      })
    }

    ctrl.on('data', onData)
    ctrl.on('close', onCtrlClose)
    child.on('error', onError)
    child.on('exit', onExit)
  })
}

export function processStartTime(pid: number): string | null {
  try {
    const out = execFileSync('ps', ['-o', 'lstart=', '-p', String(pid)], {
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'pipe'],
      env: { ...process.env, LC_ALL: 'C', TZ: 'UTC' },
    }).trim()
    return out || null
  } catch (err) {
    const e = err as { status?: number | null; stdout?: Buffer | string | null }
    const stdout = e.stdout ? e.stdout.toString('utf8').trim() : ''
    if (e.status === 1 && stdout === '') {
      try {
        process.kill(pid, 0)
      } catch (kErr) {
        if ((kErr as { code?: string }).code === 'ESRCH') {
          return null
        }
      }
    }
    throw err
  }
}

export function bootTime(): string {
  const override = process.env.POLYCLAVE_TEST_BOOT_TIME
  if (override !== undefined && override !== '') return override
  const out = execFileSync('sysctl', ['-n', 'kern.boottime'], {
    encoding: 'utf8',
  }).trim()
  const match = /sec\s*=\s*(\d+),\s*usec\s*=\s*(\d+)/.exec(out)
  if (!match) {
    throw new Error(`unexpected kern.boottime output: ${out}`)
  }
  return `${match[1]}.${match[2]}`
}

function pgrepGroup(pgid: number): { status: number; pids: number[] } {
  try {
    const out = execFileSync('pgrep', ['-g', String(pgid)], {
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'pipe'],
    }).trim()
    const pids = out
      .split(/\s+/)
      .filter(Boolean)
      .map(Number)
      .filter((n) => Number.isInteger(n) && n > 0)
    return { status: 0, pids }
  } catch (err) {
    const e = err as { status?: number | null }
    return { status: e.status ?? 2, pids: [] }
  }
}

export function groupLiveness(pgid: number, leaderStartedAt: string | null): Liveness {
  const first = pgrepGroup(pgid)
  if (first.status === 1) return 'gone'
  if (first.status >= 2) return 'unverifiable'

  if (leaderStartedAt === null) {
    const second = pgrepGroup(pgid)
    if (second.status === 1) return 'gone'
    return 'unverifiable'
  }

  let currentStart: string | null
  try {
    currentStart = processStartTime(pgid)
  } catch {
    return 'unverifiable'
  }

  if (currentStart === null) {
    const second = pgrepGroup(pgid)
    if (second.status === 1) return 'gone'
    return 'unverifiable'
  }

  if (currentStart !== leaderStartedAt) return 'gone'
  return 'live'
}

export async function killGroup(pgid: number, graceMs = 2000): Promise<void> {
  try {
    process.kill(-pgid, 'SIGTERM')
  } catch {
    return
  }

  const deadline = Date.now() + graceMs
  while (Date.now() < deadline) {
    const check = pgrepGroup(pgid)
    if (check.status === 1) return
    await new Promise((r) => setTimeout(r, 50))
  }

  try {
    process.kill(-pgid, 'SIGKILL')
  } catch {}
  const remaining = pgrepGroup(pgid)
  for (const pid of remaining.pids) {
    try {
      process.kill(pid, 'SIGKILL')
    } catch {}
  }

  const killDeadline = Date.now() + 500
  while (Date.now() < killDeadline) {
    if (pgrepGroup(pgid).status === 1) return
    await new Promise((r) => setTimeout(r, 25))
  }
}

export function acquireHostLock(stateDir: string): {
  instanceId: string
  release(): void
} {
  const realState = realpathSync(stateDir)
  const lockPath = join(realState, 'host.pid')
  const instanceId = makeHostInstanceId()
  const payload = JSON.stringify({
    pid: process.pid,
    startedAt: processStartTime(process.pid),
    instanceId,
  })

  try {
    const fd = openSync(lockPath, 'wx')
    try {
      writeSync(fd, payload)
    } finally {
      closeSync(fd)
    }
  } catch (err) {
    if ((err as { code?: string }).code !== 'EEXIST') throw err

    let existing: { pid?: number; startedAt?: string | null } | null = null
    try {
      existing = JSON.parse(readFileSync(lockPath, 'utf8')) as {
        pid?: number
        startedAt?: string | null
      }
    } catch {}

    if (existing && typeof existing.pid === 'number' && existing.pid > 0) {
      const curStart = processStartTime(existing.pid)
      if (
        curStart !== null &&
        (existing.startedAt == null || curStart === existing.startedAt)
      ) {
        throw new HostRunningError(existing.pid)
      }
    }

    const tmpPath = join(realState, `host.pid.tmp.${process.pid}.${instanceId}`)
    writeFileSync(tmpPath, payload)
    renameSync(tmpPath, lockPath)
  }

  return {
    instanceId,
    release() {
      try {
        const current = JSON.parse(readFileSync(lockPath, 'utf8')) as {
          instanceId?: string
        }
        if (current.instanceId === instanceId) {
          unlinkSync(lockPath)
        }
      } catch {}
    },
  }
}
