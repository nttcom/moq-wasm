import { expect, test } from '@playwright/test'
import { createBroadcast, type LocalSource } from './localBroadcast'
import { FakeCaptureEnvironment, FakeTrack } from './testing/fakeCaptureEnvironment'

let environment: FakeCaptureEnvironment

test.beforeEach(() => {
  environment = FakeCaptureEnvironment.install()
})

test.afterEach(() => {
  environment.uninstall()
})

async function cameraSource(camera: FakeTrack): Promise<LocalSource> {
  const broadcast = await createBroadcast(environment.session, ['room'])
  return broadcast.addSource('camera', { video: camera as unknown as MediaStreamTrack })
}

function settled(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve))
}

function holdRemoval(): () => void {
  const removal = Promise.withResolvers<void>()
  environment.broadcast.sources[0].removal = removal.promise
  return removal.resolve
}

test('setVideoEncoding during a video removal starts no encoder for the removed track', async () => {
  // Arrange
  const camera = new FakeTrack('video')
  const source = await cameraSource(camera)
  const finishRemoval = holdRemoval()
  const removing = source.setVideo(false)
  await settled()

  // Act
  source.setVideoEncoding({ main: { bitrate: 500_000 } })
  finishRemoval()
  await removing
  await settled()

  // Assert
  expect(environment.encoders(camera)).toHaveLength(0)
})

test('a video set during a video removal is published and previewed after it', async () => {
  // Arrange
  const camera = new FakeTrack('video')
  const next = new FakeTrack('video')
  const source = await cameraSource(camera)
  const finishRemoval = holdRemoval()
  const removing = source.setVideo(false)
  await settled()

  // Act
  const replacing = source.setVideo(next as unknown as MediaStreamTrack)
  finishRemoval()
  await Promise.all([removing, replacing])
  await source.setMuted('video', true)
  await source.setMuted('video', false)

  // Assert: the unmute encodes the track set during the removal
  expect(source.stream.getVideoTracks()).toEqual([next])
  expect(environment.encoders(next)).toHaveLength(1)
})

test('addSource rejects and releases the camera when the broadcast closes while its tracks are declared', async () => {
  // Arrange
  const broadcast = await createBroadcast(environment.session, ['room'])
  const declaring = Promise.withResolvers<void>()
  environment.broadcast.declaring = declaring.promise
  const adding = broadcast.addSource('camera', { video: true })
  await environment.broadcast.declareCalled

  // Act
  await broadcast.close()
  declaring.resolve()

  // Assert
  await expect(adding).rejects.toThrow('the broadcast is closed')
  expect(environment.capturedTracks.map(({ stopped }) => stopped)).toEqual([true])
  expect(environment.workers).toEqual([])
  expect(broadcast.source('camera')).toBeUndefined()
})
