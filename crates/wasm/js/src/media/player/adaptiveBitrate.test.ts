import { expect, test } from '@playwright/test'
import type { MediaCatalogTrack } from '../catalog'
import { AdaptiveBitrate, type Rung, bitrateLadder, capLadderToHeight, renditionAboveHeight } from './adaptiveBitrate'

const LADDER: Rung[] = [
  { name: 'video_360p', bitrate: 700_000 },
  { name: 'video_480p', bitrate: 1_200_000 },
  { name: 'video_720p', bitrate: 2_500_000 },
  { name: 'video', bitrate: 6_000_000 }
]
const TALL_LADDER: Rung[] = [
  { name: 'video_360p', bitrate: 700_000, height: 360 },
  { name: 'video_480p', bitrate: 1_200_000, height: 480 },
  { name: 'video_720p', bitrate: 2_500_000, height: 720 },
  { name: 'video', bitrate: 6_000_000, height: 1080 }
]
const FRAME_MS = 33
const SETTLE_MS = 3_000

function track(name: string, height: number, bitrate?: number): MediaCatalogTrack {
  return { name, label: name, altGroup: 1, width: (height * 16) / 9, height, bitrate }
}

/// Feeds 30 fps objects of `bytesPerFrame` from `fromMs` to `toMs`, each
/// `lagMs(t)` behind its capture.
function deliver(
  abr: AdaptiveBitrate,
  fromMs: number,
  toMs: number,
  bytesPerFrame: number,
  lagMs: (atMs: number) => number
): void {
  for (let atMs = fromMs; atMs < toMs; atMs += FRAME_MS) {
    abr.observe({ arrivedAtMs: atMs, bytes: bytesPerFrame, lagMs: lagMs(atMs) })
  }
}

/// Feeds a second of objects 100 ms behind their capture from `fromMs`, then
/// objects a second further behind until `toMs`, which `decide` at `toMs` takes
/// for congestion.
function congest(abr: AdaptiveBitrate, fromMs: number, toMs: number): void {
  deliver(abr, fromMs, fromMs + 1_000, 4_000, () => 100)
  deliver(abr, fromMs + 1_000, toMs, 4_000, () => 1_100)
}

test('the ladder estimates the source bitrate from its pixels and the largest known rendition', () => {
  // Arrange
  const source = track('video', 1080)
  const tracks = [source, track('video_720p', 720, 2_500_000), track('video_360p', 360, 700_000)]

  // Act
  const ladder = bitrateLadder(tracks, source)

  // Assert
  expect(ladder.map((rung) => rung.name)).toEqual(['video_360p', 'video_720p', 'video'])
  expect(ladder[2].bitrate).toBeCloseTo(2_500_000 * 2.25, 0)
})

test('a switching set without any known bitrate has no ladder', () => {
  // Arrange
  const source = track('video', 1080)

  // Act / Assert
  expect(bitrateLadder([source, track('video_720p', 720)], source)).toEqual([])
})

test('a track outside any switching set has no ladder', () => {
  // Arrange
  const lone = { ...track('video', 1080, 6_000_000), altGroup: undefined }

  // Act / Assert
  expect(bitrateLadder([lone, track('video_720p', 720, 2_500_000)], lone)).toEqual([])
})

test('a rising lag steps down to the highest rendition the measured throughput fits', () => {
  // Arrange: 1.7 Mbps arrives while the lag grows by half a second
  const abr = new AdaptiveBitrate()
  abr.restart(0)
  deliver(abr, SETTLE_MS, 6_000, 7_000, () => 100)
  deliver(abr, 6_000, 8_000, 7_000, (atMs) => 100 + (atMs - 6_000) / 2)

  // Act
  const choice = abr.decide(8_000, LADDER, 'video')

  // Assert: 0.8 × 1.7 Mbps fits 480p but not 720p
  expect(choice).toBe('video_480p')
})

test('a publisher sending nothing while its picture stands still does not step down', () => {
  // Arrange: a shared screen sends nothing from 6 s to 9 s, then its next frame on time
  const abr = new AdaptiveBitrate()
  abr.restart(0)
  deliver(abr, SETTLE_MS, 6_000, 3_000, () => 100)
  abr.observe({ arrivedAtMs: 9_000, bytes: 30_000, lagMs: 100 })

  // Act
  const choice = abr.decide(9_000, LADDER, 'video_720p')

  // Assert
  expect(choice).toBeUndefined()
})

