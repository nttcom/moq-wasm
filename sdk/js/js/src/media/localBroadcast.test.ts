import { expect, test } from '@playwright/test'
import { createBroadcast, type LocalSource } from './localBroadcast'
import { FakeCaptureEnvironment, FakeTrack } from './testing/fakeCaptureEnvironment'
import { settle } from '../testing/settle'
import { collectReportedErrors } from './internal/testing/reportedErrors'

const reported = collectReportedErrors()

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
  await settle()

  // Act
  source.setVideoEncoding({ main: { bitrate: 500_000 } })
  finishRemoval()
  await removing
  await settle()

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
  await settle()

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

test('the video is encoded while a subscriber takes its CMAF sibling and not its LOC track', async () => {
  // Arrange
  const camera = new FakeTrack('video')
  await cameraSource(camera)
  const [encoder] = environment.encoders(camera)
  environment.broadcast.onDemandChange('camera', false)
  environment.broadcast.onDemandChange('camera_cmaf', false)

  // Act
  environment.broadcast.onDemandChange('camera_cmaf', true)
  environment.broadcast.onDemandChange('camera', false)

  // Assert
  expect(encoder.messages.filter(({ type }) => type === 'wanted').at(-1)).toMatchObject({ rendition: 0, wanted: true })
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

test('createBroadcast offers each event timeline it is given, in order', async () => {
  // Act
  await createBroadcast(environment.session, ['room'], {
    eventTimelines: [
      { name: 'action', eventType: 'io.example.action.v1', priority: 0 },
      { name: 'notes', eventType: 'io.example.notes.v1' }
    ]
  })

  // Assert
  expect(environment.broadcast.eventTimelines).toEqual([
    ['action', 'io.example.action.v1', 0],
    ['notes', 'io.example.notes.v1', undefined]
  ])
})

test('an onTrackEnded that throws is reported, the other listener is told and the source is still removed', async () => {
  // Arrange
  const broadcast = await createBroadcast(environment.session, ['room'])
  const source = await broadcast.addSource('camera', { video: true })
  const error = new Error('onTrackEnded bug')
  const heard: string[] = []
  source.listen({
    onTrackEnded: () => {
      throw error
    }
  })
  source.listen({ onTrackEnded: (kind) => heard.push(`track ${kind}`), onEnded: () => heard.push('ended') })

  // Act
  environment.capturedTracks[0].end()
  await settle()

  // Assert
  expect(reported).toEqual([error])
  expect(heard).toEqual(['track video', 'ended'])
  expect(broadcast.source('camera')).toBeUndefined()
})

test('an onError that throws is reported and the later failures still reach it', async () => {
  // Arrange
  const error = new Error('onError bug')
  const heard: unknown[] = []
  const broadcast = await createBroadcast(environment.session, ['room'], {
    onError: (failure) => {
      heard.push(String(failure))
      throw error
    }
  })
  await broadcast.addSource('camera', { video: new FakeTrack('video') as unknown as MediaStreamTrack })
  const encoder = environment.workers[0] as unknown as { onmessage(event: { data: object }): void }

  // Act
  encoder.onmessage({ data: { type: 'encoderError', rendition: 0, message: 'first' } })
  encoder.onmessage({ data: { type: 'encoderError', rendition: 0, message: 'second' } })

  // Assert
  expect(reported).toEqual([error, error])
  expect(heard).toEqual(['Error: video encoder 0: encoderError: first', 'Error: video encoder 0: encoderError: second'])
})
