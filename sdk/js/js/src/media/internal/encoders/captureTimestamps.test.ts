import { expect, test } from '@playwright/test'
import { CaptureTimestamps } from './captureTimestamps'

function encodedChunk(timestamp: number): EncodedAudioChunk {
  return {
    type: 'key',
    timestamp,
    duration: 20_000,
    byteLength: 0,
    copyTo: () => {}
  } as unknown as EncodedAudioChunk
}

function recordedCaptures(inputTimestamps: number[]): CaptureTimestamps {
  let nowMicros = 1_000_000
  const captureTimestamps = new CaptureTimestamps(() => (nowMicros += 20_000))
  for (const inputTimestamp of inputTimestamps) {
    captureTimestamps.recordCapture(inputTimestamp)
  }
  return captureTimestamps
}

test('an output stamped a microsecond off its input still finds the capture time', () => {
  // Arrange: captures at 1 020 000, 1 040 000, 1 060 000 µs
  const captureTimestamps = recordedCaptures([0, 20_000, 40_000])

  // Act
  const early = captureTimestamps.restamp(encodedChunk(19_999))
  const late = captureTimestamps.restamp(encodedChunk(40_001))

  // Assert
  expect(early.captureTimestampMicros).toBe(1_040_000)
  expect(late.captureTimestampMicros).toBe(1_060_000)
})

test('an output further than the tolerance from every input has no capture time', () => {
  // Arrange
  const captureTimestamps = recordedCaptures([0, 20_000])

  // Act / Assert
  expect(captureTimestamps.restamp(encodedChunk(10_000)).captureTimestampMicros).toBeUndefined()
})

test('outputs are restamped to start at zero', () => {
  // Arrange
  const captureTimestamps = recordedCaptures([500_000, 520_000])

  // Act
  const first = captureTimestamps.restamp(encodedChunk(500_000))
  const second = captureTimestamps.restamp(encodedChunk(520_000))

  // Assert
  expect(first.init.timestamp).toBe(0)
  expect(second.init.timestamp).toBe(20_000)
})
