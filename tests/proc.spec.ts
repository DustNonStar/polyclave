import { afterEach, describe, expect, it } from 'vitest'
import { spawn } from 'node:child_process'
import { once } from 'node:events'
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  realpathSync,
  rmSync,
  writeFileSync,
} from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import {
  acquireHostLock,
  bootTime,
  groupLiveness,
  HostRunningError,
  killGroup,
  processStartTime,
  spawnSupervised,
} from '../src/proc.ts'

const temps: string[] = []
const cleanupPids: number[] = []

function makeTemp(prefix = 'polyclave-proc-'): string {
  const d = realpathSync(mkdtempSync(join(tmpdir(), prefix)))
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

async function pollUntil(fn: () => boolean, ms = 5000): Promise<void> {
  const deadline = Date.now() + ms
  while (Date.now() < deadline) {
    if (fn()) return
    await new Promise((r) => setTimeout(r, 25))
  }
  throw new Error('timed out waiting for condition')
}

afterEach(async () => {
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

describe('proc and supervisor', () => {
  it('does not start the agent before go', async () => {
    const dir = makeTemp()
    const marker = join(dir, 'started')
    const sup = await spawnSupervised(['sh', '-c', `touch "${marker}"`], {
      cwd: dir,
      env: process.env,
    })
    cleanupPids.push(sup.pgid)
    await new Promise((r) => setTimeout(r, 150))
    expect(existsSync(marker)).toBe(false)

    sup.abort()
    const code = await new Promise<number | null>((resolve) => {
      sup.child.on('exit', (c) => resolve(c))
    })
    expect(code).toBe(0)
    expect(existsSync(marker)).toBe(false)
  })

  it('does not start the agent if the host dies before go', async () => {
    const dir = makeTemp()
    const marker = join(dir, 'started')
    const procUrl = new URL('../src/proc.ts', import.meta.url).href
    const owner = spawn(
      process.execPath,
      [
        '--input-type=module',
        '-e',
        `
          import { spawnSupervised } from ${JSON.stringify(procUrl)};
          const sup = await spawnSupervised(['sh', '-c', 'touch ' + ${JSON.stringify(marker)}], {
            cwd: ${JSON.stringify(dir)},
            env: process.env,
          });
          console.log(JSON.stringify({ pgid: sup.pgid }));
          setInterval(() => {}, 10000);
        `,
      ],
      { stdio: ['ignore', 'pipe', 'pipe'] },
    )
    cleanupPids.push(owner.pid!)

    let out = ''
    const pgid = await new Promise<number>((resolve, reject) => {
      owner.stdout.on('data', (d) => {
        out += d.toString()
        const line = out.split('\n')[0]?.trim()
        if (line) {
          try {
            resolve((JSON.parse(line) as { pgid: number }).pgid)
          } catch {}
        }
      })
      owner.on('exit', (c) => reject(new Error(`owner exited early ${c}`)))
    })
    cleanupPids.push(pgid)

    process.kill(owner.pid!, 'SIGKILL')
    await pollUntil(() => !isPidAlive(pgid), 3000)
    await new Promise((r) => setTimeout(r, 150))
    expect(existsSync(marker)).toBe(false)
  })

  it('runs the agent after go, passes stdout and exit code', async () => {
    const dir = makeTemp()
    const sup = await spawnSupervised(['sh', '-c', 'echo hi; exit 3'], {
      cwd: dir,
      env: process.env,
    })
    cleanupPids.push(sup.pgid)
    let stdout = ''
    sup.child.stdout.on('data', (d) => {
      stdout += d.toString()
    })
    const exitPromise = new Promise<number | null>((resolve) => {
      sup.child.on('exit', (c) => resolve(c))
    })
    sup.go()
    const code = await exitPromise
    expect(stdout).toBe('hi\n')
    expect(code).toBe(3)
  })

  it('kills a member that ignores SIGTERM after its parent already exited', async () => {
    const dir = makeTemp()
    const pidFile = join(dir, 'child.pid')
    const sup = await spawnSupervised(
      [
        'sh',
        '-c',
        `sh -c 'trap "" TERM; echo $$ > "${pidFile}"; while true; do sleep 1; done' & wait`,
      ],
      { cwd: dir, env: process.env },
    )
    cleanupPids.push(sup.pgid)
    sup.go()

    await pollUntil(() => existsSync(pidFile) && readFileSync(pidFile, 'utf8').trim() !== '')
    const stubbornPid = Number(readFileSync(pidFile, 'utf8').trim())
    cleanupPids.push(stubbornPid)

    const start = Date.now()
    process.kill(sup.pgid, 'SIGTERM')
    await pollUntil(() => !isPidAlive(stubbornPid) && !isPidAlive(sup.pgid), 2500)
    expect(Date.now() - start).toBeLessThan(2500)
  })

  it('exits promptly when everything stops on SIGTERM', async () => {
    const dir = makeTemp()
    const readyFile = join(dir, 'ready')
    const sup = await spawnSupervised(
      ['sh', '-c', `touch "${readyFile}"; exec sleep 30`],
      { cwd: dir, env: process.env },
    )
    cleanupPids.push(sup.pgid)
    const exitPromise = new Promise<number | null>((resolve) => {
      sup.child.on('exit', (c) => resolve(c))
    })
    sup.go()
    await pollUntil(() => existsSync(readyFile))

    const t0 = Date.now()
    process.kill(sup.pgid, 'SIGTERM')
    await exitPromise
    expect(Date.now() - t0).toBeLessThan(1000)
  })

  it('ends the agent group within 3 s after its owner is SIGKILLed', async () => {
    const dir = makeTemp()
    const pidFile = join(dir, 'grandchild.pid')
    const procUrl = new URL('../src/proc.ts', import.meta.url).href
    const owner = spawn(
      process.execPath,
      [
        '--input-type=module',
        '-e',
        `
          import { spawnSupervised } from ${JSON.stringify(procUrl)};
          const sup = await spawnSupervised(
            ['sh', '-c', 'echo $$ > ' + ${JSON.stringify(pidFile)} + '; exec sleep 30'],
            { cwd: ${JSON.stringify(dir)}, env: process.env }
          );
          sup.go();
          setInterval(() => {}, 10000);
        `,
      ],
      { stdio: ['ignore', 'ignore', 'pipe'] },
    )
    cleanupPids.push(owner.pid!)

    await pollUntil(() => existsSync(pidFile) && readFileSync(pidFile, 'utf8').trim() !== '')
    const gcPid = Number(readFileSync(pidFile, 'utf8').trim())
    cleanupPids.push(gcPid)

    const t0 = Date.now()
    process.kill(owner.pid!, 'SIGKILL')
    await pollUntil(() => !isPidAlive(gcPid), 3000)
    expect(Date.now() - t0).toBeLessThan(3000)
  })

  it('kills the whole group on killGroup', async () => {
    const dir = makeTemp()
    const pidFile = join(dir, 'sleep.pid')
    const sup = await spawnSupervised(
      ['sh', '-c', `echo $$ > "${pidFile}"; exec sleep 30`],
      { cwd: dir, env: process.env },
    )
    cleanupPids.push(sup.pgid)
    const supervisorExit = once(sup.child, 'exit')
    sup.go()
    await pollUntil(() => existsSync(pidFile) && readFileSync(pidFile, 'utf8').trim() !== '')
    const pid = Number(readFileSync(pidFile, 'utf8').trim())
    cleanupPids.push(pid)

    await killGroup(sup.pgid, 500)
    expect(isPidAlive(pid)).toBe(false)
    // The supervisor can have stopped before Node reaps this direct child.
    await supervisorExit
    expect(isPidAlive(sup.pgid)).toBe(false)
  })

  it('groupLiveness: live, gone, reused leader, leaderless members', async () => {
    const dir = makeTemp()
    const readyFile = join(dir, 'ready')
    const sup = await spawnSupervised(
      ['sh', '-c', `touch "${readyFile}"; exec sleep 30`],
      { cwd: dir, env: process.env },
    )
    cleanupPids.push(sup.pgid)
    sup.go()
    await pollUntil(() => existsSync(readyFile))

    const startedAt = processStartTime(sup.pgid)
    expect(startedAt).toBeTruthy()
    expect(groupLiveness(sup.pgid, startedAt)).toBe('live')
    expect(groupLiveness(sup.pgid, 'Mon Jan  1 00:00:00 1970')).toBe('gone')

    await killGroup(sup.pgid, 500)
    expect(groupLiveness(sup.pgid, startedAt)).toBe('gone')

    // Leader exited with a member left in the same process group
    const memberPidFile = join(dir, 'member.pid')
    const leaderReady = join(dir, 'leader.ready')
    const leader = spawn(
      'sh',
      [
        '-c',
        `sleep 30 & echo $! > "${memberPidFile}"; touch "${leaderReady}"`,
      ],
      { detached: true, stdio: 'ignore' },
    )
    const leaderPgid = leader.pid!
    cleanupPids.push(leaderPgid)
    await pollUntil(() => existsSync(leaderReady) && !isPidAlive(leaderPgid))
    const memberPid = Number(readFileSync(memberPidFile, 'utf8').trim())
    cleanupPids.push(memberPid)
    expect(isPidAlive(memberPid)).toBe(true)
    expect(groupLiveness(leaderPgid, 'Mon Jan  1 00:00:00 2020')).toBe('unverifiable')
    expect(groupLiveness(leaderPgid, null)).toBe('unverifiable')
  })

  it('a detached child escapes the group', async () => {
    const dir = makeTemp()
    const detachedPidFile = join(dir, 'detached.pid')
    const script = `
      const { spawn } = require('node:child_process');
      const { writeFileSync } = require('node:fs');
      const c = spawn('sleep', ['30'], { detached: true, stdio: 'ignore' });
      c.unref();
      writeFileSync(${JSON.stringify(detachedPidFile)}, String(c.pid));
      setInterval(() => {}, 10000);
    `
    const sup = await spawnSupervised([process.execPath, '-e', script], {
      cwd: dir,
      env: process.env,
    })
    cleanupPids.push(sup.pgid)
    sup.go()
    await pollUntil(
      () => existsSync(detachedPidFile) && readFileSync(detachedPidFile, 'utf8').trim() !== '',
    )
    const detachedPid = Number(readFileSync(detachedPidFile, 'utf8').trim())
    cleanupPids.push(detachedPid)

    const start = processStartTime(sup.pgid)
    await killGroup(sup.pgid, 500)
    expect(groupLiveness(sup.pgid, start)).toBe('gone')
    expect(isPidAlive(detachedPid)).toBe(true)
  })

  it('refuses a second host lock while the first lives and takes over a dead or reused one', () => {
    const dir = makeTemp()
    const lock1 = acquireHostLock(dir)
    expect(() => acquireHostLock(dir)).toThrow(HostRunningError)
    lock1.release()

    // Dead pid 999999
    writeFileSync(
      join(dir, 'host.pid'),
      JSON.stringify({ pid: 999999, startedAt: 'Mon Jan  1 00:00:00 2020', instanceId: 'dead' }),
    )
    const lock2 = acquireHostLock(dir)
    expect(lock2.instanceId).toHaveLength(16)
    lock2.release()

    // Own pid with another start time -> reused pid -> taken over
    writeFileSync(
      join(dir, 'host.pid'),
      JSON.stringify({
        pid: process.pid,
        startedAt: 'Mon Jan  1 00:00:00 1970',
        instanceId: 'reused',
      }),
    )
    const lock3 = acquireHostLock(dir)
    expect(lock3.instanceId).toHaveLength(16)
    lock3.release()
  })

  it('bootTime and processStartTime do not depend on the time zone or locale', () => {
    const prevTz = process.env.TZ
    const prevLc = process.env.LC_ALL
    try {
      process.env.TZ = 'America/New_York'
      process.env.LC_ALL = 'en_US.UTF-8'
      const b1 = bootTime()
      const p1 = processStartTime(process.pid)

      process.env.TZ = 'Asia/Tokyo'
      process.env.LC_ALL = 'ja_JP.UTF-8'
      const b2 = bootTime()
      const p2 = processStartTime(process.pid)

      expect(b1).toBe(b2)
      expect(b1).toMatch(/^\d+\.\d+$/)
      expect(p1).toBe(p2)
      expect(p1).toBeTruthy()
    } finally {
      if (prevTz === undefined) delete process.env.TZ
      else process.env.TZ = prevTz
      if (prevLc === undefined) delete process.env.LC_ALL
      else process.env.LC_ALL = prevLc
    }
  })

  it('fake codex logs prompt_started before running its steps', async () => {
    const dir = makeTemp()
    const scriptPath = join(dir, 'script.json')
    const logPath = join(dir, 'fake.log')
    writeFileSync(scriptPath, JSON.stringify({ '1': [{ sleepMs: 5000 }] }))

    const fakePath = new URL('./fixtures/fake-codex.ts', import.meta.url).pathname
    const child = spawn(
      process.execPath,
      [fakePath, 'exec', '--', 'hello world'],
      {
        cwd: dir,
        env: {
          ...process.env,
          FAKE_CODEX_SCRIPT: scriptPath,
          FAKE_CODEX_LOG: logPath,
        },
        stdio: ['ignore', 'pipe', 'pipe'],
      },
    )
    cleanupPids.push(child.pid!)

    await pollUntil(() => existsSync(logPath) && readFileSync(logPath, 'utf8').trim() !== '')
    child.kill('SIGKILL')
    const lines = readFileSync(logPath, 'utf8')
      .trim()
      .split('\n')
      .map((l) => JSON.parse(l) as { event: string; prompt: string; turn: number })
    expect(lines[0]?.event).toBe('prompt_started')
    expect(lines[0]?.prompt).toBe('hello world')
    expect(lines[0]?.turn).toBe(1)
  })

  it('a normal exit does not leave group members behind', async () => {
    const dir = makeTemp()
    const pidFile = join(dir, 'member.pid')
    const sup = await spawnSupervised(
      ['sh', '-c', `sleep 30 & echo $! > "${pidFile}"; exit 0`],
      { cwd: dir, env: process.env },
    )
    cleanupPids.push(sup.pgid)
    const exitPromise = new Promise<number | null>((resolve) => {
      sup.child.on('exit', (c) => resolve(c))
    })
    sup.go()
    const code = await exitPromise
    expect(code).toBe(0)
    const memberPid = Number(readFileSync(pidFile, 'utf8').trim())
    cleanupPids.push(memberPid)
    expect(isPidAlive(memberPid)).toBe(false)
    expect(groupLiveness(sup.pgid, null)).toBe('gone')
  })

  it('keeps waiting until the 2 s timer when pgrep fails', async () => {
    const dir = makeTemp()
    const binDir = join(dir, 'bin')
    mkdirSync(binDir)
    const fakePgrep = join(binDir, 'pgrep')
    writeFileSync(fakePgrep, '#!/bin/sh\nexit 2\n')
    chmodSync(fakePgrep, 0o755)

    const readyFile = join(dir, 'ready')
    const sup = await spawnSupervised(
      ['sh', '-c', `touch "${readyFile}"; exec sleep 30`],
      {
        cwd: dir,
        env: { ...process.env, PATH: `${binDir}:${process.env.PATH ?? ''}` },
      },
    )
    cleanupPids.push(sup.pgid)
    const exitPromise = new Promise<number | null>((resolve) => {
      sup.child.on('exit', (c) => resolve(c))
    })
    sup.go()
    await pollUntil(() => existsSync(readyFile))

    const t0 = Date.now()
    process.kill(sup.pgid, 'SIGTERM')
    await exitPromise
    const elapsed = Date.now() - t0
    expect(elapsed).toBeGreaterThanOrEqual(1800)
  })
})
