import { execFile as execFileCb } from 'node:child_process'
import { existsSync, mkdirSync, realpathSync } from 'node:fs'
import { basename, dirname, isAbsolute, join } from 'node:path'
import { promisify } from 'node:util'
import { gitCommonDir } from './codex.ts'
import type { ResultRow, WriterCheck } from './store.ts'

const execFile = promisify(execFileCb)

export class WorktreeConflictError extends Error {}

export type BranchCheck =
  | { state: 'absent' }
  | { state: 'present'; sha: string; problem: string | null }

export type Facts = {
  writer: WriterCheck
  accepted: ResultRow | undefined
  remote: BranchCheck | { state: 'error'; message: string }
}

export type RunVerdict =
  | { verdict: 'done'; sha: string }
  | { verdict: 'retryable' }
  | { verdict: 'needs_review'; reason: string; observedSha: string | null; freeze: boolean }
  | { verdict: 'check_failed'; message: string }

async function git(cwd: string, args: string[]): Promise<string> {
  const { stdout } = await execFile('git', ['-C', cwd, ...args], {
    encoding: 'utf8',
  })
  return stdout.trim()
}

async function gitStatus(
  cwd: string,
  args: string[],
): Promise<{ status: number; stdout: string; stderr: string }> {
  try {
    const { stdout, stderr } = await execFile('git', ['-C', cwd, ...args], {
      encoding: 'utf8',
    })
    return { status: 0, stdout: stdout.trim(), stderr: stderr.trim() }
  } catch (err) {
    const e = err as { code?: number | string; stdout?: string; stderr?: string }
    if (typeof e.code !== 'number') throw err
    return {
      status: e.code,
      stdout: (e.stdout ?? '').trim(),
      stderr: (e.stderr ?? '').trim(),
    }
  }
}

export async function preflightTarget(cwd: string): Promise<{ remote: string }> {
  const realCwd = realpathSync(cwd)

  let fetchUrl: string
  try {
    fetchUrl = await git(realCwd, ['remote', 'get-url', 'origin'])
  } catch {
    throw new Error('origin must be a local bare repository')
  }

  let pushOut: string
  try {
    pushOut = await git(realCwd, ['remote', 'get-url', '--push', '--all', 'origin'])
  } catch {
    throw new Error('origin must be a local bare repository')
  }

  const pushUrls = pushOut
    .split('\n')
    .map((s) => s.trim())
    .filter(Boolean)

  if (pushUrls.length > 1) {
    throw new Error('origin has several push targets')
  }
  if (pushUrls.length === 0) {
    throw new Error('origin must be a local bare repository')
  }

  const pushUrl = pushUrls[0]!
  const fetchAbs = isAbsolute(fetchUrl)
  const pushAbs = isAbsolute(pushUrl)

  if (!fetchAbs && !pushAbs) {
    throw new Error('origin must be a local bare repository')
  }

  if (fetchAbs !== pushAbs) {
    throw new Error('origin push and fetch targets differ')
  }

  let realFetch: string
  let realPush: string
  try {
    realFetch = realpathSync(fetchUrl)
  } catch {
    throw new Error('origin must be a local bare repository')
  }
  try {
    realPush = realpathSync(pushUrl)
  } catch {
    throw new Error('origin push and fetch targets differ')
  }

  if (realFetch !== realPush) {
    throw new Error('origin push and fetch targets differ')
  }

  let isBare: string
  try {
    isBare = await git(realFetch, ['rev-parse', '--is-bare-repository'])
  } catch {
    throw new Error('origin must be a local bare repository')
  }
  if (isBare !== 'true') {
    throw new Error('origin must be a local bare repository')
  }

  const denyDeletes = await gitStatus(realFetch, ['config', '--get', 'receive.denyDeletes'])
  const denyNonFf = await gitStatus(realFetch, [
    'config',
    '--get',
    'receive.denyNonFastForwards',
  ])
  if (
    denyDeletes.status !== 0 ||
    denyDeletes.stdout !== 'true' ||
    denyNonFf.status !== 0 ||
    denyNonFf.stdout !== 'true'
  ) {
    throw new Error(
      'remote must set receive.denyDeletes=true and receive.denyNonFastForwards=true',
    )
  }

  return { remote: realFetch }
}

export async function resolveBaseSha(remote: string, base: string): Promise<string> {
  const realRemote = realpathSync(remote)
  return git(realRemote, ['rev-parse', '--verify', `${base}^{commit}`])
}