test('objects held up on the path step down once they arrive', () => {
  // Arrange: nothing arrives from 6 s to 7.5 s, then the held-up frames 1.5 s late
  const abr = new AdaptiveBitrate()
  abr.restart(0)
  deliver(abr, SETTLE_MS, 6_000, 3_000, () => 100)
  deliver(abr, 7_500, 8_600, 3_000, () => 1_600)

  // Act
  const choice = abr.decide(8_600, LADDER, 'video_720p')

  // Assert: 0.8 × 0.73 Mbps fits nothing above the lowest rendition
  expect(choice).toBe('video_360p')
})

test('a single late keyframe is not taken for congestion', () => {
  // Arrange
  const abr = new AdaptiveBitrate()
  abr.restart(0)
  deliver(abr, SETTLE_MS, 6_000, 6_000, (atMs) => (atMs > 5_500 && atMs < 5_600 ? 900 : 100))

  // Act / Assert
  expect(abr.decide(6_000, LADDER, 'video')).toBeUndefined()
})

test('a calm spell steps up one rendition', () => {
  // Arrange
  const abr = new AdaptiveBitrate()
  abr.restart(0)
  deliver(abr, SETTLE_MS, SETTLE_MS + 10_000, 4_000, () => 100)

  // Act
  const choice = abr.decide(SETTLE_MS + 10_000, LADDER, 'video_360p')

  // Assert
  expect(choice).toBe('video_480p')
})

test('nothing is decided while the new rendition settles', () => {
  // Arrange
  const abr = new AdaptiveBitrate()
  abr.restart(0)

  // Act / Assert
  expect(abr.decide(SETTLE_MS - 1, LADDER, 'video')).toBeUndefined()
})

test('a step up that has to step down again doubles the calm spell before the next one', () => {
  // Arrange: step up at 13 s, congestion right after it
  const abr = new AdaptiveBitrate()
  abr.restart(0)
  deliver(abr, SETTLE_MS, 13_000, 4_000, () => 100)
  abr.decide(13_000, LADDER, 'video_360p')
  abr.restart(13_000)
  congest(abr, 16_000, 18_000)
  abr.decide(18_000, LADDER, 'video_480p')
  abr.restart(18_000)
  deliver(abr, 21_000, 41_000, 4_000, () => 100)

  // Act
  const early = abr.decide(31_000 + 1, LADDER, 'video_360p')
  const late = abr.decide(41_000, LADDER, 'video_360p')

  // Assert: the spell is 20 s from the restart's settling at 21 s
  expect(early).toBeUndefined()
  expect(late).toBe('video_480p')
})

test('a picture shown 500 pixels tall caps the ladder at the first rendition that tall', () => {
  // Arrange
  const ladder: Rung[] = [
    { name: 'camera_480p', bitrate: 200_000, height: 480 },
    { name: 'camera_720p', bitrate: 500_000, height: 720 },
    { name: 'camera_1080p', bitrate: 1_000_000, height: 1080 }
  ]

  // Act / Assert
  expect(capLadderToHeight(ladder, 500).map((rung) => rung.name)).toEqual(['camera_480p', 'camera_720p'])
  expect(capLadderToHeight(ladder, 2_000)).toEqual(ladder)
  expect(capLadderToHeight(ladder, undefined)).toEqual(ladder)
})

test('a raised ceiling jumps to the top rendition at once', () => {
  // Arrange
  const abr = new AdaptiveBitrate()
  abr.restart(0)
  deliver(abr, SETTLE_MS, 5_000, 4_000, () => 100)

  // Act / Assert
  expect(abr.raiseTo(5_000, LADDER, 'video_360p')).toBe('video')
})

test('a raised ceiling waits while the path was congested recently', () => {
  // Arrange
  const abr = new AdaptiveBitrate()
  abr.restart(0)
  congest(abr, SETTLE_MS, 5_000)
  abr.decide(5_000, LADDER, 'video_480p')

  // Act / Assert
  expect(abr.raiseTo(6_000, LADDER, 'video_360p')).toBeUndefined()
  expect(abr.raiseTo(15_000, LADDER, 'video_360p')).toBe('video')
})

test('a minute without congestion halves a lengthened calm spell again', () => {
  // Arrange: a step up at 13 s fails at 18 s, doubling the spell to 20 s; the decision at 78 s, a minute later, halves it
  const abr = new AdaptiveBitrate()
  abr.restart(0)
  deliver(abr, SETTLE_MS, 13_000, 4_000, () => 100)
  abr.decide(13_000, LADDER, 'video_360p')
  abr.restart(13_000)
  congest(abr, 16_000, 18_000)
  abr.decide(18_000, LADDER, 'video_480p')
  abr.decide(78_000, LADDER, 'video')
  abr.restart(78_000)

  // Act: the spell after the switch at 78 s settles at 81 s
  const choice = abr.decide(91_000, LADDER, 'video_720p')

  // Assert: a 20 s spell would not have stepped up yet
  expect(choice).toBe('video')
})

