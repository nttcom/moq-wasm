import { expect, test } from '@playwright/test'
import { BroadcastCatalog, type BroadcastCatalogListener } from './broadcastCatalog'
import type { PublishDone } from '../../core/session'
import { FakeTextTrackSession } from './testing/fakeTextTrackSession'

type Heard = { catalogs: string[]; absences: (PublishDone | undefined)[] }

function recordingListener(): { listener: BroadcastCatalogListener; heard: Heard } {
  const heard: Heard = { catalogs: [], absences: [] }
  return {
    listener: {
      onCatalog: (text) => heard.catalogs.push(text),
      onPublisherAbsent: (done) => heard.absences.push(done)
    },
    heard
  }
}

/// Lets the reader of a subscription take what was delivered.
function settle(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0))
}

test('every listener reads the catalog of one SUBSCRIBE, a late one at once', async () => {
  // Arrange
  const fake = new FakeTextTrackSession()
  const catalog = new BroadcastCatalog(fake.session, ['room', 'alice'])
  const first = recordingListener()
  const late = recordingListener()
  catalog.listen(first.listener)

  // Act
  await Promise.all([catalog.start(), catalog.start()])
  fake.open('catalog')[0].deliver(1n, 'v1')
  await settle()
  catalog.listen(late.listener)

  // Assert
  expect(fake.subscriptions).toHaveLength(1)
  expect(first.heard.catalogs).toEqual(['v1'])
  expect(late.heard.catalogs).toEqual(['v1'])
})

test('a catalog of an older group than the newest one is ignored', async () => {
  // Arrange
  const fake = new FakeTextTrackSession()
  const catalog = new BroadcastCatalog(fake.session, ['room', 'alice'])
  const { listener, heard } = recordingListener()
  catalog.listen(listener)
  await catalog.start()
  const [subscription] = fake.open('catalog')

  // Act
  subscription.deliver(5n, 'new')
  subscription.deliver(4n, 'replaced publisher')
  await settle()

  // Assert
  expect(heard.catalogs).toEqual(['new'])
})

test('a removed listener hears nothing more', async () => {
  // Arrange
  const fake = new FakeTextTrackSession()
  const catalog = new BroadcastCatalog(fake.session, ['room', 'alice'])
  const { listener, heard } = recordingListener()
  const unlisten = catalog.listen(listener)
  await catalog.start()

  // Act
  unlisten()
  fake.open('catalog')[0].deliver(1n, 'v1')
  await settle()

  // Assert
  expect(heard.catalogs).toEqual([])
})

test('a namespace nobody publishes is reported absent, then read once its publisher appears', async () => {
  // Arrange
  const fake = new FakeTextTrackSession()
  fake.absent = true
  const catalog = new BroadcastCatalog(fake.session, ['room', 'alice'])
  const { listener, heard } = recordingListener()
  catalog.listen(listener)

  // Act
  await catalog.start()
  fake.absent = false
  await expect.poll(() => fake.open('catalog').length, { timeout: 5_000 }).toBe(1)
  fake.open('catalog')[0].deliver(1n, 'v1')
  await settle()

  // Assert
  expect(heard.absences).toEqual([undefined])
  expect(heard.catalogs).toEqual(['v1'])
})

test('a PUBLISH_DONE of the catalog reports the publisher absent with it and subscribes again', async () => {
  // Arrange
  const fake = new FakeTextTrackSession()
  const catalog = new BroadcastCatalog(fake.session, ['room', 'alice'])
  const { listener, heard } = recordingListener()
  catalog.listen(listener)
  await catalog.start()
  const done: PublishDone = { code: 0n, reason: 'left' }

  // Act
  fake.open('catalog')[0].end(done)
  await settle()

  // Assert
  expect(heard.absences).toEqual([done])
  await expect.poll(() => fake.subscriptions.length, { timeout: 5_000 }).toBe(2)
})

test('close unsubscribes the catalog and a later start subscribes it again', async () => {
  // Arrange
  const fake = new FakeTextTrackSession()
  const catalog = new BroadcastCatalog(fake.session, ['room', 'alice'])
  await catalog.start()

  // Act
  await catalog.close()
  const openAfterClose = fake.open('catalog').length
  await catalog.start()

  // Assert
  expect(openAfterClose).toBe(0)
  expect(fake.open('catalog')).toHaveLength(1)
})
