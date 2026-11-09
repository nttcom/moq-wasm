import { expect, test } from '@playwright/test'
import { AudioPlayout } from './audioPlayout'
import { type FakeAudioContext, FakeMediaEnvironment, fakeAudioData } from './testing/fakeMediaEnvironment'

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

test('a chunk that arrives after the sound ran out continues from the repetition that filled the gap', () => {
  // Arrange: a chunk due 100 ms from now, and the next one 150 ms after it has run out
  const driftsMs: number[] = []
  const playout = new AudioPlayout((driftMs) => driftsMs.push(driftMs))
  const startMs = environment.nowMs
  playout.play(fakeAudioData(SAMPLE_RATE, CHUNK_FRAMES), 1_000_000, () => startMs + 100)

  // Act
  environment.advanceTo(startMs + 270)
  playout.play(fakeAudioData(SAMPLE_RATE, CHUNK_FRAMES), 1_020_000, (nowMs) => nowMs)

  // Assert: no break; the repetition is heard first and the chunk reports it as drift
  expect(playout.breaks).toBe(0)
  expect(playout.concealedMs).toBeGreaterThan(150)
  expect(driftsMs[1]).toBeGreaterThan(OUTPUT_LATENCY_SECONDS * 1_000)
})

test('a repetition fades out after 300 ms and a chunk arriving after it is a break', () => {
  // Arrange
  const playout = new AudioPlayout(() => {})
  const startMs = environment.nowMs
  playout.play(fakeAudioData(SAMPLE_RATE, CHUNK_FRAMES), 1_000_000, () => startMs + 100)

  // Act
  environment.advanceTo(startMs + 1_000)
  playout.play(fakeAudioData(SAMPLE_RATE, CHUNK_FRAMES), 1_020_000, (nowMs) => nowMs)

  // Assert
  expect(playout.concealedMs).toBeCloseTo(300)
  expect(playout.breaks).toBe(1)
})

test('an input opened on a playout plays in its context and leaves its clock alone', () => {
  // Arrange
  const createdContexts: number[] = []
  const FakeAudioContext = globalThis.AudioContext
  globalThis.AudioContext = class extends FakeAudioContext {
    constructor(options: AudioContextOptions) {
      super(options)
      createdContexts.push(options.sampleRate ?? 0)
    }
  } as typeof AudioContext
  const driftsMs: number[] = []
  const host = new AudioPlayout((driftMs) => driftsMs.push(driftMs))
  const input = host.openInput()
  const startMs = environment.nowMs

  // Act: the host plays on its clock and the input is placed at a time of its own
  host.play(fakeAudioData(SAMPLE_RATE, CHUNK_FRAMES), 1_000_000, (nowMs) => nowMs)
  input.setVolume(0, 0.25)
  input.play(fakeAudioData(SAMPLE_RATE, CHUNK_FRAMES), 1_000_000, () => startMs + 100)
  const inputPositionMicros = input.positionMicrosAt(startMs + 100)
  input.close()
  host.play(fakeAudioData(SAMPLE_RATE, CHUNK_FRAMES), 1_020_000, (nowMs) => nowMs)

  // Assert: the host plays on after the input closed
  expect(createdContexts).toEqual([SAMPLE_RATE])
  expect(inputPositionMicros).toBe(1_000_000)
  expect(driftsMs).toHaveLength(2)
  expect(host.positionMicrosAt(environment.nowMs)).toBeDefined()
})

test('a repetition on a given context at another rate is made at the rate of the stream it repeats', () => {
  // Arrange: a 44.1 kHz context, and a 48 kHz chunk due 100 ms from now that runs out with nothing after it
  const context = new AudioContext({ sampleRate: 44_100 }) as FakeAudioContext
  const playout = new AudioPlayout(
    () => undefined,
    () => context
  )
  const startMs = environment.nowMs
  playout.play(fakeAudioData(SAMPLE_RATE, CHUNK_FRAMES), 1_000_000, () => startMs + 100)

  // Act
  environment.advanceTo(startMs + 270)

  // Assert
  expect(playout.concealedMs).toBeGreaterThan(0)
  expect(new Set(context.bufferSampleRates)).toEqual(new Set([SAMPLE_RATE]))
})
