import { expect, test } from '@playwright/test'
import { MseSink } from './mseSink'

/// A SourceBuffer whose `buffered` throws once its MediaSource is detached,
/// as Chromium's does. Listeners run synchronously, so an exception in one
/// reaches the caller of the event.
class FakeSourceBuffer {
  updating = false
  removed = false
  private readonly listeners: (() => void)[] = []

  get buffered(): TimeRanges {
    if (this.removed) {
      throw new DOMException('This SourceBuffer has been removed from the parent media source.', 'InvalidStateError')
    }
    return { length: 0, start: () => 0, end: () => 0 } as TimeRanges
  }

  addEventListener(_type: 'updateend', listener: () => void): void {
    this.listeners.push(listener)
  }

  appendBuffer(): void {
    this.updating = true
  }

  /// Detaching aborts an append in progress, which ends with `updateend`.
  remove(): void {
    this.removed = true
    if (this.updating) {
      this.updating = false
      for (const listener of this.listeners) {
        listener()
      }
    }
  }
}

class FakeMediaSource extends EventTarget {
  static opened: FakeMediaSource | undefined
  readyState: ReadyState = 'closed'
  readonly sourceBuffers: FakeSourceBuffer[] = []

  constructor() {
    super()
    FakeMediaSource.opened = this
  }

  addSourceBuffer(): FakeSourceBuffer {
    const sourceBuffer = new FakeSourceBuffer()
    this.sourceBuffers.push(sourceBuffer)
    return sourceBuffer
  }

  open(): void {
    this.readyState = 'open'
    this.dispatchEvent(new Event('sourceopen'))
  }

  detach(): void {
    this.readyState = 'closed'
    for (const sourceBuffer of this.sourceBuffers) {
      sourceBuffer.remove()
    }
  }
}

/// A video element that opens the MediaSource its `src` names and detaches
/// it on `load()` without one.
function fakeElement(): HTMLVideoElement {
  return {
    srcObject: null,
    muted: false,
    currentTime: 0,
    set src(_url: string) {
      queueMicrotask(() => FakeMediaSource.opened!.open())
    },
    removeAttribute: () => undefined,
    load: () => FakeMediaSource.opened!.detach(),
    play: async () => undefined
  } as unknown as HTMLVideoElement
}

test.beforeAll(() => {
  Object.assign(globalThis, { MediaSource: FakeMediaSource })
  URL.createObjectURL = () => 'blob:media-source'
  URL.revokeObjectURL = () => undefined
})

test('closing a sink while it appends leaves the aborted append alone', async () => {
  // Arrange: the init segment is still being appended
  const sink = await MseSink.open(fakeElement(), {
    video: { mimeType: 'video/mp4; codecs="avc1.42C01F"', initSegment: new Uint8Array([0]) }
  })

  // Act
  const close = () => sink.close()

  // Assert
  expect(close).not.toThrow()
  expect(sink.bufferedEnd()).toBeUndefined()
})
