import { expect, test } from '@playwright/test'
import { type BotRoomEnd, BotRoomRequest, roomIdFromSearch } from './botRoom'

test('a room ID is taken from the query only when it is one namespace element', () => {
  // Act / Assert
  expect(roomIdFromSearch('?room=team-1')).toBe('team-1')
  expect(roomIdFromSearch('?room=Team')).toBeUndefined()
  expect(roomIdFromSearch('?room=a/b')).toBeUndefined()
  expect(roomIdFromSearch(`?room=${'a'.repeat(33)}`)).toBeUndefined()
  expect(roomIdFromSearch('')).toBeUndefined()
})

/// A room stream the test writes the server's events into.
class FakeStream {
  private controller!: ReadableStreamDefaultController<Uint8Array>
  readonly body = new ReadableStream<Uint8Array>({ start: (controller) => (this.controller = controller) })

  send(text: string): void {
    this.controller.enqueue(new TextEncoder().encode(text))
  }

  drop(): void {
    this.controller.close()
  }
}

type RoomGet = { url: string; authorization: string | null; signal: AbortSignal | undefined; stream: FakeStream }

type FakeBrowser = {
  window: EventTarget
  gets: RoomGet[]
  posts: { url: string; init: RequestInit | undefined }[]
  /// The status the next GET is answered with; 200 answers with a `FakeStream`.
  status: number
}

const RECONNECT_DELAY_MS = 5

async function withFakeBrowser(act: (browser: FakeBrowser) => void | Promise<void>): Promise<void> {
  const globals = globalThis as unknown as Record<string, unknown>
  const originals = { window: globals.window, fetch: globals.fetch }
  const browser: FakeBrowser = { window: new EventTarget(), gets: [], posts: [], status: 200 }
  globals.window = browser.window
  globals.fetch = async (url: string, init?: RequestInit) => {
    if (init?.method === 'POST') {
      browser.posts.push({ url, init })
      return new Response(null, { status: 204 })
    }
    if (browser.status !== 200) {
      return new Response('refused', { status: browser.status })
    }
    const stream = new FakeStream()
    const authorization = new Headers(init?.headers).get('Authorization')
    browser.gets.push({ url, authorization, signal: init?.signal ?? undefined, stream })
    return new Response(stream.body, { headers: { 'Content-Type': 'text/event-stream' } })
  }
  try {
    await act(browser)
  } finally {
    Object.assign(globals, originals)
  }
}

function settle(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, RECONNECT_DELAY_MS * 4))
}

test('a dropped stream reconnects with the current token, and a reason ends the request', async () => {
  await withFakeBrowser(async (browser) => {
    // Arrange
    const ends: BotRoomEnd[] = []
    let token = 'token-1'
    const request = new BotRoomRequest((reason) => ends.push(reason), RECONNECT_DELAY_MS)
    request.open('http://bots.example', 'team 1', { query: { asr: 'qwen3-asr' }, token: () => token })
    await settle()

    // Act
    token = 'token-2'
    browser.gets[0].stream.drop()
    await settle()
    browser.gets[1].stream.send('event: expired\ndata: expired\n\n')
    await settle()

    // Assert
    expect(browser.gets.map(({ url, authorization }) => [url, authorization])).toEqual([
      ['http://bots.example/rooms/team%201?asr=qwen3-asr', 'Bearer token-1'],
      ['http://bots.example/rooms/team%201?asr=qwen3-asr', 'Bearer token-2']
    ])
    expect(ends).toEqual(['expired'])
  })
})

test('a request without a token sends no authorization', async () => {
  await withFakeBrowser(async (browser) => {
    // Arrange
    const request = new BotRoomRequest(() => undefined, RECONNECT_DELAY_MS)

    // Act
    request.open('http://bots.example', 'team-1', { token: () => undefined })
    await settle()

    // Assert
    expect(browser.gets.map(({ authorization }) => authorization)).toEqual([null])
    request.close()
  })
})

test('a request the server refuses ends as failed', async () => {
  await withFakeBrowser(async (browser) => {
    // Arrange
    const ends: BotRoomEnd[] = []
    browser.status = 401
    const request = new BotRoomRequest((reason) => ends.push(reason), RECONNECT_DELAY_MS)

    // Act
    request.open('http://bots.example', 'team-1', { token: () => 'forged' })
    await settle()

    // Assert
    expect(ends).toEqual(['failed'])
  })
})

test('closing tells the server once that the page leaves the hold it named', async () => {
  await withFakeBrowser(async (browser) => {
    // Arrange
    const request = new BotRoomRequest(() => undefined, RECONNECT_DELAY_MS)
    request.open('http://bots.example', 'team 1')
    await settle()
    browser.gets[0].stream.send(': heartbeat\n\nevent: member\ndata: m-1\n\n')
    await settle()

    // Act
    request.close()
    request.close()
    await settle()

    // Assert: the stream is not reopened
    expect(browser.gets[0].signal?.aborted).toBe(true)
    expect(browser.gets).toHaveLength(1)
    expect(browser.posts.map(({ url, init }) => [url, init?.method, init?.keepalive])).toEqual([
      ['http://bots.example/rooms/team%201/leave?member=m-1', 'POST', true]
    ])
  })
})

test('a page that is hidden for good closes its request', async () => {
  await withFakeBrowser(async (browser) => {
    // Arrange
    const request = new BotRoomRequest(() => undefined, RECONNECT_DELAY_MS)
    request.open('http://bots.example', 'team-1')
    await settle()
    browser.gets[0].stream.send('event: member\ndata: m-1\n\n')
    await settle()

    // Act
    browser.window.dispatchEvent(new Event('pagehide'))

    // Assert
    expect(browser.gets[0].signal?.aborted).toBe(true)
    expect(browser.posts).toHaveLength(1)
  })
})

test('stopping asks the server to stop the room while the page still holds it', async () => {
  await withFakeBrowser(async (browser) => {
    // Arrange
    const request = new BotRoomRequest(() => undefined, RECONNECT_DELAY_MS)
    request.open('http://bots.example', 'team-1')
    await settle()
    browser.gets[0].stream.send('event: member\ndata: m-1\n\n')
    await settle()

    // Act
    const stopped = request.stop()
    const openWhileStopping = !browser.gets[0].signal?.aborted
    await stopped

    // Assert
    expect(openWhileStopping).toBe(true)
    expect(browser.posts.map(({ url }) => url)).toEqual([
      'http://bots.example/rooms/team-1/stop?member=m-1',
      'http://bots.example/rooms/team-1/leave?member=m-1'
    ])
  })
})
