import { expect, test } from '@playwright/test'
import { type LocHeader, TIMESTAMP_ID } from '../internal/loc'
import { SeekTimeline } from './seekTimeline'
import {
  FIRST_ENCODED_AT_MS,
  GROUP_MS,
  followingVideoTimeline,
  timeline,
  timelineObject,
  vodTracks
} from './testing/mediaTimelineFixtures'

test('a later catalog updates the tracks the followed timeline depends on', () => {
  // Arrange: the source's video is muted when the first catalog arrives
  const seek = new SeekTimeline()
  const first = seek.followMediaTimeline(timeline('video_timeline', ['audio']))

  // Act
  const again = seek.followMediaTimeline(timeline('video_timeline', ['video', 'audio']))
  const other = seek.followMediaTimeline(timeline('screenshare_timeline', ['screenshare']))

  // Assert
  expect([first, again, other]).toEqual([true, false, false])
  expect(seek.mediaTimelineTrackName).toBe('video_timeline')
  expect(seek.stampedByMediaTimeline('video')).toBe(true)
  expect(seek.stampedByMediaTimeline('screenshare')).toBe(false)
})

test('a seek reaches a group only the media timeline lists', () => {
  // Arrange
  const seek = followingVideoTimeline()
  seek.replaceMediaTimeline(timelineObject(100, 120), 'video')

  // Act
  const target = seek.groups.resolveSeekTarget((FIRST_ENCODED_AT_MS + 5 * GROUP_MS + 500) * 1_000)

  // Assert: group 120 is still open
  expect(target?.groupId).toBe(105n)
  expect(seek.groups.newestClosed?.groupId).toBe(119n)
  expect(seek.axis().replayableSeconds).toBe(40)
})

test('the groups of another source are no seek targets', () => {
  // Arrange
  const seek = followingVideoTimeline()

  // Act
  seek.replaceMediaTimeline(timelineObject(100, 120), 'screenshare')

  // Assert
  expect(seek.groups.latest).toBeUndefined()
  expect(seek.axis().seekable).toBe(false)
})

test('a group a FETCH found evicted stays forgotten while the timeline still lists it', () => {
  // Arrange
  const seek = followingVideoTimeline()
  seek.replaceMediaTimeline(timelineObject(100, 120), 'video')
  seek.groups.forgetThrough(101n)

  // Act
  seek.replaceMediaTimeline(timelineObject(100, 121), 'video')

  // Assert
  expect(seek.groups.resolveSeekTarget(0)?.groupId).toBe(102n)
})

test('a timeline object replaces the groups the previous one listed', () => {
  // Arrange
  const seek = followingVideoTimeline()
  seek.replaceMediaTimeline(timelineObject(100, 120), 'video')

  // Act: the publisher no longer keeps groups 100 to 109
  seek.replaceMediaTimeline(timelineObject(110, 130), 'video')

  // Assert
  expect(seek.groups.resolveSeekTarget(0)?.groupId).toBe(110n)
  expect(seek.groups.latest?.groupId).toBe(130n)
})

test('the audio of the source the timeline follows shares the video ids even when its groups start later', () => {
  // Arrange: the first audio of group 101 starts 800 ms after its video keyframe
  const seek = followingVideoTimeline()
  seek.replaceMediaTimeline(timelineObject(100, 120), 'video')
  const audioStart: LocHeader = [
    { id: TIMESTAMP_ID, value: { varint: (FIRST_ENCODED_AT_MS + GROUP_MS + 800) * 1_000 } }
  ]
  seek.audio.record(101n, audioStart)

  // Act
  const sameSource = seek.audioSharesVideoIds('video', 'audio')
  const otherSource = seek.audioSharesVideoIds('screenshare', 'audio')

  // Assert
  expect([sameSource, otherSource]).toEqual([true, false])
})

test('the axis of a broadcast that is not live runs from the first timeline record for its duration', () => {
  // Arrange: groups 100 to 109 start every 2 s, and the last one lasts 2 s
  const seek = followingVideoTimeline()
  seek.applyCatalogTracks(vodTracks(20_000))

  // Act
  seek.replaceMediaTimeline(timelineObject(100, 109), 'video')

  // Assert
  const startSeconds = FIRST_ENCODED_AT_MS / 1_000
  expect(seek.live).toBe(false)
  expect(seek.axis()).toEqual({
    liveEdgeSeconds: startSeconds + 20,
    replayableStartSeconds: startSeconds,
    broadcastStartSeconds: startSeconds,
    replayableSeconds: 20,
    seekable: true
  })
})

test('the newest group of a broadcast that is not live is closed and a seek target', () => {
  // Arrange
  const seek = followingVideoTimeline()
  seek.applyCatalogTracks(vodTracks(20_000))
  seek.replaceMediaTimeline(timelineObject(100, 109), 'video')

  // Act
  const target = seek.groups.resolveSeekTarget((FIRST_ENCODED_AT_MS + 30_000) * 1_000)

  // Assert
  expect(target?.groupId).toBe(109n)
  expect(seek.groups.newestClosed?.groupId).toBe(109n)
})

test('a broadcast with a track that is live, or that does not say, stays live', () => {
  // Arrange
  const seek = new SeekTimeline()
  const [video, audio] = vodTracks(20_000)

  // Act
  seek.applyCatalogTracks([video, { ...audio, isLive: true }])
  const withLiveAudio = seek.live
  seek.applyCatalogTracks([video, { name: 'audio', label: 'audio' }])
  const withGivenAudio = seek.live

  // Assert
  expect([withLiveAudio, withGivenAudio]).toEqual([true, true])
})
