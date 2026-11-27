import {
  classifyRun,
  type BranchCheck,
  type checkRunBranch,
} from './git-effect.ts'
import { groupLiveness, killGroup } from './proc.ts'
import type { RunRow, Store, TurnRow } from './store.ts'

export interface RecoveryReport {
  killedGroups: number[]
  unverifiableGroups: number[]
  turns: {
    id: number
    sessionId: string
    outcome: 'never_started' | 'killed' | 'already_gone' | 'unverifiable'
  }[]
  frozenSessions: string[]
  runs: {
    id: string
    outcome: 'done' | 'retryable' | 'needs_review' | 'check_failed' | 'unchanged'
  }[]
}

function resolutionTimestamp(resolution: string | null): number | null {
  if (!resolution) return null
  const m = /\(seen \S+ at ([^)]+)\)$/.exec(resolution)
  if (!m) return null
  const ms = Date.parse(m[1]!)
  return Number.isFinite(ms) ? ms : null
}

function hasUnfreezeAfter(store: Store, sessionId: string, turn: TurnRow | undefined): boolean {
  if (!turn) return false
  return store.listMessages(sessionId).some((m) => {
    if (m.source !== 'host') return false
    if (!m.text.startsWith('Unfrozen by user:')) return false
    if (m.turnId !== null) return m.turnId > turn.id
    return turn.endedAt !== null && m.at >= turn.endedAt
  })
}

async function safeCheck(
  check: typeof checkRunBranch,
  run: RunRow,
): Promise<BranchCheck | { state: 'error'; message: string }> {
  try {
    return await check({
      remote: run.remote,
      baseSha: run.baseSha,
      branch: run.branch,
      runId: run.id,
    })
  } catch (err) {
    return {
      state: 'error',
      message: err instanceof Error ? err.message : String(err),
    }
  }
}

