/// Why moq-bot-rooms ended the page's hold on its room, sent as the event name.
export type BotRoomEnd = 'full' | 'closed' | 'expired' | 'failed' | 'stopped'

export const BOT_ROOM_END_TEXT: Record<BotRoomEnd, string> = {
  full: '空いているボットがありません',
  closed: '平日日中以外はボットを停止しています',
  expired: 'ボットは 1 時間で退出します',
  failed: 'ボットが停止しました',
  stopped: 'ルームの参加者がボットを止めました'
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
  /// Set from the server's `member` event.
  private holdUrl: ((action: 'leave' | 'stop') => string) | undefined
  private readonly closeOnPageHide = () => this.close()

  constructor(private readonly onEnd: (reason: BotRoomEnd) => void) {}

  /// `query` reaches the bot of a room this request starts, such as the Meeting transcriber's `asr`.
  open(serviceUrl: string, roomId: string, query: Record<string, string> = {}): void {
    this.close()
    const roomUrl = `${serviceUrl}/rooms/${encodeURIComponent(roomId)}`
    const source = new EventSource(`${roomUrl}?${new URLSearchParams(query)}`)
    source.addEventListener('member', (event) => {
      const member = new URLSearchParams({ member: (event as MessageEvent<string>).data })
      this.holdUrl = (action) => `${roomUrl}/${action}?${member}`
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

  /// Stops the bot for every page that holds the room, then closes the request; before the server's
  /// `member` event it only closes.
  async stop(): Promise<void> {
    const stopUrl = this.holdUrl?.('stop')
    if (stopUrl) {
      // The server stops only a room this page still holds, so the request closes after the stop.
      await postWithoutPreflight(stopUrl)
    }
    this.close()
  }

  /// Closes the stream and tells the server the page leaves, since a server behind a proxy such as
  /// Cloud Run's does not see the stream close. The bot then leaves at once if no other page holds
  /// the room.
  close(): void {
    window.removeEventListener('pagehide', this.closeOnPageHide)
    this.source?.close()
    this.source = undefined
    if (this.holdUrl) {
      void postWithoutPreflight(this.holdUrl('leave'))
      this.holdUrl = undefined
    }
  }
}

/// Without a body or headers the POST needs no CORS preflight; keepalive outlives a closing tab.
function postWithoutPreflight(url: string): Promise<unknown> {
  return fetch(url, { method: 'POST', mode: 'no-cors', keepalive: true }).catch(() => undefined)
}
