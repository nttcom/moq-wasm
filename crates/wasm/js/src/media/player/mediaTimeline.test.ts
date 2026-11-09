import { expect, test } from '@playwright/test'
import { MediaTimeline } from './mediaTimeline'

test('a live record places its group at its wallclock', () => {
  // Arrange
  const timeline = new MediaTimeline()

  // Act
  timeline.replace(JSON.stringify([[0, [1, 0], 1_700_000_000_000]]))

  // Assert
  expect(timeline.groupMarks()).toEqual([{ groupId: 1n, captureMicros: 1_700_000_000_000_000 }])
  expect(timeline.elapsedMsAt(1_700_000_000_500_000)).toBe(500)
})

test('a VOD record without a wallclock places its group at its presentation time', () => {
  // Arrange
  const timeline = new MediaTimeline()

  // Act
  timeline.replace(
    JSON.stringify([
      [2_000, [5, 0], 0],
      [3_000, [6, 0], 0]
    ])
  )

  // Assert
  expect(timeline.groupMarks()).toEqual([
    { groupId: 5n, captureMicros: 2_000_000 },
    { groupId: 6n, captureMicros: 3_000_000 }
  ])
  expect(timeline.elapsedMsAt(2_500_000)).toBe(2_500)
})
