import { expect, test } from '@playwright/test'
import { LivePlayout } from './livePlayout'
import { FakeMediaEnvironment, fakeAudioData, fakeVideoFrame } from './testing/fakeMediaEnvironment'

const SAMPLE_RATE = 48_000
const AUDIO_CHUNK_MS = 20
const VIDEO_FRAME_MS = 40
const STREAM_MS = 3_000
const CAPTURE_EPOCH_MS = 1_000_000
const MICROS_PER_MILLI = 1_000
const OUTPUT_LATENCY_MS = 40

let environment: FakeMediaEnvironment

test.beforeEach(() => {
  environment = FakeMediaEnvironment.install({ outputLatencySeconds: OUTPUT_LATENCY_MS / 1_000 })
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

const AUDIO_DELAY_MS = 100
const VIDEO_DELAY_MS = 300
const SETTLE_MS = 12_000

/// Streams audio arriving `AUDIO_DELAY_MS` after capture, with video arriving
/// `videoDelayMs` after capture when given, for `durationMs`.
function streamLive(playout: LivePlayout, durationMs: number, videoDelayMs?: number): void {
  const startMs = environment.nowMs
  for (let elapsedMs = 0; elapsedMs < durationMs; elapsedMs += AUDIO_CHUNK_MS) {
    environment.advanceTo(startMs + elapsedMs)
    const nowMs = environment.nowMs
    playout.playAudio(
      fakeAudioData(SAMPLE_RATE, (SAMPLE_RATE * AUDIO_CHUNK_MS) / 1_000),
      (nowMs - AUDIO_DELAY_MS) * MICROS_PER_MILLI
    )
    if (videoDelayMs !== undefined && elapsedMs % VIDEO_FRAME_MS === 0) {
      playout.presentVideo(fakeVideoFrame((nowMs - videoDelayMs) * MICROS_PER_MILLI), nowMs)
    }
  }
}

test('video arriving later than the audio raises the buffer target', () => {
  // Arrange
  const withVideo = new LivePlayout((frame) => frame.close())
  const audioOnly = new LivePlayout((frame) => frame.close())

  // Act: `withVideo` streams last, so its output latency is the one read
  streamLive(audioOnly, SETTLE_MS)
  streamLive(withVideo, SETTLE_MS, VIDEO_DELAY_MS)

  // Assert
  expect(audioOnly.targetBufferMs()).toBeCloseTo(200)
  expect(withVideo.targetBufferMs()).toBeCloseTo(VIDEO_DELAY_MS - AUDIO_DELAY_MS + OUTPUT_LATENCY_MS)
})

test('without a picture the presented position follows the sound', () => {
  // Arrange
  const playout = new LivePlayout((frame) => frame.close())

  // Act
  streamLive(playout, SETTLE_MS)

  // Assert
  expect(environment.nowMs - playout.positionMicros()! / MICROS_PER_MILLI).toBeCloseTo(
    AUDIO_DELAY_MS + playout.bufferMs()!
  )
})

test('a buffer below its target grows by stretching the audio without a break', () => {
  // Arrange: the picture arrives so much later than the sound that the opening buffer cannot cover it
  const playout = new LivePlayout((frame) => frame.close())
  const lateVideoDelayMs = VIDEO_DELAY_MS + 200

  // Act: read the buffer before it settles, then keep streaming
  streamLive(playout, SETTLE_MS - 3_000, lateVideoDelayMs)
  const openingBufferMs = playout.bufferMs()!
  streamLive(playout, 9_000, lateVideoDelayMs)

  // Assert
  expect(openingBufferMs).toBeLessThan(playout.targetBufferMs() - 2 * AUDIO_CHUNK_MS)
  expect(playout.bufferMs()!).toBeGreaterThanOrEqual(playout.targetBufferMs() - 2 * AUDIO_CHUNK_MS)
  expect(playout.audioBreaks()).toBe(0)
})

const LIVE_VIDEO_DELAY_MS = 50

function streamLiveVideo(playout: LivePlayout, durationMs: number): void {
  const startMs = environment.nowMs
  for (let elapsedMs = 0; elapsedMs < durationMs; elapsedMs += VIDEO_FRAME_MS) {
    environment.advanceTo(startMs + elapsedMs)
    const nowMs = environment.nowMs
    playout.presentVideo(fakeVideoFrame((nowMs - LIVE_VIDEO_DELAY_MS) * MICROS_PER_MILLI), nowMs)
  }
}

test('a cached frame of a still picture is shown at once and the live frames after it are not held back', () => {
  // Arrange: the relay cached the keyframe of a screen that has not changed for two minutes
  const shownDelaysMs: number[] = []
  const playout = new LivePlayout((frame) => {
    shownDelaysMs.push(environment.nowMs - frame.timestamp / MICROS_PER_MILLI)
    frame.close()
  })
  const cachedDelayMs = 120_000

  // Act
  playout.presentCachedVideo(fakeVideoFrame((environment.nowMs - cachedDelayMs) * MICROS_PER_MILLI))
  environment.advanceTo(environment.nowMs + 1)
  const shownBeforeLive = shownDelaysMs.length
  streamLiveVideo(playout, 2_000)
  environment.advanceTo(environment.nowMs + 1_000)

  // Assert
  expect(shownBeforeLive).toBe(1)
  expect(shownDelaysMs.length).toBeGreaterThan(2_000 / VIDEO_FRAME_MS / 2)
  expect(Math.max(...shownDelaysMs.slice(1))).toBeLessThan(1_000)
})

test('cached frames arriving after the live ones leave the buffer target alone', () => {
  // Arrange: the live frames anchor the clock before the joining FETCH delivers the last 4 s
  const playout = new LivePlayout((frame) => frame.close())
  streamLiveVideo(playout, 1_000)

  // Act
  for (let ageMs = 4_000; ageMs > 0; ageMs -= VIDEO_FRAME_MS) {
    playout.presentCachedVideo(fakeVideoFrame((environment.nowMs - ageMs) * MICROS_PER_MILLI))
  }
  streamLiveVideo(playout, 1_000)

  // Assert
  expect(playout.targetBufferMs()).toBeCloseTo(200)
})
