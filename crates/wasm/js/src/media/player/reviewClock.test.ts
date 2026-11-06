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
