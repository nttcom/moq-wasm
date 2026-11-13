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

test('the delivery timeout is sent with a subscription from a group start and with a plain one', async () => {
  // Arrange
  const session = new FakeSession(undefined, [])
  const subscriptions = new TrackSubscriptions(session.context(), () => {}, 2_000)
  const { handler } = recordingHandler()

  // Act
  await subscriptions.subscribe('video', 'video', VIDEO, handler, true)
  await subscriptions.subscribe('audio', 'audio', { name: 'audio', label: 'audio' }, handler, false)

  // Assert
  expect(session.calls).toEqual([
    { method: 'subscribeGroupStart', track: 'video', newGroupAfter: 60, priority: 128, deliveryTimeoutMs: 2_000 },
    { method: 'subscribe', track: 'audio', options: { forward: true, priority: 0, deliveryTimeoutMs: 2_000 } }
  ])
})

test('a delivery timeout that is not a whole number of milliseconds from 1 up is refused', () => {
  // Arrange
  const session = new FakeSession(undefined, [])

  // Act / Assert
  for (const timeout of [0, -1, 1500.5, Number.NaN, 2 ** 32]) {
    expect(() => new TrackSubscriptions(session.context(), () => {}, timeout)).toThrow(RangeError)
  }
})

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
  expect(session.calls).toEqual([{ method: 'subscribeGroupStart', track: 'video', newGroupAfter: 60, priority: 128 }])
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

test('a request for a new group is sent once, with the largest group the reader knows', async () => {
  // Arrange: no group starts after the request
  const session = new FakeSession(undefined, [])
  const subscriptions = new TrackSubscriptions(session.context(), () => {})
  const video = await subscriptions.open('video', 'video', VIDEO, () => {})

  // Act
  await subscriptions.requestNewGroup(video, 3n)

  // Assert
  expect(session.updates).toEqual(['requestNewGroup video 3'])
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

test('a video priority set while the video is subscribed is sent as an update of the video only', async () => {
  // Arrange
  const session = new FakeSession(undefined, [])
  const subscriptions = new TrackSubscriptions(session.context(), () => {})
  await subscriptions.subscribe('video', 'video', VIDEO, () => {})
  await subscriptions.subscribe('audio', 'audio', { name: 'audio', label: 'audio' }, () => {})

  // Act
  subscriptions.setVideoPriority(7)

  // Assert
  await expect.poll(() => session.updates).toEqual(['setPriority video 7'])
})

test('a video subscribed after its priority was set is subscribed with it', async () => {
  // Arrange
  const session = new FakeSession(undefined, [])
  const subscriptions = new TrackSubscriptions(session.context(), () => {})
  subscriptions.setVideoPriority(7)

  // Act: a video from a group start, a rendition opened for a switch, and the audio
  await subscriptions.subscribe('video', 'video', VIDEO, () => {}, true)
  await subscriptions.open('video', 'video_low', VIDEO, () => {})
  await subscriptions.subscribe('audio', 'audio', { name: 'audio', label: 'audio' }, () => {})

  // Assert
  expect(session.calls.map((call) => [call.track, call.priority ?? call.options?.priority])).toEqual([
    ['video', 7],
    ['video_low', 7],
    ['audio', 0]
  ])
  expect(session.updates).toEqual([])
})

test('a priority set while the first subscription is being made is sent once it is made', async () => {
  // Arrange
  const session = new FakeSession(undefined, [])
  const subscriptions = new TrackSubscriptions(session.context(), () => {})

  // Act
  const subscribed = subscriptions.subscribe('video', 'video', VIDEO, () => {})
  subscriptions.setVideoPriority(7)
  await subscribed

  // Assert
  await expect.poll(() => session.updates).toEqual(['setPriority video 7'])
})

test('a rendition that takes over takes the priority set while it was being subscribed', async () => {
  // Arrange
  const session = new FakeSession(undefined, [])
  const subscriptions = new TrackSubscriptions(session.context(), () => {})
  await subscriptions.subscribe('video', 'video', VIDEO, () => {})
  const next = await subscriptions.open('video', 'video_low', VIDEO, () => {})
  subscriptions.setVideoPriority(7)
  await expect.poll(() => session.updates).toEqual(['setPriority video 7'])

  // Act
  subscriptions.handOver('video', next)

  // Assert
  await expect.poll(() => session.updates.at(-1)).toBe('setPriority video_low 7')
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
