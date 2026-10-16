import { expect, test } from '@playwright/test'
import { AudioPlayout } from './audioPlayout'
import { FakeMediaEnvironment, fakeAudioData } from './testing/fakeMediaEnvironment'

const SAMPLE_RATE = 48_000
const CHUNK_FRAMES = 960
const OUTPUT_LATENCY_SECONDS = 0.04

let environment: FakeMediaEnvironment

test.beforeEach(() => {
  environment = FakeMediaEnvironment.install({ outputLatencySeconds: OUTPUT_LATENCY_SECONDS })
})

test.afterEach(() => {
  environment.uninstall()
})

test('a timed chunk due now reports the output latency it starts late by as drift', () => {
  // Arrange
  const driftsMs: number[] = []
  const playout = new AudioPlayout((driftMs) => driftsMs.push(driftMs))

  // Act
  playout.play(fakeAudioData(SAMPLE_RATE, CHUNK_FRAMES), 1_000_000, (nowMs) => nowMs)

  // Assert
  expect(driftsMs).toHaveLength(1)
  expect(driftsMs[0]).toBeCloseTo(OUTPUT_LATENCY_SECONDS * 1_000)
})

test('an untimed chunk reports no drift', () => {
  // Arrange
  const driftsMs: number[] = []
  const playout = new AudioPlayout((driftMs) => driftsMs.push(driftMs))

  // Act
  playout.play(fakeAudioData(SAMPLE_RATE, CHUNK_FRAMES), undefined, (nowMs) => nowMs)

  // Assert
  expect(driftsMs).toEqual([])
})

test('a compression rate below 1 lengthens the chunks and reports the added time as drift', () => {
  // Arrange: chunks due 200 ms after they arrive, every 20 ms
  const driftsMs: number[] = []
  const playout = new AudioPlayout((driftMs) => driftsMs.push(driftMs))
  playout.setCompressionRate(1 / 1.1)
  const startMs = environment.nowMs

  // Act
  for (let index = 0; index < 10; index += 1) {
    environment.advanceTo(startMs + index * 20)
    const dueMs = startMs + 200 + index * 20
    playout.play(fakeAudioData(SAMPLE_RATE, CHUNK_FRAMES), 1_000_000 + index * 20_000, () => dueMs)
  }

  // Assert
  expect(driftsMs[0]).toBeCloseTo(0)
  expect(Math.max(...driftsMs)).toBeGreaterThan(0)
  expect(driftsMs.every((driftMs) => driftMs >= 0)).toBe(true)
})
