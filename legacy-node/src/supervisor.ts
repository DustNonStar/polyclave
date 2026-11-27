import { execFileSync, spawn, type ChildProcess } from 'node:child_process'
import net from 'node:net'

const [ownerPidStr, sep, ...command] = process.argv.slice(2)
const ownerPid = Number(ownerPidStr)

if (!Number.isInteger(ownerPid) || sep !== '--' || command.length === 0) {
  process.exit(2)
}

const sock = new net.Socket({ fd: 3, readable: true, writable: true })
let started = false
let terminating = false
let child: ChildProcess | null = null
let childExited = false
let childCode = 0
let ppidTimer: NodeJS.Timeout | null = null
let pollTimer: NodeJS.Timeout | null = null
let killTimer: NodeJS.Timeout | null = null
let buf = ''

function listOtherGroupMembers(): { ok: true; pids: number[] } | { ok: false } {
  try {
    const out = execFileSync('pgrep', ['-g', String(process.pid)], {
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'pipe'],
    }).trim()
    const pids = out
      .split(/\s+/)
      .filter(Boolean)
      .map(Number)
      .filter((pid) => Number.isInteger(pid) && pid > 0 && pid !== process.pid)
    return { ok: true, pids }
  } catch (err) {
    const e = err as { status?: number | null }
    if (e.status === 1) return { ok: true, pids: [] }
    return { ok: false }
  }
}

function noGroupMembersLeft(): boolean {
  const res = listOtherGroupMembers()
  return res.ok && res.pids.length === 0
}

function finishExit(): void {
  if (ppidTimer) clearInterval(ppidTimer)
  if (pollTimer) clearInterval(pollTimer)
  if (killTimer) clearTimeout(killTimer)
  process.exit(childCode)
}

function terminate(): void {
  if (!started) {
    try {
      sock.destroy()
    } catch {}
    process.exit(0)
  }
  if (terminating) return
  terminating = true
  if (ppidTimer) clearInterval(ppidTimer)

  if (childExited && noGroupMembersLeft()) {
    finishExit()
  }

  try {
    process.kill(-process.pid, 'SIGTERM')
  } catch {}

  const checkDone = () => {
    if (childExited && noGroupMembersLeft()) {
      finishExit()
    }
  }

  pollTimer = setInterval(checkDone, 50)
  killTimer = setTimeout(() => {
    const members = listOtherGroupMembers()
    if (members.ok) {
      for (const pid of members.pids) {
        try {
          process.kill(pid, 'SIGKILL')
        } catch {}
      }
    }
    if (!childExited && child?.pid) {
      try {
        process.kill(child.pid, 'SIGKILL')
      } catch {}
    }
    finishExit()
  }, 2000)

  checkDone()
}

for (const sig of ['SIGTERM', 'SIGINT', 'SIGHUP'] as const) {
  process.on(sig, () => {
    terminate()
  })
}

function launchAgent(): void {
  if (started) return
  started = true
  try {
    sock.destroy()
  } catch {}

  const cmd = command[0]!
  const args = command.slice(1)
  child = spawn(cmd, args, {
    stdio: ['ignore', 'inherit', 'inherit'],
  })

  child.on('error', () => {
    childExited = true
    childCode = 127
    if (!terminating) {
      terminate()
    } else if (noGroupMembersLeft()) {
      finishExit()
    }
  })

  child.on('exit', (code, signal) => {
    childExited = true
    childCode = code ?? (signal ? 1 : 0)
    if (!terminating) {
      terminate()
    } else if (noGroupMembersLeft()) {
      finishExit()
    }
  })

  ppidTimer = setInterval(() => {
    if (process.ppid !== ownerPid) {
      terminate()
    }
  }, 500)
}

sock.on('data', (chunk) => {
  if (started) return
  buf += chunk.toString('utf8')
  const nl = buf.indexOf('\n')
  if (nl < 0) return
  const line = buf.slice(0, nl).trim()
  if (line === 'go') {
    launchAgent()
  } else {
    try {
      sock.destroy()
    } catch {}
    process.exit(0)
  }
})

sock.on('end', () => {
  if (!started) {
    try {
      sock.destroy()
    } catch {}
    process.exit(0)
  }
})

sock.on('close', () => {
  if (!started) {
    process.exit(0)
  }
})

sock.on('error', () => {
  if (!started) {
    process.exit(0)
  }
})

sock.write('ready\n')