export async function recover(d: {
  store: Store
  hostInstanceId: string
  bootTime: string
  check: typeof checkRunBranch
  liveness?: typeof groupLiveness
}): Promise<RecoveryReport> {
  const liveness = d.liveness ?? groupLiveness
  const killedGroups: number[] = []
  const unverifiableGroups: number[] = []
  const turns: RecoveryReport['turns'] = []
  const frozenSessions: string[] = []
  const frozenSet = new Set<string>()
  const runs: RecoveryReport['runs'] = []

  const addFrozen = (sessionId: string) => {
    if (!frozenSet.has(sessionId)) {
      frozenSet.add(sessionId)
      frozenSessions.push(sessionId)
    }
  }

  for (const turn of d.store.runningTurns()) {
    if (turn.hostInstanceId === d.hostInstanceId) continue

    if (turn.phase === 'prepared') {
      d.store.endTurn(turn.id, 'interrupted', {
        exitCode: null,
        endReason: 'never_started',
        writerCheck: 'excluded',
      })
      turns.push({ id: turn.id, sessionId: turn.sessionId, outcome: 'never_started' })
      continue
    }

    if (turn.bootTime !== d.bootTime) {
      d.store.endTurn(turn.id, 'interrupted', {
        exitCode: null,
        endReason: 'host_lost',
        writerCheck: 'excluded',
      })
      turns.push({ id: turn.id, sessionId: turn.sessionId, outcome: 'already_gone' })
      continue
    }

    const liv =
      turn.agentPgid !== null ? liveness(turn.agentPgid, turn.agentStartedAt) : 'unverifiable'

    let outcome: 'killed' | 'already_gone' | 'unverifiable'
    if (liv === 'live') {
      await killGroup(turn.agentPgid!)
      killedGroups.push(turn.agentPgid!)
      outcome = 'killed'
    } else if (liv === 'gone') {
      outcome = 'already_gone'
    } else {
      if (turn.agentPgid !== null) {
        unverifiableGroups.push(turn.agentPgid)
      }
      outcome = 'unverifiable'
    }

    const sessBefore = d.store.getSession(turn.sessionId)
    d.store.endTurn(turn.id, 'interrupted', {
      exitCode: null,
      endReason: 'host_lost',
      writerCheck: 'possible',
      freezeReason: `host stopped while an agent was running (turn ${turn.id})`,
    })
    if (sessBefore && sessBefore.frozenAt === null) {
      addFrozen(turn.sessionId)
    }
    turns.push({ id: turn.id, sessionId: turn.sessionId, outcome })
  }

  // Rule 1a: freeze any unfrozen session whose latest turn has writer='possible' on current boot without an unfreeze after it
  for (const session of d.store.listSessions()) {
    if (session.frozenAt !== null) continue
    const last = d.store.lastTurn(session.id)
    if (!last || last.writerCheck !== 'possible' || last.bootTime !== d.bootTime) continue
    if (hasUnfreezeAfter(d.store, session.id, last)) continue
    d.store.freezeSession(
      session.id,
      `host stopped while an agent was running (turn ${last.id})`,
    )
    addFrozen(session.id)
  }

  // Rule 2: classify active/interrupted runs with no running turn
  const activeRunning = d.store.runningTurns()
  for (const run of d.store.listRuns()) {
    if (run.status !== 'pending' && run.status !== 'running' && run.status !== 'interrupted') {
      continue
    }
    if (activeRunning.some((t) => t.runId === run.id || t.sessionId === run.sessionId)) {
      continue
    }

    const lastTurn = d.store.lastRunTurn(run.id)
    let writer = lastTurn?.writerCheck ?? 'excluded'
    if (writer === 'possible' && lastTurn?.endedAt) {
      const resolvedAt = resolutionTimestamp(run.resolution)
      const endedAt = Date.parse(lastTurn.endedAt)
      if (resolvedAt !== null && Number.isFinite(endedAt) && resolvedAt >= endedAt) {
        writer = 'excluded'
      }
    }
    const remote = await safeCheck(d.check, run)
    const verdict = classifyRun({
      writer,
      accepted: d.store.acceptedResult(run.id),
      remote,
    })

    if (verdict.verdict === 'done') {
      d.store.updateRun(
        run.id,
        {
          status: 'done',
          resultSha: verdict.sha,
          observedSha: verdict.sha,
          resultSource: 'agent',
          detail: null,
        },
        ['pending', 'running', 'interrupted'],
      )
      runs.push({ id: run.id, outcome: 'done' })
    } else if (verdict.verdict === 'needs_review') {
      d.store.updateRun(
        run.id,
        {
          status: 'needs_review',
          detail: verdict.reason,
          observedSha: verdict.observedSha,
        },
        ['pending', 'running', 'interrupted'],
      )
      if (verdict.freeze) {
        const sess = d.store.getSession(run.sessionId)
        if (sess && sess.frozenAt === null && !hasUnfreezeAfter(d.store, run.sessionId, lastTurn)) {
          d.store.freezeSession(
            run.sessionId,
            lastTurn
              ? `host stopped while an agent was running (turn ${lastTurn.id})`
              : verdict.reason,
          )
          addFrozen(run.sessionId)
        }
      }
      runs.push({ id: run.id, outcome: 'needs_review' })
    } else if (verdict.verdict === 'check_failed') {
      const detail = `remote check failed: ${verdict.message}`
      if (run.status === 'interrupted' && run.detail === detail) {
        runs.push({ id: run.id, outcome: 'unchanged' })
      } else {
        d.store.updateRun(
          run.id,
          { status: 'interrupted', detail },
          ['pending', 'running', 'interrupted'],
        )
        runs.push({ id: run.id, outcome: 'check_failed' })
      }
    } else {
      const detail = 'not on the remote; safe to retry'
      if (run.status === 'interrupted' && run.detail === detail) {
        runs.push({ id: run.id, outcome: 'unchanged' })
      } else {
        d.store.updateRun(
          run.id,
          { status: 'interrupted', detail },
          ['pending', 'running', 'interrupted'],
        )
        runs.push({ id: run.id, outcome: 'retryable' })
      }
    }
  }

  return {
    killedGroups,
    unverifiableGroups,
    turns,
    frozenSessions,
    runs,
  }
}
