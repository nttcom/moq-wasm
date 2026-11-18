/// The recorder bot of a room, which the temporary-apps service starts, reports and stops on `bots`
/// for the page of a temporary app (docs/architecture/bots/architecture.md).

import type { RecordingFormat, RecordingLayout } from './recorderApi'

type BotState = 'starting' | 'running' | 'stopping' | 'finished' | 'failed'

export interface RecorderBot {
  id: string
  state: BotState
}

/// A recording the room's recorder bot is making, as the header and the controls show it.
export interface ActiveRecording {
  id: string
  state: 'starting' | 'recording' | 'stopping'
}

/// The temporary-apps service and the page's temporary app token, read on every request so a
/// refreshed one is used; the token is `undefined` outside a temporary app.
export interface BotsApi {
  url: string
  token: () => string | undefined
}

export interface RecordingOptions {
  layout: RecordingLayout
  composite: boolean
  format: RecordingFormat
}

export function activeRecording(bot: RecorderBot | undefined): ActiveRecording | undefined {
  switch (bot?.state) {
    case 'starting':
    case 'stopping':
      return { id: bot.id, state: bot.state }
    case 'running':
      return { id: bot.id, state: 'recording' }
    default:
      return undefined
  }
}

/// The room's latest recorder bot, or `undefined` if it never had one or the temporary-apps service
/// has no `bots`.
export async function latestRecorderBot(api: BotsApi, namespacePrefix: string): Promise<RecorderBot | undefined> {
  const query = new URLSearchParams({ namespace_prefix: namespacePrefix })
  const response = await send(api, `/bots/recorder?${query}`, {})
  return response.status === 404 ? undefined : answer<RecorderBot>(response)
}

export async function startRecorderBot(
  api: BotsApi,
  namespacePrefix: string,
  options: RecordingOptions
): Promise<RecorderBot> {
  return answer(
    await send(api, '/bots/recorder', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ namespace_prefix: namespacePrefix, options })
    })
  )
}

/// The bot stops at its next heartbeat.
export async function stopRecorderBot(api: BotsApi, id: string): Promise<void> {
  const response = await send(api, `/bots/recorder/${encodeURIComponent(id)}`, { method: 'DELETE' })
  if (!response.ok) {
    await answer(response)
  }
}

async function send(api: BotsApi, path: string, init: RequestInit): Promise<Response> {
  const token = api.token()
  if (!token) {
    throw new Error('recording needs a temporary app')
  }
  const headers = new Headers(init.headers)
  headers.set('Authorization', `Bearer ${token}`)
  return fetch(`${api.url}${path}`, { ...init, headers })
}

async function answer<T>(response: Response): Promise<T> {
  const body = await response.json().catch(() => ({}))
  if (!response.ok) {
    throw new Error(body.error ?? `bots answered ${response.status}`)
  }
  return body as T
}
