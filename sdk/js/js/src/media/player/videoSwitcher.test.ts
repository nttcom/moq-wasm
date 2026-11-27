import { expect, test } from '@playwright/test'
import type { MediaCatalogTrack } from '../catalog'
import type { SpliceStart } from './renditionSplice'
import { FakeTrackSubscriptions } from './testing/fakeTrackSubscriptions'
import { VideoSwitcher } from './videoSwitcher'

const CAMERA: MediaCatalogTrack = { name: 'camera', label: 'camera', altGroup: 1 }

/// A switcher from `camera_low`, whose newest group is 4, to `camera`, whose
/// SUBSCRIBE_OK reports `incomingLargestGroup`.
async function switchingToCamera(incomingLargestGroup?: bigint) {
  const subscriptions = new FakeTrackSubscriptions()
  subscriptions.openedLargestGroup = incomingLargestGroup
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

test('the outgoing rendition is left to its PUBLISH_DONE after the incoming one passes the start group', async () => {
  // Arrange: the outgoing rendition lags, so the incoming one has reached group 6 while the outgoing one still delivers group 4
  const { subscriptions, switcher, played } = await switchingToCamera()
  subscriptions.deliver('camera', 5n, 0n)
  subscriptions.deliver('camera', 6n, 0n)

  // Act
  const tailPlays = switcher.acceptOutgoing('camera_low', 4n)
  await subscriptions.endedByPublisher('camera_low')

  // Assert: the switch is over without an UNSUBSCRIBE, so settling it retires nothing
  expect(tailPlays).toBe(true)
  expect(played).toEqual(['camera 5/0', 'camera 6/0'])
  expect(switcher.abandon()).toBeUndefined()
  expect(subscriptions.calls).not.toContain('retire camera_low')
})

test('an outgoing object past the start group retires the outgoing rendition its publisher did not end', async () => {
  // Arrange
  const { subscriptions, switcher } = await switchingToCamera()
  subscriptions.deliver('camera', 5n, 0n)

  // Act
  const plays = switcher.acceptOutgoing('camera_low', 6n)

  // Assert
  expect(plays).toBe(false)
  expect(subscriptions.calls).toContain('retire camera_low')
})

test('an End Group that cannot be sent retires the outgoing rendition', async () => {
  // Arrange
  const { subscriptions } = await switchingToCamera()
  subscriptions.endBeforeGroupError = new Error('session closed')

  // Act
  subscriptions.deliver('camera', 5n, 0n)
  await Promise.resolve()

  // Assert
  expect(subscriptions.calls).toContain('retire camera_low')
})

test('group ids that do not continue the outgoing rendition retire it at the handover', async () => {
  // Arrange
  const { subscriptions, played, handovers } = await switchingToCamera()

  // Act
  subscriptions.deliver('camera', 100n, 0n)

  // Assert
  expect(played).toEqual(['camera 100/0'])
  expect(handovers).toEqual([{ groupId: 100n, aligned: false }])
  expect(subscriptions.calls).toContain('retire camera_low')
  expect(subscriptions.calls).not.toContain('endBeforeGroup camera_low 100')
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

test('the incoming group asked for and reset before the splice is asked for once more', async () => {
  // Arrange
  const { subscriptions, switcher } = await switchingToCamera(4n)
  const incoming = subscriptions.openedFor('camera')!

  // Act
  switcher.groupReset(incoming, 5n)
  switcher.groupReset(incoming, 6n)

  // Assert: a repeat would start one more group after the one asked for
  expect(subscriptions.calls).toEqual(['open camera', 'requestNewGroup camera 4', 'requestNewGroup camera 5'])
})

test('a reset of an incoming group from before the request asks for nothing', async () => {
  // Arrange
  const { subscriptions, switcher } = await switchingToCamera(4n)

  // Act
  switcher.groupReset(subscriptions.openedFor('camera')!, 4n)

  // Assert
  expect(subscriptions.calls).toEqual(['open camera', 'requestNewGroup camera 4'])
})

test('a reset once the splice started is left to the played rendition', async () => {
  // Arrange
  const { subscriptions, switcher } = await switchingToCamera()
  subscriptions.deliver('camera', 5n, 0n)

  // Act
  switcher.groupReset(subscriptions.openedFor('camera')!, 5n)

  // Assert
  expect(subscriptions.calls.filter((call) => call.startsWith('requestNewGroup'))).toEqual([
    'requestNewGroup camera undefined'
  ])
})
