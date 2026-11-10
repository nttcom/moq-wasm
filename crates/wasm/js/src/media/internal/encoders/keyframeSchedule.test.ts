import { expect, test } from '@playwright/test'
import { KeyframeSchedule } from './keyframeSchedule'

test('the first frame and the first frame an interval after the last keyframe are keyframes', () => {
  // Arrange
  const schedule = new KeyframeSchedule(1_000_000)

  // Act
  const keyframes = [0, 400_000, 999_999, 1_000_000, 1_500_000, 2_100_000].map((micros) =>
    schedule.isKeyframe(micros, false)
  )

  // Assert
  expect(keyframes).toEqual([true, false, false, true, false, true])
})

test('a forced keyframe restarts the interval', () => {
  // Arrange
  const schedule = new KeyframeSchedule(1_000_000)
  schedule.isKeyframe(0, false)

  // Act
  const forced = schedule.isKeyframe(600_000, true)
  const afterForced = schedule.isKeyframe(1_200_000, false)

  // Assert: the next keyframe is due 1 s after the forced one, not after the first
  expect([forced, afterForced]).toEqual([true, false])
  expect(schedule.isKeyframe(1_600_000, false)).toBe(true)
})

test('after a reset the next frame is a keyframe', () => {
  // Arrange
  const schedule = new KeyframeSchedule(1_000_000)
  schedule.isKeyframe(0, false)

  // Act
  schedule.reset()

  // Assert
  expect(schedule.isKeyframe(100_000, false)).toBe(true)
})
