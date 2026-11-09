import { expect, test } from '@playwright/test'
import { CaptureTimestamps } from './captureTimestamps'
import { EncoderSamplePosition } from './encoderSamplePosition'

const FRAME_MICROS = 10_000

function input(timestamp: number): { timestamp: number; numberOfFrames: number; sampleRate: number } {
  return { timestamp, numberOfFrames: 480, sampleRate: 48_000 }
}

function encodedChunk(timestamp: number): EncodedAudioChunk {
  return {
    type: 'key',
    timestamp,
    duration: 2 * FRAME_MICROS,
    byteLength: 0,
    copyTo: () => {}
  } as unknown as EncodedAudioChunk
}

test('positions follow the samples, not a jump in the input timestamps', () => {
  // Arrange
  const position = new EncoderSamplePosition()

  // Act
  const positions = [input(5_000_000), input(5_010_000), input(7_020_000)].map((audio) => position.take(audio))

  // Assert
  expect(positions).toEqual([5_000_000, 5_010_000, 5_020_000])
})

test('a reset starts the positions from the next input', () => {
  // Arrange
  const position = new EncoderSamplePosition()
  position.take(input(5_000_000))

  // Act
  position.reset()

  // Assert
  expect(position.take(input(9_000_000))).toBe(9_000_000)
})

test('an output after a capture gap gets the capture time of its own input', () => {
  // Arrange: inputs every 10 ms of wall clock; their timestamps jump 2 s after the 100th
  let nowMicros = 0
  const captureTimestamps = new CaptureTimestamps(() => nowMicros)
  const position = new EncoderSamplePosition()
  for (let index = 0; index < 300; index++) {
    nowMicros = index * FRAME_MICROS
    const timestamp = index * FRAME_MICROS + (index >= 100 ? 2_000_000 : 0)
    captureTimestamps.recordCapture(position.take(input(timestamp)))
  }

  // Act: the encoder stamps the output of inputs 298-299 from the samples encoded since the first input, ignoring the 2 s jump
  const { captureTimestampMicros } = captureTimestamps.restamp(encodedChunk(298 * FRAME_MICROS))

  // Assert
  expect(captureTimestampMicros).toBe(298 * FRAME_MICROS)
})
