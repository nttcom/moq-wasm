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
