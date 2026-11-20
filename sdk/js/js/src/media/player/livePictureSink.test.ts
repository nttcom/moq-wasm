import { expect, test } from '@playwright/test'
import { createLivePictureSink } from './livePictureSink'

type FakeFrame = VideoFrame & { closed: boolean }

class FakeVideoElement extends EventTarget {
  srcObject: unknown = null
}

function fakeFrame(): FakeFrame {
  const frame = { closed: false, close: () => (frame.closed = true) }
  return frame as unknown as FakeFrame
}

function arrangeTrackGeneratorSink() {
  const written: VideoFrame[] = []
  const globals = globalThis as Record<string, unknown>
  globals.MediaStreamTrackGenerator = class {
    readonly writable = {
      getWriter: () => ({ desiredSize: 1, write: async (frame: VideoFrame) => void written.push(frame) })
    }
  }
  globals.MediaStream = class {}
  const element = new FakeVideoElement()
  const sink = createLivePictureSink(element as unknown as HTMLVideoElement, {} as HTMLCanvasElement, () => {})
  const uninstall = () => {
    delete globals.MediaStreamTrackGenerator
    delete globals.MediaStream
  }
  return { element, sink, written, uninstall }
}

test('the first frame waits until the video element starts loading the stream', () => {
  // Arrange
  const { element, sink, written, uninstall } = arrangeTrackGeneratorSink()
  const first = fakeFrame()

  try {
    // Act
    sink.present(first)
    const writtenBeforeLoading = [...written]
    element.dispatchEvent(new Event('loadstart'))

    // Assert
    expect(element.srcObject).not.toBeNull()
    expect(writtenBeforeLoading).toEqual([])
    expect(written).toEqual([first])
  } finally {
    uninstall()
  }
})

test('only the newest frame presented before loading starts is written', () => {
  // Arrange
  const { element, sink, written, uninstall } = arrangeTrackGeneratorSink()
  const older = fakeFrame()
  const newer = fakeFrame()

  try {
    // Act
    sink.present(older)
    sink.present(newer)
    element.dispatchEvent(new Event('loadstart'))

    // Assert
    expect(older.closed).toBe(true)
    expect(written).toEqual([newer])
  } finally {
    uninstall()
  }
})

test('a picture attached again waits for the next loadstart', () => {
  // Arrange
  const { element, sink, written, uninstall } = arrangeTrackGeneratorSink()
  sink.present(fakeFrame())
  element.dispatchEvent(new Event('loadstart'))
  sink.detach()
  const reattached = fakeFrame()

  try {
    // Act
    sink.present(reattached)
    const writtenBeforeLoading = written.length
    element.dispatchEvent(new Event('loadstart'))

    // Assert
    expect(writtenBeforeLoading).toBe(1)
    expect(written.at(-1)).toBe(reattached)
  } finally {
    uninstall()
  }
})
