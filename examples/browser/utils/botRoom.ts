/// Why moq-bot-rooms ended the page's hold on its room, sent as the event name.
export type BotRoomEnd = 'full' | 'closed' | 'expired' | 'failed'

export const BOT_ROOM_END_TEXT: Record<BotRoomEnd, string> = {
  full: '空いているボットがありません',
  closed: '平日日中以外はボットを停止しています',
  expired: 'ボットは 1 時間で退出します',
  failed: 'ボットが停止しました'
}

// One namespace element, as moq-bot-rooms accepts it.
const ROOM_ID_PATTERN = /^[a-z0-9-]{1,32}$/

export function roomIdFromSearch(search: string): string | undefined {
  const roomId = new URLSearchParams(search).get('room')?.trim()
  return roomId && ROOM_ID_PATTERN.test(roomId) ? roomId : undefined
}

/// The `room` query parameter, or a new room ID written into the URL so the link can be shared.
export function pageRoomId(): string {
  const url = new URL(window.location.href)
  const given = roomIdFromSearch(url.search)
  if (given) {
    return given
  }
  const created = crypto.randomUUID().slice(0, 8)
  url.searchParams.set('room', created)
  window.history.replaceState(window.history.state, '', url)
  return created
}

/// Keeps the room's bot running while the request is open (docs/architecture/ai-bots/on-demand-rooms.md).
export class BotRoomRequest {
  private source: EventSource | undefined

  constructor(private readonly onEnd: (reason: BotRoomEnd) => void) {}

  open(serviceUrl: string, roomId: string): void {
    this.close()
    const source = new EventSource(`${serviceUrl}/rooms/${roomId}`)
    for (const reason of Object.keys(BOT_ROOM_END_TEXT) as BotRoomEnd[]) {
      source.addEventListener(reason, () => {
        // EventSource reopens any stream the server ends.
        this.close()
        this.onEnd(reason)
      })
    }
    this.source = source
  }

  close(): void {
    this.source?.close()
    this.source = undefined
  }
}
