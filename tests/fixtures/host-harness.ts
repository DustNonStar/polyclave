import { EventEmitter } from 'node:events'
import { mkdirSync, realpathSync } from 'node:fs'
import { join } from 'node:path'
import { createInterface } from 'node:readline'
import { fileURLToPath } from 'node:url'
import { prepareCodexHome } from '../../src/codex.ts'
import { checkRunBranch } from '../../src/git-effect.ts'
import { startHostMcp } from '../../src/mcp.ts'
import { acquireHostLock, bootTime, HostRunningError } from '../../src/proc.ts'
import { recover } from '../../src/recovery.ts'
import { TurnRunner } from '../../src/runner.ts'
import { Runs, type Resolution } from '../../src/runs.ts'
import { Store } from '../../src/store.ts'

const FAKE_CODEX = [
  process.execPath,
  fileURLToPath(new URL('./fake-codex.ts', import.meta.url)),
]

async function main(): Promise<void> {
  const [stateDirArg, command, ...args] = process.argv.slice(2)
  if (!stateDirArg || !command) {
    process.stderr.write('usage: host-harness <stateDir> <command> [args]\n')
    process.exit(2)
  }

  mkdirSync(stateDirArg, { recursive: true })
  const stateDir = realpathSync(stateDirArg)

  let lock: ReturnType<typeof acquireHostLock>
  try {
    lock = acquireHostLock(stateDir)
  } catch (err) {
    if (err instanceof HostRunningError) {
      process.stderr.write(err.message + '\n')
      process.exit(3)
    }
    throw err
  }

  const store = new Store(join(stateDir, 'polyclave.sqlite'))
  const codexHome = prepareCodexHome(
    stateDir,
    process.env.POLYCLAVE_TEST_USER_CODEX_HOME,
  )
  const mcp = await startHostMcp({
    store,
    hostInstanceId: lock.instanceId,
    check: checkRunBranch,
  })
  mcp.setOpen(false)

  const bus = new EventEmitter()
  const runner = new TurnRunner({
    store,
    mcpUrl: mcp.url,
    codexCommand: FAKE_CODEX,
    codexHome,
    hostInstanceId: lock.instanceId,
    bus,
  })
  runner.setDispatch(false)

  const runs = new Runs({
    store,
    runner,
    stateDir,
    check: checkRunBranch,
    bus,
  })

  const report = await recover({
    store,
    hostInstanceId: lock.instanceId,
    bootTime: bootTime(),
    check: checkRunBranch,
  })

  mcp.setOpen(true)
  runner.setDispatch(true)
  runner.drainQueues()

  const shutdownAll = async () => {
    await runs.idle()
    await runner.shutdown()
    await mcp.close()
    lock.release()
    store.close()
  }

  try {
    if (command === 'recover') {
      process.stdout.write(
        JSON.stringify({
          report,
          runs: store.listRuns(),
          sessions: store.listSessions(),
        }) + '\n',
      )
      await shutdownAll()
      return
    }

    if (command === 'start-run') {
      const [workDir, requestId, ...instrParts] = args
      if (!workDir || !requestId || instrParts.length === 0) {
        throw new Error('usage: start-run <workDir> <requestId> <instructions>')
      }
      const realWork = realpathSync(workDir)
      let session = store.listSessions().find((s) => s.cwd === realWork)
      if (!session) {
        session = store.createSession({ name: 'harness', cwd: realWork })
      }
      const { run } = await runs.create({
        sessionId: session.id,
        requestId,
        instructions: instrParts.join(' '),
      })
      const finished = await runs.start(run.id)
      process.stdout.write(JSON.stringify({ run: finished }) + '\n')
      await shutdownAll()
      return
    }

    if (command === 'retry') {
      const [runId] = args
      if (!runId) throw new Error('usage: retry <runId>')
      await runs.retry(runId)
      await runs.idle()
      process.stdout.write(JSON.stringify(store.getRun(runId)) + '\n')
      await shutdownAll()
      return
    }

    if (command === 'resolve') {
      const [runId, decision, note, sha] = args
      if (!runId || !decision || !note) {
        throw new Error('usage: resolve <runId> <decision> <note> [sha]')
      }
      let resolution: Resolution
      if (decision === 'accept') {
        const current = store.getRun(runId)
        resolution = {
          decision: 'accept',
          sha: sha ?? current?.observedSha ?? '',
          note,
        }
      } else if (decision === 'fail' || decision === 'allow_retry') {
        resolution = { decision, note }
      } else {
        throw new Error(`unknown decision ${decision}`)
      }
      const updated = await runs.resolve(runId, resolution)
      process.stdout.write(JSON.stringify(updated) + '\n')
      await shutdownAll()
      return
    }

    if (command === 'unfreeze') {
      const [sessionId, ...noteParts] = args
      if (!sessionId) throw new Error('usage: unfreeze <sessionId> [note]')
      const note = noteParts.join(' ') || 'resolved'
      store.unfreezeSession(sessionId)
      store.addMessage({
        sessionId,
        turnId: null,
        source: 'host',
        text: `Unfrozen by user: ${note}`,
      })
      runner.drainQueues()
      await shutdownAll()
      process.stdout.write(JSON.stringify(store.getSession(sessionId)) + '\n')
      return
    }

    if (command === 'serve') {
      process.stdout.write(
        JSON.stringify({ ready: true, mcpUrl: mcp.url, report }) + '\n',
      )

      const rl = createInterface({ input: process.stdin, crlfDelay: Infinity })
      const inflightOps = new Set<Promise<void>>()

      const handleOp = async (msg: Record<string, unknown>) => {
        const id = msg.id
        const op = msg.op as string
        try {
          let result: unknown
          if (op === 'create-session') {
            const cwd = realpathSync(String(msg.cwd))
            const session = store.createSession({
              name: String(msg.name ?? 'harness'),
              cwd,
            })
            result = { session }
          } else if (op === 'create-run') {
            let sessionId = msg.sessionId as string | undefined
            if (!sessionId) {
              const cwd = msg.cwd ? realpathSync(String(msg.cwd)) : undefined
              const existing = cwd
                ? store.listSessions().find((s) => s.cwd === cwd)
                : store.listSessions()[0]
              if (existing) {
                sessionId = existing.id
              } else if (cwd) {
                sessionId = store.createSession({ name: 'harness', cwd }).id
              } else {
                throw new Error('sessionId or cwd required')
              }
            }
            result = await runs.create({
              sessionId,
              requestId: String(msg.requestId),
              instructions: String(msg.instructions),
              base: msg.base !== undefined ? String(msg.base) : undefined,
            })
          } else if (op === 'start-run') {
            const run = await runs.start(String(msg.runId))
            result = { run }
          } else if (op === 'retry') {
            const run = await runs.retry(String(msg.runId))
            result = { run }
          } else if (op === 'idle') {
            await runs.idle()
            await runner.idle()
            result = { ok: true }
          } else if (op === 'resolve') {
            const decision = String(msg.decision)
            const note = String(msg.note ?? '')
            const resolution: Resolution =
              decision === 'accept'
                ? { decision: 'accept', sha: String(msg.sha ?? ''), note }
                : { decision: decision as 'fail' | 'allow_retry', note }
            const run = await runs.resolve(String(msg.runId), resolution)
            result = { run }
          } else if (op === 'unfreeze') {
            const sessionId = String(msg.sessionId)
            const note = String(msg.note ?? 'resolved')
            store.unfreezeSession(sessionId)
            store.addMessage({
              sessionId,
              turnId: null,
              source: 'host',
              text: `Unfrozen by user: ${note}`,
            })
            runner.drainQueues()
            result = { session: store.getSession(sessionId) }
          } else if (op === 'send') {
            const message = runner.sendUser(String(msg.sessionId), String(msg.text))
            result = { message }
          } else if (op === 'get-state') {
            const sessions = store.listSessions()
            result = {
              sessions,
              runs: store.listRuns(),
              messages: sessions.flatMap((s) => store.listMessages(s.id)),
            }
          } else {
            throw new Error(`unknown op ${op}`)
          }
          process.stdout.write(JSON.stringify({ id, ok: true, result }) + '\n')
        } catch (err) {
          process.stdout.write(
            JSON.stringify({
              id,
              ok: false,
              error: err instanceof Error ? err.message : String(err),
              errorType: err instanceof Error ? err.constructor.name : 'Error',
            }) + '\n',
          )
        }
      }

      for await (const line of rl) {
        const trimmed = line.trim()
        if (!trimmed) continue
        const msg = JSON.parse(trimmed) as Record<string, unknown>
        const p = handleOp(msg)
        inflightOps.add(p)
        void p.finally(() => inflightOps.delete(p))
      }

      await Promise.allSettled([...inflightOps])
      await shutdownAll()
      return
    }

    throw new Error(`unknown command ${command}`)
  } catch (err) {
    process.stderr.write((err instanceof Error ? err.stack ?? err.message : String(err)) + '\n')
    await shutdownAll()
    process.exit(1)
  }
}

main().catch((err) => {
  process.stderr.write((err instanceof Error ? err.stack ?? err.message : String(err)) + '\n')
  process.exit(1)
})
