import { randomBytes } from 'node:crypto'

export function sessionId(): string {
  return 's' + randomBytes(4).toString('hex')
}

export function runId(): string {
  return 'r' + randomBytes(4).toString('hex')
}

export function hostInstanceId(): string {
  return randomBytes(8).toString('hex')
}

export function mcpToken(): string {
  return randomBytes(24).toString('hex')
}
