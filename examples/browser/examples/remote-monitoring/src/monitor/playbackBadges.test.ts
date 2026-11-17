import { expect, test } from '@playwright/test'
import { framesPerSecond } from './playbackBadges'

test('frames per second are the frames between the oldest and the newest sample, scaled to a second', () => {
  // Arrange: 15 frames every 500 ms
  const samples = [
    { atMs: 1_000, frames: 100 },
    { atMs: 1_500, frames: 115 },
    { atMs: 2_000, frames: 130 }
  ]

  // Act / Assert
  expect(framesPerSecond(samples)).toBe(30)
})

test('a picture that presented no frame between its samples counts 0', () => {
  // Arrange
  const samples = [
    { atMs: 1_000, frames: 100 },
    { atMs: 1_750, frames: 100 }
  ]

  // Act / Assert
  expect(framesPerSecond(samples)).toBe(0)
})

test('one sample alone gives no count, so a picture that just appeared is not reported as stalled', () => {
  // Act / Assert
  expect(framesPerSecond([{ atMs: 1_000, frames: 0 }])).toBeUndefined()
  expect(framesPerSecond([])).toBeUndefined()
})
