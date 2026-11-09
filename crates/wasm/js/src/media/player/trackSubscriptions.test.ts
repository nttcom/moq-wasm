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

test('a subscription from the current group marks the objects up to the largest one as cached', async () => {
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
  expect(session.calls.map((call) => call.method)).toEqual(['subscribeCurrentGroup'])
  expect(received).toEqual([
    [3n, 0n, true],
    [3n, 1n, true],
    [3n, 2n, false],
    [4n, 0n, false]
  ])
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

test('a subscription from the current group waits for the next group while forwarding is paused', async () => {
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
