import { expect, test } from '@playwright/test'
import type { MediaCatalogTrack } from '../catalog'
import type { SpliceStart } from './renditionSplice'
import { FakeTrackSubscriptions } from './testing/fakeTrackSubscriptions'
import { VideoSwitcher } from './videoSwitcher'

const CAMERA: MediaCatalogTrack = { name: 'camera', label: 'camera', altGroup: 1 }

/// A switcher from `camera_low`, whose newest group is 4, to `camera`.
async function switchingToCamera() {
  const subscriptions = new FakeTrackSubscriptions()
  const outgoing = subscriptions.subscribed('video', 'camera_low')
  const played: string[] = []
  const handovers: SpliceStart[] = []
  const switcher = new VideoSwitcher({
    subscriptions,
    play: (trackName, groupId, object) => played.push(`${trackName} ${groupId}/${object.objectId}`),
    handedOver: (_next, start) => handovers.push(start),
    log: () => {}
  })
  await switcher.begin(outgoing, 'camera', CAMERA, 4n, 'camera_low')
  return { subscriptions, switcher, played, handovers }
}

test('the incoming rendition takes over at the group after the newest outgoing one', async () => {
  // Arrange
  const { subscriptions, switcher, played, handovers } = await switchingToCamera()

  // Act
  subscriptions.deliver('camera', 5n, 0n)

  // Assert
  expect(played).toEqual(['camera 5/0'])
  expect(handovers).toEqual([{ groupId: 5n, aligned: true }])
  expect(subscriptions.calls).toEqual([
    'open camera',
    'requestNewGroup camera undefined',
    'handOver camera',
    'endBeforeGroup camera_low 5'
  ])
  expect(switcher.acceptOutgoing('camera_low', 4n)).toBe(true)
  expect(switcher.acceptOutgoing('camera_low', 5n)).toBe(false)
})

test('a seek after the handover keeps the incoming rendition from playing the group before its start', async () => {
  // Arrange: the incoming rendition's first object is in the outgoing rendition's newest group, so it starts at 5
  const { subscriptions, switcher, played } = await switchingToCamera()
  subscriptions.deliver('camera', 4n, 0n)

  // Act
  switcher.abandon()
  subscriptions.deliver('camera', 4n, 1n)
  subscriptions.deliver('camera', 5n, 0n)

  // Assert
  expect(played).toEqual(['camera 5/0'])
  expect(subscriptions.calls).toContain('retire camera_low')
})

test('a switch abandoned before the handover closes the incoming rendition and returns the name selected before it', async () => {
  // Arrange
  const { subscriptions, switcher, played } = await switchingToCamera()

  // Act
  const previousName = switcher.abandon()
  subscriptions.deliver('camera', 5n, 0n)

  // Assert
  expect(previousName).toBe('camera_low')
  expect(played).toEqual([])
  expect(subscriptions.calls).toContain('close camera')
  expect(switcher.acceptOutgoing('camera_low', 5n)).toBe(true)
})
