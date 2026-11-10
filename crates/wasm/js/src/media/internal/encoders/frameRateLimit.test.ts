import { expect, test } from '@playwright/test'
import { FrameRateLimit } from './frameRateLimit'

const FRAME_30_FPS_MICROS = 1_000_000 / 30

function timestamps(count: number, intervalMicros: number): number[] {
  return Array.from({ length: count }, (_, index) => Math.round(index * intervalMicros))
}

test('a 30 fps source limited to 15 fps keeps every other frame', () => {
  // Arrange
  const limit = new FrameRateLimit(15)

  // Act
  const admitted = timestamps(8, FRAME_30_FPS_MICROS).map((micros) => limit.admit(micros, false))

  // Assert
  expect(admitted).toEqual([true, false, true, false, true, false, true, false])
})

test('a source at the limit keeps every frame despite jitter', () => {
  // Arrange
  const limit = new FrameRateLimit(30)
  const jittered = timestamps(8, FRAME_30_FPS_MICROS).map((micros, index) => micros + (index % 2 ? -5_000 : 5_000))

  // Act
  const admitted = jittered.map((micros) => limit.admit(micros, false))

  // Assert
  expect(admitted.every(Boolean)).toBe(true)
})

test('a keyframe is kept even when it comes early', () => {
  // Arrange
  const limit = new FrameRateLimit(15)
  limit.admit(0, false)

  // Act / Assert
  expect(limit.admit(Math.round(FRAME_30_FPS_MICROS), true)).toBe(true)
})

test('a frame after a long gap passes and restarts the count', () => {
  // Arrange: a still screen share sends one frame, then nothing for 5 s
  const limit = new FrameRateLimit(15)
  limit.admit(0, false)

  // Act
  const afterGap = [
    5_000_000,
    5_000_000 + Math.round(FRAME_30_FPS_MICROS),
    5_000_000 + Math.round(2 * FRAME_30_FPS_MICROS)
  ].map((micros) => limit.admit(micros, false))

  // Assert
  expect(afterGap).toEqual([true, false, true])
})
