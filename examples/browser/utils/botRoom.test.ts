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

test('a request the server refuses ends as failed, while a dropped stream keeps reconnecting', () => {
  // Arrange
  const original = globalThis.EventSource
  globalThis.EventSource = FakeEventSource as unknown as typeof EventSource
  const ends: BotRoomEnd[] = []
  const request = new BotRoomRequest((reason) => ends.push(reason))
  try {
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
  } finally {
    globalThis.EventSource = original
  }
})
