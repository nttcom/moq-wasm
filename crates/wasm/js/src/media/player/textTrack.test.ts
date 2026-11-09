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
