import { createServer } from 'node:http'
import type { AddressInfo } from 'node:net'
import { z } from 'zod'
import type { checkRunBranch } from './git-effect.ts'
import type { Store } from './store.ts'

export interface HostMcp {
  url: string
  close(): Promise<void>
  setOpen(open: boolean): void
}

const SubmitSchema = z.strictObject({
  run_id: z.string(),
  branch: z.string(),
  sha: z.string().regex(/^[0-9a-f]{40}$/, 'sha must be a 40-char lowercase hex commit id'),
})

const SUBMIT_JSON_SCHEMA = {
  type: 'object',
  properties: {
    run_id: { type: 'string' },
    branch: { type: 'string' },
    sha: { type: 'string' },
  },
  required: ['run_id', 'branch', 'sha'],
  additionalProperties: false,
}

export async function startHostMcp(d: {
  store: Store
  hostInstanceId: string
  check: typeof checkRunBranch
}): Promise<HostMcp> {
  let open = true

  const server = createServer(async (req, res) => {
    if (req.url !== '/mcp') {
      res.writeHead(404).end()
      return
    }
    if (req.method !== 'POST') {
      res.writeHead(405, { Allow: 'POST' }).end()
      return
    }
    if (!open) {
      res.writeHead(503).end()
      return
    }

    const auth = req.headers.authorization ?? ''
    const token = auth.startsWith('Bearer ') ? auth.slice('Bearer '.length) : ''
    const turn = token ? d.store.turnByToken(token, d.hostInstanceId) : undefined
    if (!turn) {
      res.writeHead(401).end()
      return
    }

    let body = ''
    for await (const chunk of req) {
      body += chunk.toString()
    }

    let msg: {
      id?: unknown
      method?: string
      params?: Record<string, unknown>
    }
    try {
      msg = JSON.parse(body) as typeof msg
    } catch {
      res
        .writeHead(200, { 'Content-Type': 'application/json' })
        .end(
          JSON.stringify({
            jsonrpc: '2.0',
            id: null,
            error: { code: -32700, message: 'Parse error' },
          }),
        )
      return
    }

    if (msg.id === undefined) {
      res.writeHead(202).end()
      return
    }

    const reply = (result: unknown) => {
      res
        .writeHead(200, { 'Content-Type': 'application/json' })
        .end(JSON.stringify({ jsonrpc: '2.0', id: msg.id, result }))
    }

    const replyError = (code: number, message: string) => {
      res
        .writeHead(200, { 'Content-Type': 'application/json' })
        .end(JSON.stringify({ jsonrpc: '2.0', id: msg.id, error: { code, message } }))
    }

    try {
      switch (msg.method) {
        case 'initialize': {
          const params = msg.params as { protocolVersion?: string } | undefined
          reply({
            protocolVersion: params?.protocolVersion ?? '2025-06-18',
            capabilities: { tools: {} },
            serverInfo: { name: 'polyclave-host', version: '0.1.0' },
          })
          return
        }
        case 'ping': {
          reply({})
          return
        }
        case 'tools/list': {
          reply({
            tools: [
              {
                name: 'submit_result',
                description:
                  'Submit the pushed branch and commit sha for the active run. The host verifies the bare repository before accepting.',
                inputSchema: SUBMIT_JSON_SCHEMA,
              },
            ],
          })
          return
        }
        case 'tools/call': {
          if (msg.params?.name !== 'submit_result') {
            replyError(-32601, 'unknown tool')
            return
          }
          const args = msg.params.arguments
          if (turn.runId === null) {
            reply({
              content: [{ type: 'text', text: 'Rejected: this session has no active task' }],
              isError: true,
            })
            return
          }

          const activeRunId = turn.runId
          const rejectRecorded = (reason: string): void => {
            if (!d.store.turnByToken(token, d.hostInstanceId)) {
              res.writeHead(401).end()
              return
            }
            d.store.addRejectedResult({
              runId: activeRunId,
              turnId: turn.id,
              payload: args,
              error: reason,
            })
            reply({
              content: [{ type: 'text', text: `Rejected: ${reason}` }],
              isError: true,
            })
          }

          const parsed = SubmitSchema.safeParse(args)
          if (!parsed.success) {
            const msgText = parsed.error.issues
              .map((iss) => `${iss.path.join('.') || 'arguments'}: ${iss.message}`)
              .join('; ')
            rejectRecorded(msgText)
            return
          }

          const data = parsed.data
          if (data.run_id !== activeRunId) {
            rejectRecorded(`run_id ${data.run_id} is not the active task of this session`)
            return
          }

          const run = d.store.getRun(activeRunId)
          if (!run || data.branch !== run.branch) {
            rejectRecorded(`branch must be ${run?.branch ?? ''}`)
            return
          }

          const checkRes = await d.check({
            remote: run.remote,
            baseSha: run.baseSha,
            branch: run.branch,
            runId: run.id,
          })

          if (checkRes.state === 'absent') {
            rejectRecorded(`branch ${run.branch} is not on the remote`)
            return
          }
          if (checkRes.sha !== data.sha) {
            rejectRecorded(
              `remote ${run.branch} is at ${checkRes.sha}, not ${data.sha}`,
            )
            return
          }
          if (checkRes.problem !== null) {
            rejectRecorded(checkRes.problem)
            return
          }

          const outcome = d.store.settleResult({
            token,
            hostInstanceId: d.hostInstanceId,
            runId: run.id,
            branch: run.branch,
            sha: data.sha,
          })

          switch (outcome) {
            case 'turn_not_active':
              res.writeHead(401).end()
              return
            case 'accepted':
              reply({
                content: [{ type: 'text', text: 'Accepted and stored by the host.' }],
                isError: false,
              })
              return
            case 'already_accepted':
              reply({
                content: [{ type: 'text', text: 'Already accepted.' }],
                isError: false,
              })
              return
            case 'different_result_accepted':
              reply({
                content: [
                  {
                    type: 'text',
                    text: 'Rejected: a different result was already accepted for this task',
                  },
                ],
                isError: true,
              })
              return
          }
        }
        default: {
          replyError(-32601, 'method not found')
          return
        }
      }
    } catch (err) {
      if (!res.headersSent) {
        replyError(-32603, err instanceof Error ? err.message : String(err))
      }
    }
  })

  await new Promise<void>((resolve) => {
    server.listen(0, '127.0.0.1', () => resolve())
  })
  const addr = server.address() as AddressInfo

  return {
    url: `http://127.0.0.1:${addr.port}/mcp`,
    setOpen(v: boolean) {
      open = v
    },
    close() {
      return new Promise<void>((resolve) => {
        server.closeAllConnections?.()
        server.close(() => resolve())
      })
    },
  }
}
