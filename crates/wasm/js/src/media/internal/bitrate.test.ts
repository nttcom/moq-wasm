import { expect, test } from '@playwright/test'
import { createBitrateLogger } from './bitrate'

test('the rate is over the time since the previous report, not a fixed second', () => {
  // Arrange: 10,000 bytes arriving 1.5 s after the logger started
  let timeMs = 0
  const reports: number[] = []
  const addBytes = createBitrateLogger(
    (kbps) => reports.push(kbps),
    () => timeMs
  )

  // Act
  timeMs = 1_500
  addBytes(10_000)

  // Assert: 80,000 bits over 1.5 s
  expect(reports).toHaveLength(1)
  expect(reports[0]).toBeCloseTo(53.333, 3)
})

test('bytes received within a second wait for the next report', () => {
  // Arrange
  let timeMs = 0
  const reports: number[] = []
  const addBytes = createBitrateLogger(
    (kbps) => reports.push(kbps),
    () => timeMs
  )

  // Act
  timeMs = 400
  addBytes(5_000)
  timeMs = 1_000
  addBytes(5_000)

  // Assert: 80,000 bits over exactly 1 s
  expect(reports).toEqual([80])
})
