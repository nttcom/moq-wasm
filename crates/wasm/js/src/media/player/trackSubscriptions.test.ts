import { expect, test } from '@playwright/test'
import type { MediaCatalogTrack } from '../catalog'
import { FakeSession, receivedObject } from './testing/fakeSession'
import type { MediaKind, ObjectHandler } from './trackContext'
import { TrackSubscriptions } from './trackSubscriptions'

const VIDEO: MediaCatalogTrack = { name: 'video', label: 'video' }

function recordingHandler(): { handler: ObjectHandler; received: [bigint, bigint, boolean][] } {
  const received: [bigint, bigint, boolean][] = []
  return {
    handler: (groupId, object, cached) => received.push([groupId, object.objectId, cached]),
    received
  }
}

test('a subscription from a group start marks the objects up to the largest one as cached', async () => {
  // Arrange
  const session = new FakeSession({ groupId: 3n, objectId: 1n }, [
    receivedObject(3n, 0n),
    receivedObject(3n, 1n),
    receivedObject(3n, 2n),
    receivedObject(4n, 0n)
  ])
  const subscriptions = new TrackSubscriptions(session.context(), () => {})
  const { handler, received } = recordingHandler()

  // Act
  await subscriptions.subscribe('video', 'video', VIDEO, handler, true)
  await session.drained

  // Assert
  expect(session.calls).toEqual([{ method: 'subscribeGroupStart', track: 'video', newGroupAfter: 60 }])
  expect(received).toEqual([
    [3n, 0n, true],
    [3n, 1n, true],
    [3n, 2n, false],
    [4n, 0n, false]
  ])
})

test('resuming forwarding asks the video for a new group, after its Forward update', async () => {
  // Arrange
  const session = new FakeSession(undefined, [])
  const subscriptions = new TrackSubscriptions(session.context(), () => {})
  await subscriptions.subscribe('video', 'video', VIDEO, () => {})
  await subscriptions.subscribe('audio', 'audio', { name: 'audio', label: 'audio' }, () => {})
  subscriptions.pauseForward()

  // Act
  subscriptions.resumeForward()
  await expect.poll(() => session.updates.at(-1)).toBe('setForward audio true')

  // Assert: delivery resumes at a group start, which audio reaches within a frame
  expect(session.updates).toEqual([
    'setForward video false',
    'setForward audio false',
    'setForward video true',
    'requestNewGroup video undefined',
    'setForward audio true'
  ])
})

test('a request for a new group is repeated while no group starts, and not once one has', async () => {
  // Arrange: `waiting` receives nothing, `started` the first object of a group
  const idle = new FakeSession(undefined, [])
  const live = new FakeSession(undefined, [receivedObject(4n, 0n)])
  const waiting = await new TrackSubscriptions(idle.context(), () => {}).open('video', 'video', VIDEO, () => {})
  const started = await new TrackSubscriptions(live.context(), () => {}).open('video', 'video', VIDEO, () => {})
  await live.drained
  const startedSubscriptions = new TrackSubscriptions(live.context(), () => {})

  // Act
  const again = new TrackSubscriptions(idle.context(), () => {}).requestNewGroup(waiting, 3n)
  started.groupStarts = 0
  const once = startedSubscriptions.requestNewGroup(started, 3n)
  started.groupStarts = 1
  await expect.poll(() => idle.updates.length, { timeout: 5_000 }).toBe(2)
  waiting.forgotten = true
  await Promise.all([again, once])

  // Assert: the relay may have held the first request back
  expect(idle.updates).toEqual(['requestNewGroup video 3', 'requestNewGroup video undefined'])
  expect(live.updates).toEqual(['requestNewGroup video 3'])
})

test('a rendition switch subscribes without a joining FETCH', async () => {
  // Arrange
  const session = new FakeSession({ groupId: 3n, objectId: 1n }, [receivedObject(4n, 0n), receivedObject(4n, 1n)])
  const subscriptions = new TrackSubscriptions(session.context(), () => {})
  const { handler, received } = recordingHandler()

  // Act
  await subscriptions.open('video', 'video_low', VIDEO, handler)
  await session.drained

  // Assert
  expect(session.calls).toEqual([
    { method: 'subscribe', track: 'video_low', options: { forward: true, priority: 128 } }
  ])
  expect(received).toEqual([
    [4n, 0n, false],
    [4n, 1n, false]
  ])
})

