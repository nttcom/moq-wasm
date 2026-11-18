/// The bots of a room, which the temporary-apps service starts, reports and stops on `bots` for the
/// page of a temporary app (docs/architecture/bots/architecture.md).

import { temporaryAppToken, temporaryAppsUrl } from './access'

export type BotState = 'starting' | 'running' | 'stopping' | 'finished' | 'failed'

export interface RoomBot {
  id: string
  state: BotState
}

/// The temporary-apps service and the page's temporary app token, read on every request so a
/// refreshed one is used; the token is `undefined` outside a temporary app.
export interface BotsApi {
  url: string
  token: () => string | undefined
}

/// A refused request: `status` is the HTTP status, such as 409 for a room that already runs a bot of
/// the type.
export class BotsError extends Error {
  constructor(
    message: string,
    readonly status: number
  ) {
    super(message)
  }
}

export function temporaryAppsBots(): BotsApi {
  return { url: temporaryAppsUrl(), token: temporaryAppToken }
}

/// The room's latest bot of `type`, or `undefined` if it never had one or the temporary-apps service
/// has no `bots`.
export async function latestBot(api: BotsApi, type: string, namespacePrefix: string): Promise<RoomBot | undefined> {
  const query = new URLSearchParams({ namespace_prefix: namespacePrefix })
  const response = await send(api, `/bots/${type}?${query}`, {})
  return response.status === 404 ? undefined : answer<RoomBot>(response)
}

export async function startBot(api: BotsApi, type: string, namespacePrefix: string, options: object): Promise<RoomBot> {
  return answer(
    await send(api, `/bots/${type}`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ namespace_prefix: namespacePrefix, options })
    })
  )
}

/// The bot stops at its next heartbeat.
export async function stopBot(api: BotsApi, type: string, id: string): Promise<void> {
  const response = await send(api, `/bots/${type}/${encodeURIComponent(id)}`, { method: 'DELETE' })
  if (!response.ok) {
    await answer(response)
  }
}

async function send(api: BotsApi, path: string, init: RequestInit): Promise<Response> {
  const token = api.token()
  if (!token) {
    throw new Error('bots need a temporary app')
  }
  const headers = new Headers(init.headers)
  headers.set('Authorization', `Bearer ${token}`)
  return fetch(`${api.url}${path}`, { ...init, headers })
}

async function answer<T>(response: Response): Promise<T> {
  const body = await response.json().catch(() => ({}))
  if (!response.ok) {
    throw new BotsError(body.error ?? `bots answered ${response.status}`, response.status)
  }
  return body as T
}
