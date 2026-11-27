import { afterEach, describe, expect, it } from 'vitest'
import { execFileSync } from 'node:child_process'
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { Store } from '../src/store.ts'
import {
  checkRunBranch,
  classifyRun,
  preflightTarget,
  prepareWorktree,
  resolveBaseSha,
  WorktreeConflictError,
  type Facts,
  type RunVerdict,
} from '../src/git-effect.ts'
import { makeRepo, tempDir } from './helpers.ts'

const temps: string[] = []
function trackTemp(d: string): string {
  temps.push(d)
  return d
}

afterEach(() => {
  while (temps.length > 0) {
    rmSync(temps.pop()!, { recursive: true, force: true })
  }
})

const SHA_A = 'a'.repeat(40)
const SHA_B = 'b'.repeat(40)
const ACCEPTED_A = {
  id: 1,
  runId: 'r11111111',
  turnId: 1,
  accepted: true,
  payload: JSON.stringify({ run_id: 'r11111111', branch: 'agent/r11111111', sha: SHA_A }),
  error: null,
  at: '2026-11-25T00:00:00.000Z',
}

const CLASSIFY_TABLE: [string, Facts, RunVerdict][] = [
  [
    'writer possible -> needs_review + freeze',
    { writer: 'possible', accepted: ACCEPTED_A, remote: { state: 'present', sha: SHA_A, problem: null } },
    {
      verdict: 'needs_review',
      reason: 'an agent process may still be writing',
      observedSha: SHA_A,
      freeze: true,
    },
  ],
  [
    'remote error -> check_failed',
    { writer: 'excluded', accepted: undefined, remote: { state: 'error', message: 'unreadable' } },
    { verdict: 'check_failed', message: 'unreadable' },
  ],
  [
    'accepted X, remote present tip X, no problem -> done',
    { writer: 'excluded', accepted: ACCEPTED_A, remote: { state: 'present', sha: SHA_A, problem: null } },
    { verdict: 'done', sha: SHA_A },
  ],
  [
    'remote present without accepted -> needs_review',
    { writer: 'excluded', accepted: undefined, remote: { state: 'present', sha: SHA_B, problem: null } },
    {
      verdict: 'needs_review',
      reason: 'branch pushed but no accepted result',
      observedSha: SHA_B,
      freeze: false,
    },
  ],
  [
    'accepted X, remote present tip Y != X -> needs_review',
    { writer: 'excluded', accepted: ACCEPTED_A, remote: { state: 'present', sha: SHA_B, problem: null } },
    {
      verdict: 'needs_review',
      reason: `remote tip ${SHA_B.slice(0, 8)} differs from accepted ${SHA_A.slice(0, 8)}`,
      observedSha: SHA_B,
      freeze: false,
    },
  ],
  [
    'accepted X, remote present tip X with problem -> needs_review',
    { writer: 'excluded', accepted: ACCEPTED_A, remote: { state: 'present', sha: SHA_A, problem: 'no commits after base' } },
    {
      verdict: 'needs_review',
      reason: 'no commits after base',
      observedSha: SHA_A,
      freeze: false,
    },
  ],
  [
    'accepted X, remote absent -> needs_review',
    { writer: 'excluded', accepted: ACCEPTED_A, remote: { state: 'absent' } },
    {
      verdict: 'needs_review',
      reason: `accepted ${SHA_A.slice(0, 8)} but branch is gone`,
      observedSha: null,
      freeze: false,
    },
  ],
  [
    'no accepted, remote absent -> retryable',
    { writer: 'excluded', accepted: undefined, remote: { state: 'absent' } },
    { verdict: 'retryable' },
  ],
]