test('a subscription from a group start waits for the next group while forwarding is paused', async () => {
  // Arrange
  const session = new FakeSession(undefined, [])
  const subscriptions = new TrackSubscriptions(session.context(), () => {})
  subscriptions.pauseForward()

  // Act
  await subscriptions.subscribe('video', 'video', VIDEO, () => {}, true)

  // Assert
  expect(session.calls).toEqual([{ method: 'subscribe', track: 'video', options: { forward: false, priority: 128 } }])
})

test('audio is subscribed with a lower subscriber priority value than video, so the relay sends it first', async () => {
  // Arrange
  const session = new FakeSession(undefined, [])
  const subscriptions = new TrackSubscriptions(session.context(), () => {})
  const subscribe = (kind: MediaKind, name: string) =>
    subscriptions.subscribe(kind, name, { name, label: name }, () => {})

  // Act
  await subscribe('video', 'video-720p')
  await subscribe('audio', 'audio')

  // Assert
  expect(session.calls.map(({ track, options }) => [track, options?.priority])).toEqual([
    ['video-720p', 128],
    ['audio', 0]
  ])
})

test('a failed UNSUBSCRIBE resolves, so the other tracks are still unsubscribed', async () => {
  // Arrange
  const session = new FakeSession(undefined, [])
  const subscriptions = new TrackSubscriptions(session.context(), () => {})
  await subscriptions.subscribe('video', 'video', VIDEO, () => {})
  await subscriptions.subscribe('audio', 'audio', { name: 'audio', label: 'audio' }, () => {})
  session.failedClose = new Error('connection lost')

  // Act
  await subscriptions.unsubscribe('video')
  await subscriptions.unsubscribe('audio')

  // Assert
  expect(session.updates).toEqual(['close video', 'close audio'])
})

test('unsubscribing every track while forwarding is paused sends no SUBSCRIBE_UPDATE to resume it', async () => {
  // Arrange
  const session = new FakeSession(undefined, [])
  const subscriptions = new TrackSubscriptions(session.context(), () => {})
  await subscriptions.subscribe('video', 'video', VIDEO, () => {})
  await subscriptions.subscribe('audio', 'audio', { name: 'audio', label: 'audio' }, () => {})
  subscriptions.pauseForward()
  const unsubscribing = subscriptions.unsubscribeAll()

  // Act
  const resumed = subscriptions.resumeForward()
  await unsubscribing

  // Assert
  expect(resumed).toBe(false)
  expect(session.updates.filter((update) => update.startsWith('setForward') && update.endsWith('true'))).toEqual([])
  expect(session.updates.filter((update) => update.startsWith('close'))).toEqual(['close video', 'close audio'])
})

const AUDIO: MediaCatalogTrack = { name: 'audio', label: 'audio' }
const DONE = { code: 2n, reason: 'track ended' }

async function endSubscription(kinds: MediaKind[], ended: MediaKind): Promise<[MediaKind, boolean][]> {
  const session = new FakeSession(undefined, [])
  const reported: [MediaKind, boolean][] = []
  const subscriptions = new TrackSubscriptions(session.context(), (kind, _done, leading) =>
    reported.push([kind, leading])
  )
  for (const kind of kinds) {
    await subscriptions.subscribe(kind, kind, kind === 'video' ? VIDEO : AUDIO, () => {})
  }
  session.publishDone(ended, DONE)
  await new Promise((resolve) => setTimeout(resolve, 0))
  return reported
}

test('the PUBLISH_DONE of the audio leads while no video is subscribed', async () => {
  // Act
  const reported = await endSubscription(['audio'], 'audio')

  // Assert
  expect(reported).toEqual([['audio', true]])
})

test('the PUBLISH_DONE of the audio does not lead while the video is subscribed, and that of the video does', async () => {
  // Act
  const audioEnded = await endSubscription(['video', 'audio'], 'audio')
  const videoEnded = await endSubscription(['video', 'audio'], 'video')

  // Assert
  expect(audioEnded).toEqual([['audio', false]])
  expect(videoEnded).toEqual([['video', true]])
})
