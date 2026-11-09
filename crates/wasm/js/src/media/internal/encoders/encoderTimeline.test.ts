import { expect, test } from '@playwright/test'
import { EncoderTimeline } from './encoderTimeline'

const PUBLICATION_START_MICROS = 1_000_000_000

test('chunks are placed from the capture time of the first chunk, not the encoder start', () => {
  // Arrange: the encoder started 1 s into the publication and captured its first frame 2.4 s later
  const timeline = new EncoderTimeline(PUBLICATION_START_MICROS, 1_000_000)

  // Act
  const first = timeline.place(0, PUBLICATION_START_MICROS + 3_400_000)
  const second = timeline.place(33_000, PUBLICATION_START_MICROS + 3_433_500)

  // Assert: later chunks keep the encoder's spacing, not their own capture times
  expect([first, second]).toEqual([3_400_000, 3_433_000])
})

test('without a capture time on the first chunk the fallback offset anchors the chunks', () => {
  // Arrange
  const timeline = new EncoderTimeline(PUBLICATION_START_MICROS, 1_000_000)

  // Act
  const first = timeline.place(0, undefined)
  const second = timeline.place(20_000, PUBLICATION_START_MICROS + 9_000_000)

  // Assert
  expect([first, second]).toEqual([1_000_000, 1_020_000])
})
