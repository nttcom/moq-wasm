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
  /// Set from the server's `member` event; `close` POSTs to it.
  private leaveUrl: string | undefined
  private readonly closeOnPageHide = () => this.close()

  constructor(private readonly onEnd: (reason: BotRoomEnd) => void) {}

  /// `query` reaches the bot of a room this request starts, such as the Meeting transcriber's `asr`.
  open(serviceUrl: string, roomId: string, query: Record<string, string> = {}): void {
    this.close()
    const roomUrl = `${serviceUrl}/rooms/${encodeURIComponent(roomId)}`
    const source = new EventSource(`${roomUrl}?${new URLSearchParams(query)}`)
    source.addEventListener('member', (event) => {
      this.leaveUrl = `${roomUrl}/leave?${new URLSearchParams({ member: (event as MessageEvent<string>).data })}`
    })
    for (const reason of Object.keys(BOT_ROOM_END_TEXT) as BotRoomEnd[]) {
      source.addEventListener(reason, () => {
        // EventSource reopens any stream the server ends.
        this.close()
        this.onEnd(reason)
      })
    }
    // A response that is not an event stream, such as the 400 for a query the bot refuses, closes
    // the source for good; a dropped stream leaves it reconnecting.
    source.addEventListener('error', () => {
      if (source.readyState === EventSource.CLOSED) {
        this.close()
        this.onEnd('failed')
      }
    })
    this.source = source
    window.addEventListener('pagehide', this.closeOnPageHide)
  }

  /// Closes the stream and tells the server the page leaves, since a server behind a proxy such as
  /// Cloud Run's does not see the stream close. The bot then leaves at once if no other page holds
  /// the room.
  close(): void {
    window.removeEventListener('pagehide', this.closeOnPageHide)
    this.source?.close()
    this.source = undefined
    if (this.leaveUrl) {
      // Without a body or headers the POST needs no CORS preflight; keepalive outlives a closing tab.
      void fetch(this.leaveUrl, { method: 'POST', mode: 'no-cors', keepalive: true }).catch(() => undefined)
      this.leaveUrl = undefined
    }
  }
}
