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

export type BotRoomOptions = {
  /// Reaches the bot of a room this request starts, such as the Meeting transcriber's `asr`.
  query?: Record<string, string>
  /// The page's temporary app token, read on every connection; with one, the room runs under the
  /// token's app instead of the public root.
  token?: () => string | undefined
}

/// What `EventSource` waits by default before it reopens a dropped stream.
const RECONNECT_DELAY_MS = 3_000

/// Keeps the room's bot running while the request is open (docs/architecture/ai-bots/on-demand-rooms.md).
/// The stream is read with `fetch` rather than `EventSource`, which cannot send the token in a header.
export class BotRoomRequest {
  private abort: AbortController | undefined
  /// Set from the server's `member` event.
  private holdUrl: ((action: 'leave' | 'stop') => string) | undefined
  private readonly closeOnPageHide = () => this.close()

  constructor(
    private readonly onEnd: (reason: BotRoomEnd) => void,
    private readonly reconnectDelayMs = RECONNECT_DELAY_MS
  ) {}

  open(serviceUrl: string, roomId: string, options: BotRoomOptions = {}): void {
    this.close()
    const roomUrl = `${serviceUrl}/rooms/${encodeURIComponent(roomId)}`
    const abort = new AbortController()
    this.abort = abort
    window.addEventListener('pagehide', this.closeOnPageHide)
    void this.hold(roomUrl, options, abort.signal)
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
    this.abort?.abort()
    this.abort = undefined
    if (this.holdUrl) {
      void postWithoutPreflight(this.holdUrl('leave'))
      this.holdUrl = undefined
    }
  }

  /// Reopens a stream that drops, as `EventSource` does. A response that is not an event stream, such
  /// as the 400 for a query the bot refuses or the 401 for a token it refuses, ends the request.
  private async hold(roomUrl: string, options: BotRoomOptions, signal: AbortSignal): Promise<void> {
    const url = `${roomUrl}?${new URLSearchParams(options.query ?? {})}`
    while (!signal.aborted) {
      const token = options.token?.()
      const response = await fetch(url, {
        headers: token ? { Authorization: `Bearer ${token}` } : {},
        cache: 'no-store',
        signal
      }).catch(() => undefined)
      if (signal.aborted) {
        return
      }
      if (response && (!response.ok || !response.body)) {
        this.end('failed')
        return
      }
      const reason = response?.body ? await this.readEvents(response.body, roomUrl, signal) : undefined
      if (signal.aborted) {
        return
      }
      if (reason) {
        this.end(reason)
        return
      }
      await new Promise((resolve) => {
        const timer = setTimeout(resolve, this.reconnectDelayMs)
        signal.addEventListener('abort', () => {
          clearTimeout(timer)
          resolve(undefined)
        })
      })
    }
  }

  /// The reason the server ended the room with, or `undefined` once the stream drops.
  private async readEvents(
    body: ReadableStream<Uint8Array>,
    roomUrl: string,
    signal: AbortSignal
  ): Promise<BotRoomEnd | undefined> {
    const reader = body.getReader()
    const decoder = new TextDecoder()
    signal.addEventListener('abort', () => void reader.cancel().catch(() => undefined))
    let buffered = ''
    try {
      for (;;) {
        const { value, done } = await reader.read()
        if (done) {
          return undefined
        }
        buffered += decoder.decode(value, { stream: true })
        let boundary: number
        while ((boundary = buffered.indexOf('\n\n')) >= 0) {
          const { event, data } = parseEvent(buffered.slice(0, boundary))
          buffered = buffered.slice(boundary + 2)
          if (event === 'member') {
            const member = new URLSearchParams({ member: data })
            this.holdUrl = (action) => `${roomUrl}/${action}?${member}`
          } else if (event && event in BOT_ROOM_END_TEXT) {
            return event as BotRoomEnd
          }
        }
      }
    } catch {
      return undefined
    }
  }

  private end(reason: BotRoomEnd): void {
    this.close()
    this.onEnd(reason)
  }
}

/// One event of the stream; comment lines, such as the server's heartbeat, have neither field.
function parseEvent(block: string): { event: string | undefined; data: string } {
  let event: string | undefined
  let data = ''
  for (const line of block.split('\n')) {
    if (line.startsWith('event:')) {
      event = line.slice('event:'.length).trim()
    } else if (line.startsWith('data:')) {
      data = line.slice('data:'.length).trim()
    }
  }
  return { event, data }
}

/// Without a body or headers the POST needs no CORS preflight; keepalive outlives a closing tab.
function postWithoutPreflight(url: string): Promise<unknown> {
  return fetch(url, { method: 'POST', mode: 'no-cors', keepalive: true }).catch(() => undefined)
}
