import { expect, test } from '@playwright/test'
import type { MediaCatalogTrack } from '../catalog'
import { type ReviewHost, ReviewSession } from './reviewSession'
import { FakeSession, receivedObject } from './testing/fakeSession'
import { followingVideoTimeline, groupStartMicros, timelineObject, vodTracks } from './testing/mediaTimelineFixtures'
import { TrackSubscriptions } from './trackSubscriptions'

const FIRST_GROUP = 100
const LAST_GROUP = 109

/// Records the start of each window it is given to play; `ended` resolves with
/// the `throughMicros` of `reachedEnd`.
function recordingHost(tracks: MediaCatalogTrack[]) {
  const played: bigint[] = []
  let resolveEnded: (throughMicros: number | undefined) => void = () => {}
  const ended = new Promise<number | undefined>((resolve) => {
    resolveEnded = resolve
  })
  const host: ReviewHost = {
    playedTrack: (kind) => {
      const track = tracks.find(({ name }) => name === kind)
      return track && { name: track.name, track }
    },
    playWindow: async (window) => {
      played.push(window.start)
      return true
    },
    reachedEnd: (_session, throughMicros) => resolveEnded(throughMicros),
    windowStarted: () => {},
    fastForwardDrained: () => false,
    observeLiveVideoGroup: () => {},
    resumeLiveForward: () => {},
    goLive: () => {},
    seek: () => {},
    failed: () => {}
  }
  return { host, played, ended }
}

/// One object per group at the group's start, on `video` and `audio`.
function vodObjects() {
  const objects = []
  for (let groupId = FIRST_GROUP; groupId <= LAST_GROUP; groupId += 1) {
    objects.push(receivedObject(BigInt(groupId), 0n, groupStartMicros(FIRST_GROUP, groupId)))
  }
  return { video: objects, audio: objects }
}

test('a review of a broadcast that is not live fetches every window to the last group, then reaches the end', async () => {
  // Arrange
  const session = new FakeSession(undefined, [], vodObjects())
  const tracks = vodTracks(20_000)
  const timeline = followingVideoTimeline()
  timeline.applyCatalogTracks(tracks)
  timeline.replaceMediaTimeline(timelineObject(FIRST_GROUP, LAST_GROUP), 'video')
  const { host, played, ended } = recordingHost(tracks)
  const target = timeline.groups.resolveSeekTarget(0)
  if (!target) {
    throw new Error('the timeline lists no group')
  }
  const review = new ReviewSession(
    session.context(),
    timeline,
    new TrackSubscriptions(session.context(), () => {}),
    host,
    target,
    target.captureMicros
  )

  // Act
  review.start()
  const throughMicros = await ended

  // Assert: the newest group is fetched as well, rather than waited on to close
  expect(played).toEqual([100n, 105n])
  expect(throughMicros).toBe(groupStartMicros(FIRST_GROUP, LAST_GROUP))
})
