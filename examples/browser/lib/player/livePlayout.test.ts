import { expect, test } from '@playwright/test'
import { LivePlayout } from './livePlayout'
import { FakeMediaEnvironment, fakeAudioData, fakeVideoFrame } from './testing/fakeMediaEnvironment'

const SAMPLE_RATE = 48_000
const AUDIO_CHUNK_MS = 20
const VIDEO_FRAME_MS = 40
const STREAM_MS = 3_000
const CAPTURE_EPOCH_MS = 1_000_000
const MICROS_PER_MILLI = 1_000

let environment: FakeMediaEnvironment

test.beforeEach(() => {
  environment = FakeMediaEnvironment.install({ outputLatencySeconds: 0.04 })
})

test.afterEach(() => {
  environment.uninstall()
})

test('audio without capture timestamps leaves the delay of timed video unchanged', () => {
  // Arrange
  const videoDelaysMs: number[] = []
  const playout = new LivePlayout((frame) => {
    videoDelaysMs.push(environment.nowMs - frame.timestamp / MICROS_PER_MILLI)
    frame.close()
  })
  const startMs = environment.nowMs

  // Act
  for (let elapsedMs = 0; elapsedMs < STREAM_MS; elapsedMs += AUDIO_CHUNK_MS) {
    environment.advanceTo(startMs + elapsedMs)
    playout.playAudio(fakeAudioData(SAMPLE_RATE, (SAMPLE_RATE * AUDIO_CHUNK_MS) / 1_000), undefined)
    if (elapsedMs % VIDEO_FRAME_MS === 0) {
      playout.presentVideo(fakeVideoFrame((CAPTURE_EPOCH_MS + elapsedMs) * MICROS_PER_MILLI))
    }
  }
  environment.advanceTo(startMs + STREAM_MS * 4)

  // Assert
  expect(videoDelaysMs.length).toBeGreaterThan(STREAM_MS / VIDEO_FRAME_MS / 2)
  expect(videoDelaysMs.at(-1)! - videoDelaysMs[0]).toBeCloseTo(0)
})