test('a rendition taller than the picture needs gives way to the top of the capped ladder', () => {
  // Act / Assert
  expect(renditionAboveHeight(TALL_LADDER, 'video', 400)).toBe('video_480p')
  expect(renditionAboveHeight(TALL_LADDER, 'video_480p', 400)).toBeUndefined()
  expect(renditionAboveHeight(TALL_LADDER, 'video', undefined)).toBeUndefined()
  expect(renditionAboveHeight(TALL_LADDER, 'other', 400)).toBeUndefined()
})

test('a picture shrunk below the playing rendition steps down at once, before the switch settles', () => {
  // Arrange
  const abr = new AdaptiveBitrate()
  abr.restart(0)

  // Act
  const choice = abr.decideUnderHeight(1_000, TALL_LADDER, 'video', 400)

  // Assert
  expect(choice).toMatchObject({ name: 'video_480p', reason: 'height' })
})

test('a calm path steps up no higher than the picture needs', () => {
  // Arrange
  const abr = new AdaptiveBitrate()
  abr.restart(0)
  deliver(abr, SETTLE_MS, 13_000, 4_000, () => 100)

  // Act
  const fromBelow = abr.decideUnderHeight(13_000, TALL_LADDER, 'video_360p', 400)
  const atCap = abr.decideUnderHeight(13_000, TALL_LADDER, 'video_480p', 400)

  // Assert
  expect(fromBelow).toMatchObject({ name: 'video_480p', reason: 'calm' })
  expect(atCap).toBeUndefined()
})

test('a picture grown taller raises to the top of the new cap at once', () => {
  // Arrange: settled on the cap of a 400 pixel picture
  const abr = new AdaptiveBitrate()
  abr.restart(0)
  deliver(abr, SETTLE_MS, 5_000, 4_000, () => 100)
  abr.decideUnderHeight(5_000, TALL_LADDER, 'video_480p', 400)

  // Act
  const choice = abr.decideUnderHeight(5_500, TALL_LADDER, 'video_480p', 1_000)

  // Assert
  expect(choice).toMatchObject({ name: 'video', reason: 'raise' })
})

test('a step down for congestion carries the lag and throughput it was decided on', () => {
  // Arrange: 0.96 Mbps arrives, its least lag rising from 100 ms to 1.1 s
  const abr = new AdaptiveBitrate()
  abr.restart(0)
  congest(abr, SETTLE_MS, 5_000)

  // Act
  const choice = abr.decideUnderHeight(5_000, TALL_LADDER, 'video_720p', undefined)

  // Assert
  expect(choice).toMatchObject({ name: 'video_360p', reason: 'congested', recentLagMs: 1_100, baselineLagMs: 100 })
  expect(choice?.throughputBps).toBeCloseTo(4_000 * 8 * (1_000 / FRAME_MS), -5)
})

test('a high latency tolerance rides out a lag a low one steps down on', () => {
  // Arrange: the least lag stands a second above its best
  const low = new AdaptiveBitrate()
  const high = new AdaptiveBitrate()
  high.setLatencyTolerance('high')
  for (const abr of [low, high]) {
    abr.restart(0)
    congest(abr, SETTLE_MS, 5_000)
  }

  // Act / Assert
  expect(low.decide(5_000, LADDER, 'video_720p')).toBe('video_360p')
  expect(high.decide(5_000, LADDER, 'video_720p')).toBeUndefined()
})

test('a latency tolerance in milliseconds steps down past that excess lag', () => {
  // Arrange
  const abr = new AdaptiveBitrate()
  abr.setLatencyTolerance(900)
  abr.restart(0)
  congest(abr, SETTLE_MS, 5_000)

  // Act / Assert: the excess of 1 s is past 900 ms
  expect(abr.decide(5_000, LADDER, 'video_720p')).toBe('video_360p')
})

test('a latency tolerance rejects what is not a duration', () => {
  // Arrange
  const abr = new AdaptiveBitrate()

  // Act / Assert
  expect(() => abr.setLatencyTolerance(Infinity)).not.toThrow()
  expect(() => abr.setLatencyTolerance(-1)).toThrow(RangeError)
  expect(() => abr.setLatencyTolerance(Number.NaN)).toThrow(RangeError)
  expect(() => abr.setLatencyTolerance('medium' as 'low')).toThrow(RangeError)
})