describe('git-effect', () => {
  it('preflight accepts the fixture and rejects URL, split push, several push URLs and missing receive settings', async () => {
    const root = trackTemp(tempDir('polyclave-pf-'))
    const { work, remote } = makeRepo(root)
    await expect(preflightTarget(work)).resolves.toEqual({ remote })

    // URL origin
    const urlRepo = join(root, 'url-repo')
    mkdirSync(urlRepo)
    execFileSync('git', ['init', '-b', 'main', urlRepo])
    execFileSync('git', ['-C', urlRepo, 'remote', 'add', 'origin', 'https://example.com/repo.git'])
    await expect(preflightTarget(urlRepo)).rejects.toThrow('origin must be a local bare repository')

    // Split push
    const otherBare = join(root, 'other.git')
    execFileSync('git', ['init', '--bare', otherBare])
    execFileSync('git', ['-C', work, 'remote', 'set-url', '--push', 'origin', otherBare])
    await expect(preflightTarget(work)).rejects.toThrow('origin push and fetch targets differ')

    // Several push URLs
    execFileSync('git', ['-C', work, 'remote', 'set-url', '--add', '--push', 'origin', remote])
    await expect(preflightTarget(work)).rejects.toThrow('origin has several push targets')

    // Restore single push URL and remove denyNonFastForwards
    execFileSync('git', ['-C', work, 'config', '--unset-all', 'remote.origin.pushurl'])
    execFileSync('git', ['-C', remote, 'config', '--unset', 'receive.denyNonFastForwards'])
    await expect(preflightTarget(work)).rejects.toThrow(
      'remote must set receive.denyDeletes=true and receive.denyNonFastForwards=true',
    )
  })

  it('preflight is not fooled by changing origin after run creation', async () => {
    const root = trackTemp(tempDir('polyclave-pf2-'))
    const { work, remote } = makeRepo(root)
    const store = new Store(join(root, 'polyclave.sqlite'))
    const session = store.createSession({ name: 's1', cwd: work })
    const { remote: storedRemote } = await preflightTarget(work)
    const baseSha = await resolveBaseSha(storedRemote, 'main')
    const { run } = store.acceptRun({
      sessionId: session.id,
      requestId: 'q-pf',
      instructions: 'work',
      remote: storedRemote,
      base: 'main',
      baseSha,
      worktreePath: (id) => join(root, 'wt', id),
    })

    execFileSync('git', ['-C', work, 'remote', 'set-url', 'origin', 'https://example.com/hijack.git'])
    execFileSync('git', ['-C', work, 'checkout', '-b', run.branch, baseSha])
    writeFileSync(join(work, 'pf.txt'), 'pf\n')
    execFileSync('git', ['-C', work, 'add', '.'])
    execFileSync('git', ['-C', work, 'commit', '-m', 'pf commit', '--trailer', `Run-Id: ${run.id}`])
    const pushedSha = execFileSync('git', ['-C', work, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim()
    execFileSync('git', ['-C', work, 'push', run.remote, `refs/heads/${run.branch}:refs/heads/${run.branch}`])

    const check = await checkRunBranch({
      remote: run.remote,
      baseSha: run.baseSha,
      branch: run.branch,
      runId: run.id,
    })
    expect(check).toEqual({ state: 'present', sha: pushedSha, problem: null })
    expect(run.remote).toBe(remote)
    store.close()
  })

  it('prepares a worktree from baseSha and keeps existing work on the second call', async () => {
    const root = trackTemp(tempDir('polyclave-wt-'))
    const { work, remote } = makeRepo(root)
    const baseSha = await resolveBaseSha(remote, 'main')
    const wtPath = join(root, 'worktrees', 'r11111111')

    await prepareWorktree({
      cwd: work,
      remote,
      baseSha,
      branch: 'agent/r11111111',
      path: wtPath,
    })

    writeFileSync(join(wtPath, 'committed.txt'), 'v1\n')
    execFileSync('git', ['-C', wtPath, 'add', 'committed.txt'])
    execFileSync('git', ['-C', wtPath, 'commit', '-m', 'local work', '--trailer', 'Run-Id: r11111111'])
    const localHead = execFileSync('git', ['-C', wtPath, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim()
    writeFileSync(join(wtPath, 'dirty.txt'), 'uncommitted\n')

    await prepareWorktree({
      cwd: work,
      remote,
      baseSha,
      branch: 'agent/r11111111',
      path: wtPath,
    })

    expect(execFileSync('git', ['-C', wtPath, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim()).toBe(localHead)
    expect(execFileSync('git', ['-C', wtPath, 'status', '--porcelain'], { encoding: 'utf8' })).toContain('dirty.txt')
  })

  it('refuses a worktree path that holds something else', async () => {
    const root = trackTemp(tempDir('polyclave-wtc-'))
    const { work, remote } = makeRepo(root)
    const baseSha = await resolveBaseSha(remote, 'main')

    // Other repo at path
    const otherDir = join(root, 'other-repo')
    mkdirSync(otherDir)
    execFileSync('git', ['init', '-b', 'main', otherDir])
    writeFileSync(join(otherDir, 'keep.txt'), 'precious')
    await expect(
      prepareWorktree({ cwd: work, remote, baseSha, branch: 'agent/r1', path: otherDir }),
    ).rejects.toThrow(WorktreeConflictError)
    expect(readFileSync(join(otherDir, 'keep.txt'), 'utf8')).toBe('precious')

    // Same repo worktree, wrong branch
    const wtWrongBranch = join(root, 'wt-wrong-branch')
    await prepareWorktree({ cwd: work, remote, baseSha, branch: 'agent/r2', path: wtWrongBranch })
    execFileSync('git', ['-C', wtWrongBranch, 'checkout', '-b', 'other-branch'])
    writeFileSync(join(wtWrongBranch, 'branch-keep.txt'), 'keep branch')
    await expect(
      prepareWorktree({ cwd: work, remote, baseSha, branch: 'agent/r2', path: wtWrongBranch }),
    ).rejects.toThrow(WorktreeConflictError)
    expect(existsSync(wtWrongBranch)).toBe(true)
    expect(readFileSync(join(wtWrongBranch, 'branch-keep.txt'), 'utf8')).toBe('keep branch')

    // Same repo worktree, same branch, HEAD does not descend from baseSha
    const wtWrongBase = join(root, 'wt-wrong-base')
    await prepareWorktree({ cwd: work, remote, baseSha, branch: 'agent/r3', path: wtWrongBase })
    writeFileSync(join(wtWrongBase, 'base-keep.txt'), 'keep base')
    writeFileSync(join(work, 'second.txt'), '2\n')
    execFileSync('git', ['-C', work, 'add', '.'])
    execFileSync('git', ['-C', work, 'commit', '-m', 'second'])
    execFileSync('git', ['-C', work, 'push', 'origin', 'main'])
    const newerSha = await resolveBaseSha(remote, 'main')
    await expect(
      prepareWorktree({ cwd: work, remote, baseSha: newerSha, branch: 'agent/r3', path: wtWrongBase }),
    ).rejects.toThrow(WorktreeConflictError)
    expect(existsSync(wtWrongBase)).toBe(true)
    expect(readFileSync(join(wtWrongBase, 'base-keep.txt'), 'utf8')).toBe('keep base')
  })

  it('checkRunBranch: absent, clean, wrong base, empty range, body-only Run-Id', async () => {
    const root = trackTemp(tempDir('polyclave-crb-'))
    const { work, remote } = makeRepo(root)
    const baseSha = await resolveBaseSha(remote, 'main')
    const runId = 'r1234abcd'
    const branch = `agent/${runId}`

    // Absent
    expect(await checkRunBranch({ remote, baseSha, branch, runId })).toEqual({ state: 'absent' })

    // Empty range (branch points at baseSha)
    execFileSync('git', ['-C', work, 'checkout', '-b', branch])
    execFileSync('git', ['-C', work, 'push', remote, `refs/heads/${branch}:refs/heads/${branch}`])
    expect(await checkRunBranch({ remote, baseSha, branch, runId })).toEqual({
      state: 'present',
      sha: baseSha,
      problem: 'no commits after base',
    })

    // Body-only Run-Id (no blank line -> not a trailer)
    writeFileSync(join(work, 'a.txt'), 'a\n')
    execFileSync('git', ['-C', work, 'add', '.'])
    execFileSync('git', ['-C', work, 'commit', '-m', `subject\nRun-Id: ${runId}`])
    const bodyOnlySha = execFileSync('git', ['-C', work, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim()
    execFileSync('git', ['-C', work, 'push', remote, `refs/heads/${branch}:refs/heads/${branch}`])
    expect(await checkRunBranch({ remote, baseSha, branch, runId })).toEqual({
      state: 'present',
      sha: bodyOnlySha,
      problem: `commit ${bodyOnlySha.slice(0, 8)} lacks trailer Run-Id: ${runId}`,
    })

    // Clean commit with proper trailer on a fresh branch
    const cleanRunId = 'r9999aaaa'
    const cleanBranch = `agent/${cleanRunId}`
    execFileSync('git', ['-C', work, 'checkout', '-b', cleanBranch, baseSha])
    writeFileSync(join(work, 'b.txt'), 'b\n')
    execFileSync('git', ['-C', work, 'add', '.'])
    execFileSync('git', ['-C', work, 'commit', '-m', 'clean work', '--trailer', `Run-Id: ${cleanRunId}`])
    const cleanSha = execFileSync('git', ['-C', work, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim()
    execFileSync('git', ['-C', work, 'push', remote, `refs/heads/${cleanBranch}:refs/heads/${cleanBranch}`])
    expect(await checkRunBranch({ remote, baseSha, branch: cleanBranch, runId: cleanRunId })).toEqual({
      state: 'present',
      sha: cleanSha,
      problem: null,
    })

    // Wrong base (base is a newer unrelated commit)
    execFileSync('git', ['-C', work, 'checkout', 'main'])
    writeFileSync(join(work, 'm.txt'), 'm\n')
    execFileSync('git', ['-C', work, 'add', '.'])
    execFileSync('git', ['-C', work, 'commit', '-m', 'main ahead'])
    const mainAhead = execFileSync('git', ['-C', work, 'rev-parse', 'HEAD'], { encoding: 'utf8' }).trim()
    execFileSync('git', ['-C', work, 'push', 'origin', 'main'])
    expect(await checkRunBranch({ remote, baseSha: mainAhead, branch: cleanBranch, runId: cleanRunId })).toEqual({
      state: 'present',
      sha: cleanSha,
      problem: `base ${mainAhead.slice(0, 8)} is not an ancestor of ${cleanSha.slice(0, 8)}`,
    })
  })

  it('rejects when the bare repo cannot be read', async () => {
    const root = trackTemp(tempDir('polyclave-crb-err-'))
    const { remote } = makeRepo(root)
    const baseSha = await resolveBaseSha(remote, 'main')
    rmSync(remote, { recursive: true, force: true })
    await expect(
      checkRunBranch({ remote, baseSha, branch: 'agent/r1', runId: 'r1' }),
    ).rejects.toThrow()
  })

  it.each(CLASSIFY_TABLE)('classifyRun: %s', (_n, facts, expected) => {
    expect(classifyRun(facts)).toEqual(expected)
  })
})
