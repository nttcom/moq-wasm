/// The bots of a room, which the temporary-apps service starts, reports and stops on `bots` for the
/// page of a temporary app (docs/architecture/bots/architecture.md).

import { temporaryAppToken, temporaryAppsUrl } from './access'
import { icon } from './icon'

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

export function sameNamespace(left: string[], right: string[]): boolean {
  return left.join('/') === right.join('/')
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

type BotPresence = 'absent' | 'starting' | 'present'

const BOT_BUTTON_LABELS: Record<BotPresence, string> = {
  absent: 'bot を参加させる',
  starting: 'bot 起動中…',
  present: 'bot 参加中'
}

/// The button that starts the room's bot of `type` for the page of a temporary app. It can be pressed
/// while the page is in the room and the bot is not, says the bot is starting until its broadcast
/// appears, and shows in `notice` why no bot could be started.
export class RoomBotButton {
  private inRoom = false
  private presence: BotPresence = 'absent'

  constructor(
    private readonly type: string,
    private readonly room: string[],
    private readonly button: HTMLButtonElement,
    private readonly notice: HTMLElement
  ) {
    notice.style.display = 'none'
    button.addEventListener('click', () => void this.start())
    this.render()
  }

  /// Call when the page joins or leaves the room. Leaving also forgets the bot and hides the notice.
  setInRoom(inRoom: boolean): void {
    this.inRoom = inRoom
    if (!inRoom) {
      this.presence = 'absent'
      this.notice.style.display = 'none'
    }
    this.render()
  }

  /// Call when the bot's broadcast is announced in the room or withdrawn.
  setBotPresent(present: boolean): void {
    this.presence = present ? 'present' : 'absent'
    this.render()
  }

  private async start(): Promise<void> {
    this.presence = 'starting'
    this.notice.style.display = 'none'
    this.render()
    try {
      await startBot(this.type, this.room.join('/'), {})
    } catch (error) {
      if (!this.inRoom) {
        return
      }
      if (this.presence === 'starting') {
        this.presence = 'absent'
      }
      // 409: the room already runs one. It either appears in the room or is leaving it, and once it
      // has left the button starts another.
      if (!(error instanceof BotsError && error.status === 409)) {
        this.notice.textContent =
          error instanceof BotsError && error.status === 503
            ? '空いているボットがありません'
            : `ボットを起動できません: ${error instanceof Error ? error.message : String(error)}`
        this.notice.style.display = ''
      }
      this.render()
    }
  }

  private render(): void {
    this.button.disabled = !this.inRoom || this.presence !== 'absent'
    this.button.replaceChildren(icon('plus'), BOT_BUTTON_LABELS[this.presence])
  }
}
