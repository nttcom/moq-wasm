import { expect, test } from '@playwright/test'
import { TrackStats, decodeCursorSample, encodeCursorSample } from './cursorWire'

test('a cursor sample survives encoding and fits in one datagram', () => {
  // Arrange
  const sample = { seq: 42, x: 0.25, y: 0.75, sentAtMs: 1_731_456_000_123.5 }

  // Act
  const payload = encodeCursorSample(sample)

  // Assert
  expect(payload.byteLength).toBe(20)
  expect(decodeCursorSample(payload)).toEqual(sample)
  expect(decodeCursorSample(payload.subarray(1))).toBeUndefined()
})

test('the stats count lost and reordered samples and measure latency within the window', () => {
  // Arrange
  const stats = new TrackStats(1_000)
  const at = (seq: number, sentAtMs: number) => ({ seq, x: 0, y: 0, sentAtMs })

  // Act: seq 2 never arrives, seq 3 arrives after seq 4
  stats.record(at(0, 0), 10)
  stats.record(at(1, 16), 30)
  stats.record(at(4, 64), 74)
  stats.record(at(3, 48), 200)
  const summary = stats.summary(200)
  const later = stats.summary(1_150)

  // Assert: the window drops the arrivals older than 1 s
  expect(summary).toEqual({
    received: 4,
    lost: 1,
    reordered: 1,
    lastLatencyMs: 152,
    medianLatencyMs: 14,
    p95LatencyMs: 152,
    maxGapMs: 126
  })
  expect(later.received).toBe(4)
  expect(later.lastLatencyMs).toBe(152)
  expect(stats.latencies(1_150)).toHaveLength(1)
})
