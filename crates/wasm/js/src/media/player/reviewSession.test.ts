import { expect, test } from '@playwright/test'
import type { MediaCatalogTrack } from '../catalog'
import { ReviewGroupStore } from './reviewGroupStore'
import { type ReviewHost, ReviewSession, type ReviewWindowResult, reviewWindowBytes } from './reviewSession'
import { FakeSession, receivedObject } from './testing/fakeSession'
import { followingVideoTimeline, groupStartMicros, timelineObject, vodTracks } from './testing/mediaTimelineFixtures'
import type { SeekTimeline } from './seekTimeline'
import { TrackSubscriptions } from './trackSubscriptions'

const FIRST_GROUP = 100
const LAST_GROUP = 109

/// Records the start of each window it is given to play; `ended` resolves with
/// the `throughMicros` of `reachedEnd`. A paused host holds each window it is
/// given, as a paused player does.
function recordingHost(tracks: MediaCatalogTrack[], paused = false) {
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
    playWindow: (window) => {
      played.push(window.start)
      return paused ? new Promise(() => {}) : Promise.resolve(true)
    },
    reachedEnd: (_session, throughMicros) => resolveEnded(throughMicros),
    windowStarted: () => {},
    paused: () => paused,
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

function vodTimeline(tracks: MediaCatalogTrack[]): SeekTimeline {
  const timeline = followingVideoTimeline()
  timeline.applyCatalogTracks(tracks)
  timeline.replaceMediaTimeline(timelineObject(FIRST_GROUP, LAST_GROUP), 'video')
  return timeline
}

function groupStore(): ReviewGroupStore<ReviewWindowResult> {
  return new ReviewGroupStore(reviewWindowBytes)
}

/// A review of the group `groupId` that the position reached moving away from the live edge.
function reviewOf(
  groupId: number,
  session: FakeSession,
  timeline: SeekTimeline,
  host: ReviewHost,
  groups: ReviewGroupStore<ReviewWindowResult>
): ReviewSession {
  const target = timeline.groups.resolveSeekTarget(groupStartMicros(FIRST_GROUP, groupId))
  if (!target) {
    throw new Error('the timeline lists no group')
  }
  return new ReviewSession(
    session.context(),
    timeline,
    new TrackSubscriptions(session.context(), () => {}),
    host,
    target,
    target.captureMicros,
    groups,
    -1n
  )
}

function videoFetches(session: FakeSession): string[] {
  return session.fetches.filter((fetch) => fetch.startsWith('video '))
}

test('a review of a broadcast that is not live fetches every window to the last group, then reaches the end', async () => {
  // Arrange
  const session = new FakeSession(undefined, [], vodObjects())
  const tracks = vodTracks(20_000)
  const { host, played, ended } = recordingHost(tracks)
  const review = reviewOf(FIRST_GROUP, session, vodTimeline(tracks), host, groupStore())

  // Act
  review.start()
  const throughMicros = await ended

  // Assert: the first window is the target group alone, and the newest group is fetched as well, rather than waited on to close
  expect(played).toEqual([100n, 101n, 106n])
  expect(throughMicros).toBe(groupStartMicros(FIRST_GROUP, LAST_GROUP))
})

test('a paused review fetches its group and the two it was moving toward, not the window after it', async () => {
  // Arrange
  const session = new FakeSession(undefined, [], vodObjects())
  const tracks = vodTracks(20_000)
  const { host, played } = recordingHost(tracks, true)
  const review = reviewOf(105, session, vodTimeline(tracks), host, groupStore())

  // Act
  review.start()

  // Assert
  await expect.poll(() => videoFetches(session)).toEqual(['video 105-105', 'video 104-104', 'video 103-103'])
  expect(played).toEqual([105n])
})

test('a review of a group an earlier review fetched beside it plays without fetching it again', async () => {
  // Arrange: a paused review of group 105 has fetched groups 104 and 103 as well
  const session = new FakeSession(undefined, [], vodObjects())
  const tracks = vodTracks(20_000)
  const timeline = vodTimeline(tracks)
  const { host, played } = recordingHost(tracks, true)
  const groups = groupStore()
  const first = reviewOf(105, session, timeline, host, groups)
  first.start()
  await expect.poll(() => videoFetches(session)).toHaveLength(3)
  first.end()

  // Act
  reviewOf(104, session, timeline, host, groups).start()

  // Assert: group 104 plays from the store, and only group 102 is new beside it
  await expect.poll(() => played).toEqual([105n, 104n])
  await expect
    .poll(() => videoFetches(session))
    .toEqual(['video 105-105', 'video 104-104', 'video 103-103', 'video 102-102'])
})
