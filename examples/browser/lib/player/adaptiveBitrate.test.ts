import { expect, test } from '@playwright/test'
import type { MediaCatalogTrack } from '../msf/catalog'
import { AdaptiveBitrate, type Rung, bitrateLadder, capLadderToHeight } from './adaptiveBitrate'

const LADDER: Rung[] = [
  { name: 'video_360p', bitrate: 700_000 },
  { name: 'video_480p', bitrate: 1_200_000 },
  { name: 'video_720p', bitrate: 2_500_000 },
  { name: 'video', bitrate: 6_000_000 }
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
  const choice = abr.decide(8_000, LADDER, 'video', false)

  // Assert: 0.8 × 1.7 Mbps fits 480p but not 720p
  expect(choice).toBe('video_480p')
})

test('a stall steps down even while the lag holds', () => {
  // Arrange
  const abr = new AdaptiveBitrate()
  abr.restart(0)
  deliver(abr, SETTLE_MS, 6_000, 3_000, () => 100)

  // Act
  const choice = abr.decide(6_000, LADDER, 'video_720p', true)

  // Assert: 0.8 × 0.73 Mbps fits nothing above the lowest rendition
  expect(choice).toBe('video_360p')
})

test('a single late keyframe is not taken for congestion', () => {
  // Arrange
  const abr = new AdaptiveBitrate()
  abr.restart(0)
  deliver(abr, SETTLE_MS, 6_000, 6_000, (atMs) => (atMs > 5_500 && atMs < 5_600 ? 900 : 100))

  // Act / Assert
  expect(abr.decide(6_000, LADDER, 'video', false)).toBeUndefined()
})

test('a calm spell steps up one rendition', () => {
  // Arrange
  const abr = new AdaptiveBitrate()
  abr.restart(0)
  deliver(abr, SETTLE_MS, SETTLE_MS + 10_000, 4_000, () => 100)

  // Act
  const choice = abr.decide(SETTLE_MS + 10_000, LADDER, 'video_360p', false)

  // Assert
  expect(choice).toBe('video_480p')
})

test('nothing is decided while the new rendition settles', () => {
  // Arrange
  const abr = new AdaptiveBitrate()
  abr.restart(0)

  // Act / Assert
  expect(abr.decide(SETTLE_MS - 1, LADDER, 'video', true)).toBeUndefined()
})

test('a step up that has to step down again doubles the calm spell before the next one', () => {
  // Arrange: step up at 13 s, congestion right after it
  const abr = new AdaptiveBitrate()
  abr.restart(0)
  deliver(abr, SETTLE_MS, 13_000, 4_000, () => 100)
  abr.decide(13_000, LADDER, 'video_360p', false)
  abr.restart(13_000)
  deliver(abr, 16_000, 18_000, 4_000, () => 100)
  abr.decide(18_000, LADDER, 'video_480p', true)
  abr.restart(18_000)
  deliver(abr, 21_000, 41_000, 4_000, () => 100)

  // Act
  const early = abr.decide(31_000 + 1, LADDER, 'video_360p', false)
  const late = abr.decide(41_000, LADDER, 'video_360p', false)

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
