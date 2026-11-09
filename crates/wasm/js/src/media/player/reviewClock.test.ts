import { expect, test } from '@playwright/test'
import { ReviewClock } from './reviewClock'
import { FakeMediaEnvironment } from './testing/fakeMediaEnvironment'

const CAPTURE_MICROS = 1_759_924_100_000_000
const PAUSE_MS = 3_000

let environment: FakeMediaEnvironment

test.beforeEach(() => {
  environment = FakeMediaEnvironment.install({ outputLatencySeconds: 0 })
})

test.afterEach(() => {
  environment.uninstall()
})

test('resuming a paused clock moves the times it gives, and the frames its followers hold, by the pause', () => {
  // Arrange
  const clock = new ReviewClock()
  const resumedAfterMs: number[] = []
  clock.follow({
    followPause: (paused, pausedForMs) => {
      if (!paused) {
        resumedAfterMs.push(pausedForMs)
      }
    }
  })
  clock.anchor(CAPTURE_MICROS, false)
  const dueMs = clock.dueAt(CAPTURE_MICROS)
  clock.setPaused(true)

  // Act
  environment.advanceTo(environment.nowMs + PAUSE_MS)
  clock.setPaused(false)

  // Assert
  expect(resumedAfterMs).toEqual([PAUSE_MS])
  expect(clock.dueAt(CAPTURE_MICROS)).toBe(dueMs + PAUSE_MS)
})

test('the position starts at the anchor before the first frame is due, and stands still while paused', () => {
  // Arrange
  const clock = new ReviewClock()
  clock.anchor(CAPTURE_MICROS, false)
  const atAnchor = clock.positionMicros()
  environment.advanceTo(environment.nowMs + 1_150)
  clock.setPaused(true)

  // Act
  environment.advanceTo(environment.nowMs + PAUSE_MS)

  // Assert: the first frame was due 150 ms after the anchor
  expect(atAnchor).toBe(CAPTURE_MICROS)
  expect(clock.positionMicros()).toBe(CAPTURE_MICROS + 1_000_000)
})

test('only the first follower to report drift since the anchor moves the clock, until it stops following', () => {
  // Arrange
  const first = { followPause: () => {} }
  const second = { followPause: () => {} }
  const clock = new ReviewClock()
  clock.follow(first)
  clock.follow(second)
  clock.anchor(CAPTURE_MICROS, false)
  const dueMs = clock.dueAt(CAPTURE_MICROS)

  // Act
  clock.drift(first, 10)
  clock.drift(second, 100)
  clock.unfollow(first)
  clock.drift(second, 1)

  // Assert
  expect(clock.dueAt(CAPTURE_MICROS)).toBe(dueMs + 11)
})
