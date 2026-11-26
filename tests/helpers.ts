import { execFileSync } from 'node:child_process'
import {
  chmodSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  realpathSync,
  writeFileSync,
} from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

export const FAKE_CODEX: string[] = [
  process.execPath,
  fileURLToPath(new URL('./fixtures/fake-codex.ts', import.meta.url)),
]

export function tempDir(prefix = 'polyclave-test-'): string {
  return realpathSync(mkdtempSync(join(tmpdir(), prefix)))
}

export function makeRepo(baseDir?: string): {
  work: string
  remote: string
  refLog: string
} {
  const root = baseDir ? realpathSync(baseDir) : tempDir('polyclave-repo-')
  const remote = join(root, 'remote.git')
  const work = join(root, 'work')
  const refLog = join(root, 'refs.log')

  execFileSync('git', ['init', '--bare', '-b', 'main', remote])
  execFileSync('git', ['-C', remote, 'config', 'receive.denyDeletes', 'true'])
  execFileSync('git', ['-C', remote, 'config', 'receive.denyNonFastForwards', 'true'])

  const hookPath = join(remote, 'hooks', 'post-receive')
  writeFileSync(
    hookPath,
    `#!/bin/sh\nwhile read old new ref; do\n  printf '%s %s %s\\n' "$ref" "$old" "$new" >> ${JSON.stringify(refLog)}\ndone\n`,
  )
  chmodSync(hookPath, 0o755)

  execFileSync('git', ['clone', '-q', remote, work])
  execFileSync('git', ['-C', work, 'config', 'user.name', 'Test User'])
  execFileSync('git', ['-C', work, 'config', 'user.email', 'test@example.invalid'])
  execFileSync('git', ['-C', work, 'checkout', '-q', '-B', 'main'])
  writeFileSync(join(work, 'README.md'), '# test repo\n')
  execFileSync('git', ['-C', work, 'add', '.'])
  execFileSync('git', ['-C', work, 'commit', '-q', '-m', 'init'])
  execFileSync('git', ['-C', work, 'push', '-q', 'origin', 'main'])

  writeFileSync(refLog, '')

  return {
    work: realpathSync(work),
    remote: realpathSync(remote),
    refLog: realpathSync(refLog),
  }
}

export interface Barrier {
  dir: string
  reachedPath: string
  releasePath: string
  wait(): Promise<void>
  waitReached(ms?: number): Promise<void>
  release(): void
}

export function barrier(dir: string, name: string): Barrier {
  const bDir = join(realpathSync(dir), name)
  mkdirSync(bDir, { recursive: true })
  const reachedPath = join(bDir, 'reached')
  const releasePath = join(bDir, 'release')
  return {
    dir: bDir,
    reachedPath,
    releasePath,
    async wait() {
      writeFileSync(reachedPath, '1')
      await waitFor(() => existsSync(releasePath))
    },
    async waitReached(ms = 10000) {
      await waitFor(() => existsSync(reachedPath), ms)
    },
    release() {
      writeFileSync(releasePath, '1')
    },
  }
}

export async function waitFor<T>(
  fn: () => T | Promise<T>,
  ms = 10000,
): Promise<Exclude<T, false | null | undefined | 0 | ''>> {
  const deadline = Date.now() + ms
  let lastError: unknown
  while (Date.now() < deadline) {
    try {
      const value = await fn()
      if (value) {
        return value as Exclude<T, false | null | undefined | 0 | ''>
      }
      lastError = undefined
    } catch (err) {
      lastError = err
    }
    await new Promise((r) => setTimeout(r, 25))
  }
  if (lastError) throw lastError
  throw new Error(`waitFor timed out after ${ms} ms`)
}
