import { expect, test } from '@playwright/test'
import { JitterBuffer } from './jitterBuffer'

const MICROS_PER_MILLI = 1_000
const ARRIVAL_INTERVAL_MS = 20

function observeArrivals(buffer: JitterBuffer): void {
  const delaysMs = [0, ...Array<number>(94).fill(50), ...Array<number>(5).fill(500)]
  delaysMs.forEach((delayMs, index) => {
    const captureMs = index * ARRIVAL_INTERVAL_MS
    buffer.observe('audio', captureMs * MICROS_PER_MILLI, captureMs + delayMs)
  })
}

test('by default the spread reaches the slowest arrival', () => {
  // Arrange
  const buffer = new JitterBuffer({ minimumMs: 0, maximumMs: Number.POSITIVE_INFINITY })

  // Act
  observeArrivals(buffer)

  // Assert
  expect(buffer.spreadMs()).toBe(500)
})

test('a quantile below 1 leaves the slowest arrivals out of the spread', () => {
  // Arrange
  const buffer = new JitterBuffer({ minimumMs: 0, maximumMs: Number.POSITIVE_INFINITY, arrivalQuantile: 0.95 })

  // Act
  observeArrivals(buffer)

  // Assert
  expect(buffer.spreadMs()).toBe(50)
})

const OUTPUT_LATENCY_MS = 200
const OUTPUT_FLOOR_MS = 220

test('a maximum above the output floor caps the target', () => {
  // Arrange
  const buffer = new JitterBuffer({ minimumMs: 0, maximumMs: 300 })
  observeArrivals(buffer)

  // Act
  const targetMs = buffer.targetMs(OUTPUT_LATENCY_MS, OUTPUT_FLOOR_MS)

  // Assert
  expect(targetMs).toBe(300)
})

test('a maximum below the output floor gives way to it', () => {
  // Arrange
  const buffer = new JitterBuffer({ minimumMs: 0, maximumMs: 40 })
  observeArrivals(buffer)

  // Act
  const targetMs = buffer.targetMs(OUTPUT_LATENCY_MS, OUTPUT_FLOOR_MS)

  // Assert
  expect(targetMs).toBe(OUTPUT_FLOOR_MS)
})
