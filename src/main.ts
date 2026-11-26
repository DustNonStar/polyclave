import { EventEmitter } from 'node:events'
import { mkdirSync, realpathSync } from 'node:fs'
import { homedir } from 'node:os'
import { join } from 'node:path'
import { prepareCodexHome } from './codex.ts'
import { checkRunBranch } from './git-effect.ts'
import { startHttpServer } from './http.ts'
import { startHostMcp } from './mcp.ts'
import { acquireHostLock, bootTime, HostRunningError } from './proc.ts'
import { recover } from './recovery.ts'
import { TurnRunner } from './runner.ts'
import { Runs } from './runs.ts'
import { Store } from './store.ts'

function parseArgs(argv: string[]): {
  stateDir: string
  port: number
  codexCommand: string[]
} {
  let stateDir = join(homedir(), '.polyclave')
  let port = 4310
  let codexCommand: string[] = ['codex']

  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i]!
    if (arg === '--state-dir') {
      const val = argv[++i]
      if (!val) throw new Error('--state-dir requires a path')
      stateDir = val
    } else if (arg === '--port') {
      const val = argv[++i]
      const n = Number(val)
      if (val === undefined || !Number.isInteger(n) || n < 0 || n > 65535) {
        throw new Error('--port requires a valid port number')
      }
      port = n
    } else if (arg === '--codex') {
      const cmd: string[] = []
      while (i + 1 < argv.length) {
        const next = argv[i + 1]!
        if (next === '--state-dir' || next === '--port') break
        cmd.push(next)
        i++
      }
      if (cmd.length === 0) throw new Error('--codex requires a command')
      codexCommand = cmd
    } else {
      throw new Error(`unknown flag: ${arg}`)
    }
  }

  return { stateDir, port, codexCommand }
}

async function main(): Promise<void> {
  const opts = parseArgs(process.argv.slice(2))
  mkdirSync(opts.stateDir, { recursive: true })
  const stateDir = realpathSync(opts.stateDir)

  let lock: ReturnType<typeof acquireHostLock>
  try {
    lock = acquireHostLock(stateDir)
  } catch (err) {
    if (err instanceof HostRunningError) {
      process.stderr.write(err.message + '\n')
      process.exit(1)
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
    codexCommand: opts.codexCommand,
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

  await recover({
    store,
    hostInstanceId: lock.instanceId,
    bootTime: bootTime(),
    check: checkRunBranch,
  })

  mcp.setOpen(true)
  runner.setDispatch(true)
  runner.drainQueues()

  const httpServer = await startHttpServer({
    store,
    runner,
    runs,
    bus,
    hostInstanceId: lock.instanceId,
    port: opts.port,
  })

  process.stdout.write(`polyclave listening on ${httpServer.url}\n`)

  let shuttingDown = false
  const handleSignal = () => {
    if (shuttingDown) {
      runner.terminateGroups()
      process.exit(130)
    }
    shuttingDown = true
    const running = store.runningTurns().length
    if (running > 0) {
      process.stderr.write(
        `Waiting for ${running} running turn(s) to finish. Press Ctrl-C again to stop them now.\n`,
      )
    }
    void (async () => {
      await runner.shutdown()
      await runs.idle()
      await httpServer.close()
      await mcp.close()
      lock.release()
      store.close()
      process.exit(0)
    })()
  }

  process.on('SIGINT', handleSignal)
  process.on('SIGTERM', handleSignal)
}

main().catch((err) => {
  process.stderr.write((err instanceof Error ? err.stack ?? err.message : String(err)) + '\n')
  process.exit(1)
})
