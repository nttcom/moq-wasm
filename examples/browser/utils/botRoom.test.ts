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

class FakeEventSource extends EventTarget {
  static readonly CONNECTING = 0
  static readonly CLOSED = 2
  static last: FakeEventSource | undefined
  readyState = FakeEventSource.CONNECTING

  constructor(readonly url: string) {
    super()
    FakeEventSource.last = this
  }

  close(): void {
    this.readyState = FakeEventSource.CLOSED
  }

  fail(readyState: number): void {
    this.readyState = readyState
    this.dispatchEvent(new Event('error'))
  }
}

type FakeBrowser = { window: EventTarget; posts: { url: string; init: RequestInit | undefined }[] }

async function withFakeBrowser(act: (browser: FakeBrowser) => void | Promise<void>): Promise<void> {
  const globals = globalThis as unknown as Record<string, unknown>
  const originals = { EventSource: globals.EventSource, window: globals.window, fetch: globals.fetch }
  const browser: FakeBrowser = { window: new EventTarget(), posts: [] }
  globals.EventSource = FakeEventSource
  globals.window = browser.window
  globals.fetch = async (url: string, init?: RequestInit) => {
    browser.posts.push({ url, init })
    return new Response(null, { status: 204 })
  }
  try {
    await act(browser)
  } finally {
    Object.assign(globals, originals)
  }
}

test('a request the server refuses ends as failed, while a dropped stream keeps reconnecting', async () => {
  await withFakeBrowser(() => {
    // Arrange
    const ends: BotRoomEnd[] = []
    const request = new BotRoomRequest((reason) => ends.push(reason))
    request.open('http://bots.example', 'team 1', { asr: 'qwen3-asr' })
    const source = FakeEventSource.last!

    // Act
    source.fail(FakeEventSource.CONNECTING)
    const afterDrop = [...ends]
    source.fail(FakeEventSource.CLOSED)

    // Assert
    expect(source.url).toBe('http://bots.example/rooms/team%201?asr=qwen3-asr')
    expect(afterDrop).toEqual([])
    expect(ends).toEqual(['failed'])
  })
})

test('closing tells the server once that the page leaves the hold it named', async () => {
  await withFakeBrowser((browser) => {
    // Arrange
    const request = new BotRoomRequest(() => undefined)
    request.open('http://bots.example', 'team 1')
    const source = FakeEventSource.last!
    source.dispatchEvent(new MessageEvent('member', { data: 'm-1' }))

    // Act
    request.close()
    request.close()

    // Assert
    expect(source.readyState).toBe(FakeEventSource.CLOSED)
    expect(browser.posts.map(({ url, init }) => [url, init?.method, init?.keepalive])).toEqual([
      ['http://bots.example/rooms/team%201/leave?member=m-1', 'POST', true]
    ])
  })
})

test('a page that is hidden for good closes its request', async () => {
  await withFakeBrowser((browser) => {
    // Arrange
    const request = new BotRoomRequest(() => undefined)
    request.open('http://bots.example', 'team-1')
    FakeEventSource.last!.dispatchEvent(new MessageEvent('member', { data: 'm-1' }))

    // Act
    browser.window.dispatchEvent(new Event('pagehide'))

    // Assert
    expect(FakeEventSource.last!.readyState).toBe(FakeEventSource.CLOSED)
    expect(browser.posts).toHaveLength(1)
  })
})

test('stopping asks the server to stop the room while the page still holds it', async () => {
  await withFakeBrowser(async (browser) => {
    // Arrange
    const request = new BotRoomRequest(() => undefined)
    request.open('http://bots.example', 'team-1')
    const source = FakeEventSource.last!
    source.dispatchEvent(new MessageEvent('member', { data: 'm-1' }))

    // Act
    const stopped = request.stop()
    const openWhileStopping = source.readyState === FakeEventSource.CONNECTING
    await stopped

    // Assert
    expect(openWhileStopping).toBe(true)
    expect(browser.posts.map(({ url }) => url)).toEqual([
      'http://bots.example/rooms/team-1/stop?member=m-1',
      'http://bots.example/rooms/team-1/leave?member=m-1'
    ])
  })
})