export async function prepareWorktree(i: {
  cwd: string
  remote: string
  baseSha: string
  branch: string
  path: string
}): Promise<void> {
  const realCwd = realpathSync(i.cwd)
  const realRemote = realpathSync(i.remote)

  if (existsSync(i.path)) {
    const realTarget = realpathSync(i.path)
    let targetCommon: string
    let cwdCommon: string
    try {
      targetCommon = gitCommonDir(realTarget)
      cwdCommon = gitCommonDir(realCwd)
    } catch {
      throw new WorktreeConflictError(`worktree path ${i.path} is not a git worktree`)
    }
    if (targetCommon !== cwdCommon) {
      throw new WorktreeConflictError(
        `worktree path ${i.path} belongs to another repository`,
      )
    }

    const headBranch = await gitStatus(realTarget, [
      'symbolic-ref',
      '--quiet',
      '--short',
      'HEAD',
    ])
    if (headBranch.status !== 0 || headBranch.stdout !== i.branch) {
      throw new WorktreeConflictError(
        `worktree path ${i.path} is on ${headBranch.stdout || 'detached HEAD'}, expected ${i.branch}`,
      )
    }

    const ancestor = await gitStatus(realTarget, [
      'merge-base',
      '--is-ancestor',
      i.baseSha,
      'HEAD',
    ])
    if (ancestor.status !== 0) {
      throw new WorktreeConflictError(
        `worktree path ${i.path} HEAD does not descend from ${i.baseSha.slice(0, 8)}`,
      )
    }
    return
  }

  const parent = dirname(i.path)
  mkdirSync(parent, { recursive: true })
  const realTarget = join(realpathSync(parent), basename(i.path))

  await git(realCwd, ['fetch', realRemote])
  const hasCommit = await gitStatus(realCwd, ['cat-file', '-e', `${i.baseSha}^{commit}`])
  if (hasCommit.status !== 0) {
    await git(realCwd, ['fetch', realRemote, i.baseSha])
  }

  await git(realCwd, ['worktree', 'prune'])
  const existingBranch = await gitStatus(realCwd, [
    'rev-parse',
    '--verify',
    '--quiet',
    `refs/heads/${i.branch}`,
  ])
  if (existingBranch.status === 0) {
    const anc = await gitStatus(realCwd, [
      'merge-base',
      '--is-ancestor',
      i.baseSha,
      `refs/heads/${i.branch}`,
    ])
    if (anc.status !== 0) {
      throw new WorktreeConflictError(
        `existing branch ${i.branch} does not descend from ${i.baseSha.slice(0, 8)}`,
      )
    }
    await git(realCwd, ['worktree', 'add', realTarget, i.branch])
  } else {
    await git(realCwd, ['worktree', 'add', '-b', i.branch, realTarget, i.baseSha])
  }
}

export async function checkRunBranch(i: {
  remote: string
  baseSha: string
  branch: string
  runId: string
}): Promise<BranchCheck> {
  const realRemote = realpathSync(i.remote)
  const tipRes = await gitStatus(realRemote, [
    'rev-parse',
    '--verify',
    '--quiet',
    `refs/heads/${i.branch}`,
  ])

  if (tipRes.status === 1 && tipRes.stdout === '' && tipRes.stderr === '') {
    return { state: 'absent' }
  }
  if (tipRes.status !== 0) {
    throw new Error(tipRes.stderr || `git rev-parse failed with status ${tipRes.status}`)
  }

  const tip = tipRes.stdout
  const ancRes = await gitStatus(realRemote, [
    'merge-base',
    '--is-ancestor',
    i.baseSha,
    tip,
  ])
  if (ancRes.status === 1) {
    return {
      state: 'present',
      sha: tip,
      problem: `base ${i.baseSha.slice(0, 8)} is not an ancestor of ${tip.slice(0, 8)}`,
    }
  }
  if (ancRes.status !== 0) {
    throw new Error(ancRes.stderr || `git merge-base failed with status ${ancRes.status}`)
  }

  const countOut = await git(realRemote, ['rev-list', '--count', `${i.baseSha}..${tip}`])
  if (Number(countOut) === 0) {
    return {
      state: 'present',
      sha: tip,
      problem: 'no commits after base',
    }
  }

  const logOut = await git(realRemote, [
    'log',
    '--format=%H%x00%(trailers:key=Run-Id,valueonly,separator=%x2C)',
    `${i.baseSha}..${tip}`,
  ])
  for (const line of logOut.split('\n')) {
    if (!line) continue
    const sep = line.indexOf('\x00')
    const sha = sep >= 0 ? line.slice(0, sep) : line
    const trailers = sep >= 0 ? line.slice(sep + 1) : ''
    const values = trailers
      .split(',')
      .map((v) => v.trim())
      .filter(Boolean)
    if (!values.includes(i.runId)) {
      return {
        state: 'present',
        sha,
        problem: `commit ${sha.slice(0, 8)} lacks trailer Run-Id: ${i.runId}`,
      }
    }
  }

  return { state: 'present', sha: tip, problem: null }
}

export function classifyRun(f: Facts): RunVerdict {
  if (f.writer === 'possible') {
    return {
      verdict: 'needs_review',
      reason: 'an agent process may still be writing',
      observedSha: f.remote.state === 'present' ? f.remote.sha : null,
      freeze: true,
    }
  }

  if (f.remote.state === 'error') {
    return { verdict: 'check_failed', message: f.remote.message }
  }

  const acceptedSha = f.accepted
    ? (JSON.parse(f.accepted.payload) as { sha: string }).sha
    : undefined

  if (acceptedSha !== undefined) {
    if (f.remote.state === 'absent') {
      return {
        verdict: 'needs_review',
        reason: `accepted ${acceptedSha.slice(0, 8)} but branch is gone`,
        observedSha: null,
        freeze: false,
      }
    }
    if (f.remote.sha !== acceptedSha) {
      return {
        verdict: 'needs_review',
        reason: `remote tip ${f.remote.sha.slice(0, 8)} differs from accepted ${acceptedSha.slice(0, 8)}`,
        observedSha: f.remote.sha,
        freeze: false,
      }
    }
    if (f.remote.problem !== null) {
      return {
        verdict: 'needs_review',
        reason: f.remote.problem,
        observedSha: f.remote.sha,
        freeze: false,
      }
    }
    return { verdict: 'done', sha: acceptedSha }
  }

  if (f.remote.state === 'present') {
    return {
      verdict: 'needs_review',
      reason: 'branch pushed but no accepted result',
      observedSha: f.remote.sha,
      freeze: false,
    }
  }

  return { verdict: 'retryable' }
}
