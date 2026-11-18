/// The bots of a room, which the temporary-apps service starts, reports and stops on `bots` for the
/// page of a temporary app (docs/architecture/bots/architecture.md).

import { temporaryAppToken, temporaryAppsUrl } from './access'

export type BotState = 'starting' | 'running' | 'stopping' | 'finished' | 'failed'

export interface RoomBot {
  id: string
  state: BotState
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

/// The namespace a bot of `botType` publishes at in `room` (docs/architecture/bots/architecture.md
/// "Bots in the room").
export function botNamespace(room: string[], botType: string): string[] {
  return [...room, 'bots', botType]
}

/// The room's latest bot of `type`, or `undefined` if it never had one or the temporary-apps service
/// has no `bots`.
export async function latestBot(type: string, namespacePrefix: string): Promise<RoomBot | undefined> {
  const query = new URLSearchParams({ namespace_prefix: namespacePrefix })
  const response = await send(`/bots/${type}?${query}`, {})
  return response.status === 404 ? undefined : answer<RoomBot>(response)
}

export async function startBot(type: string, namespacePrefix: string, options: object): Promise<RoomBot> {
  return answer(
    await send(`/bots/${type}`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ namespace_prefix: namespacePrefix, options })
    })
  )
}

/// The bot stops at its next heartbeat.
export async function stopBot(type: string, id: string): Promise<void> {
  const response = await send(`/bots/${type}/${encodeURIComponent(id)}`, { method: 'DELETE' })
  if (!response.ok) {
    await answer(response)
  }
}

async function send(path: string, init: RequestInit): Promise<Response> {
  const token = temporaryAppToken()
  if (!token) {
    throw new Error('bots need a temporary app')
  }
  const headers = new Headers(init.headers)
  headers.set('Authorization', `Bearer ${token}`)
  return fetch(`${temporaryAppsUrl()}${path}`, { ...init, headers })
}

async function answer<T>(response: Response): Promise<T> {
  const body = await response.json().catch(() => ({}))
  if (!response.ok) {
    throw new BotsError(body.error ?? `bots answered ${response.status}`, response.status)
  }
  return body as T
}

/// Starts the room's bot of `type` for the page of a temporary app, and shows in `notice` why it could
/// not. A room that already runs one answers 409, which leaves that bot in place.
export async function startRoomBot(type: string, room: string[], notice: HTMLElement): Promise<void> {
  try {
    await startBot(type, room.join('/'), {})
  } catch (error) {
    if (error instanceof BotsError && error.status === 409) {
      return
    }
    notice.textContent =
      error instanceof BotsError && error.status === 503
        ? '空いているボットがありません'
        : `ボットを起動できません: ${error instanceof Error ? error.message : String(error)}`
    notice.style.display = ''
  }
}
