import { expect, test } from '@playwright/test'
import { createBroadcast } from './localBroadcast'
import { FakeCaptureEnvironment } from './testing/fakeCaptureEnvironment'

let environment: FakeCaptureEnvironment

test.beforeEach(() => {
  environment = FakeCaptureEnvironment.install()
})

test.afterEach(() => {
  environment.uninstall()
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
