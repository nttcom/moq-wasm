import { expect, test } from '@playwright/test'
import { captureWithRetry } from './captureRetry'

type FakeTrack = { readyState: MediaStreamTrackState; stopped: boolean; stop: () => void }

function fakeStream(readyState: MediaStreamTrackState): { stream: MediaStream; track: FakeTrack } {
  const track: FakeTrack = {
    readyState,
    stopped: false,
    stop() {
      this.stopped = true
    }
  }
  const stream = { getTracks: () => [track] } as unknown as MediaStream
  return { stream, track }
}

const OPTIONS = { maxAttempts: 3, retryDelayMs: 0, isCancelled: () => false }

test('a failed capture is retried until one succeeds', async () => {
  // Arrange
  const { stream } = fakeStream('live')
  let calls = 0
  const capture = async () => {
    calls += 1
    if (calls < 3) {
      throw new Error('NotFoundError')
    }
    return stream
  }

  // Act
  const result = await captureWithRetry(capture, OPTIONS)

  // Assert
  expect(result).toBe(stream)
  expect(calls).toBe(3)
})

test('the last error is thrown once every attempt has failed', async () => {
  // Arrange
  let calls = 0
  const capture = async (): Promise<MediaStream> => {
    calls += 1
    throw new Error(`attempt ${calls}`)
  }

  // Act / Assert
  await expect(captureWithRetry(capture, OPTIONS)).rejects.toThrow('attempt 3')
  expect(calls).toBe(3)
})

test('a capture whose track already ended counts as a failure and is stopped', async () => {
  // Arrange
  const ended = fakeStream('ended')
  const live = fakeStream('live')
  const streams = [ended.stream, live.stream]
  const capture = async () => streams.shift()!

  // Act
  const result = await captureWithRetry(capture, OPTIONS)

  // Assert
  expect(result).toBe(live.stream)
  expect(ended.track.stopped).toBe(true)
})

test('a capture that finishes after cancelling is stopped and discarded', async () => {
  // Arrange
  const { stream, track } = fakeStream('live')
  let cancelled = false
  const capture = async () => {
    cancelled = true
    return stream
  }

  // Act
  const result = await captureWithRetry(capture, { ...OPTIONS, isCancelled: () => cancelled })

  // Assert
  expect(result).toBeNull()
  expect(track.stopped).toBe(true)
})

test('a failure after cancelling ends the retries without an error', async () => {
  // Arrange
  let calls = 0
  let cancelled = false
  const capture = async (): Promise<MediaStream> => {
    calls += 1
    cancelled = true
    throw new Error('NotAllowedError')
  }

  // Act
  const result = await captureWithRetry(capture, { ...OPTIONS, isCancelled: () => cancelled })

  // Assert
  expect(result).toBeNull()
  expect(calls).toBe(1)
})
