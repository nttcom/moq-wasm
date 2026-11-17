import { expect, test } from '@playwright/test'
import { FakeSession } from './testing/fakeSession'
import { TextTracks } from './textTrack'

test('a failed UNSUBSCRIBE resolves, so the other text tracks are still unsubscribed', async () => {
  // Arrange
  const session = new FakeSession(undefined, [])
  const textTracks = new TextTracks(session.context())
  await textTracks.subscribe('catalog', () => {})
  await textTracks.subscribe('video_timeline', () => {})
  session.failedClose = new Error('connection lost')

  // Act
  await textTracks.unsubscribeAll()

  // Assert
  expect(session.updates).toEqual(['close catalog', 'close video_timeline'])
})

test('a text track is subscribed ahead of audio and video, and the catalog ahead of it', async () => {
  // Arrange
  const session = new FakeSession(undefined, [])
  const textTracks = new TextTracks(session.context())

  // Act
  await textTracks.subscribe('captions', () => {})
  await textTracks.subscribe('catalog', () => {}, { currentGroup: true, priority: 0 })

  // Assert
  expect(session.calls).toEqual([
    { method: 'subscribe', track: 'captions', options: { priority: 16 } },
    { method: 'subscribeCurrentGroup', track: 'catalog', options: { priority: 0 } }
  ])
})

test('a text track whose objects replace each other takes the delivery options it is given', async () => {
  // Arrange
  const session = new FakeSession(undefined, [])
  const textTracks = new TextTracks(session.context())

  // Act
  await textTracks.subscribe('video_timeline', () => {}, {
    delivery: { deliveryTimeoutMs: 2_000, groupOrder: 'descending' }
  })

  // Assert
  expect(session.calls).toEqual([
    {
      method: 'subscribe',
      track: 'video_timeline',
      options: { priority: 16, deliveryTimeoutMs: 2_000, groupOrder: 'descending' }
    }
  ])
})
