import { expect, test } from '@playwright/test'
import type { PublishDone } from '../../core/session'
import type { MediaCatalogTrack } from '../catalog'
import { AddedAudioTrack } from './addedAudioTrack'
import { LivePlayout } from './livePlayout'
import { FakeMediaEnvironment } from './testing/fakeMediaEnvironment'
import { FakeSession, receivedObject } from './testing/fakeSession'

const ENGLISH: MediaCatalogTrack = { name: 'audio_en', label: 'en' }
const ENDED = { code: 0, reason: 'time limit' } as unknown as PublishDone

let environment: FakeMediaEnvironment

test.beforeEach(() => {
  environment = FakeMediaEnvironment.install({ outputLatencySeconds: 0 })
  // The lane decodes in a worker, which the unit tests do not run.
  Object.assign(globalThis, {
    Worker: class {
      postMessage(): void {}
      terminate(): void {}
    }
  })
})

test.afterEach(() => {
  environment.uninstall()
  Reflect.deleteProperty(globalThis, 'Worker')
})

function playout(): LivePlayout {
  return new LivePlayout((frame) => frame.close())
}

test('an added audio track joins its current group, and closing it unsubscribes once', async () => {
  // Arrange
  const session = new FakeSession({ groupId: 3n, objectId: 0n }, [receivedObject(3n, 0n), receivedObject(3n, 1n)])

  // Act
  const added = await AddedAudioTrack.open(session.context(), playout(), ENGLISH, {}, () => {})
  await session.drained
  await added.close()
  await added.close()

  // Assert
  expect(session.calls).toEqual([{ method: 'subscribeCurrentGroup', track: 'audio_en' }])
  expect(added.receivedObjects).toBe(2)
  expect(session.updates).toEqual(['close audio_en'])
})

test('closing an added audio track resolves when its UNSUBSCRIBE fails', async () => {
  // Arrange
  const session = new FakeSession(undefined, [])
  session.failedClose = new Error('session closing')
  const added = await AddedAudioTrack.open(session.context(), playout(), ENGLISH, {}, () => {})

  // Act / Assert
  await expect(added.close()).resolves.toBeUndefined()
})

test('the PUBLISH_DONE of an added audio track reaches the listener only while it is open', async () => {
  // Arrange
  const open = new FakeSession(undefined, [])
  const closed = new FakeSession(undefined, [])
  const ended: string[] = []
  await AddedAudioTrack.open(open.context(), playout(), ENGLISH, { onEnded: () => ended.push('open') }, () => {})
  const closedTrack = await AddedAudioTrack.open(
    closed.context(),
    playout(),
    ENGLISH,
    { onEnded: () => ended.push('closed') },
    () => {}
  )
  await closedTrack.close()

  // Act
  open.publishDone('audio_en', ENDED)
  closed.publishDone('audio_en', ENDED)
  await Promise.resolve()
  await Promise.resolve()

  // Assert
  expect(ended).toEqual(['open'])
})
