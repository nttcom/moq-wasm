import { type Page, expect, test } from '@playwright/test'
import { arrangeMediaE2ESession, mediaE2EConfig, type PublisherPageModel } from './media-e2e-arrange'
import type { ViewSnapshot } from './remote-broadcast/main'

const PAGE_PATH = '/moq-wasm/tests/remote-broadcast/index.html'
/// Long enough for a 15 fps camera to deliver several video objects.
const DELIVERY_WAIT_MS = 1_500

/// Publishes under `namespace`, or the namespace the media E2E opens the publisher with.
async function startPublishing(publisher: PublisherPageModel, namespace?: string[]): Promise<void> {
  if (namespace) {
    await publisher.namespaceInput.fill(namespace.join('/'))
  }
  await publisher.connectButton.click()
  await expect(publisher.connectionStatus).toContainText('Connected:')
  await publisher.getUserMediaButton.click()
  await expect(publisher.captureStatus).toContainText('Media ready')
  await publisher.sendButton.click()
  await expect(publisher.sendStatus).toContainText('Publishing started')
}

function views(page: Page): Promise<ViewSnapshot[]> {
  return page.evaluate(() => window.remoteBroadcastE2E.snapshot())
}

async function camera(page: Page): Promise<ViewSnapshot | undefined> {
  return (await views(page)).find(({ name }) => name === 'camera')
}

test('a remote broadcast shows each source the catalog lists, in a view the page may move', async ({ browser }) => {
  // Arrange: the media publisher publishes its camera
  const session = await arrangeMediaE2ESession(browser)
  const page = await session.context.newPage()
  await page.goto(PAGE_PATH)
  await startPublishing(session.publisher)

  try {
    // Act: the page plays the broadcast
    await page.evaluate(({ url, namespace }) => window.remoteBroadcastE2E.start(url, namespace), {
      url: mediaE2EConfig.moqtUrl,
      namespace: mediaE2EConfig.namespace.split('/')
    })

    // Assert
    await expect
      .poll(() => views(page), { timeout: 20_000 })
      .toEqual([expect.objectContaining({ name: 'camera', video: 'playing', audio: 'playing', hasVideoSize: true })])

    // Act / Assert: a muted video turns the view's video off and an unmuted one plays again
    await session.publisher.muteVideoButton.click()
    await expect.poll(async () => (await camera(page))?.video, { timeout: 20_000 }).toBe('off')
    await session.publisher.muteVideoButton.click()
    await expect.poll(async () => (await camera(page))?.video, { timeout: 20_000 }).toBe('playing')

    // Act: move the view to another slot in one task
    await page.evaluate(() => window.remoteBroadcastE2E.move('camera', 'second'))
    const moved = await camera(page)
    await page.waitForTimeout(DELIVERY_WAIT_MS)
    const afterMove = await camera(page)

    // Assert: the moved view keeps receiving its video
    expect(afterMove).toEqual(expect.objectContaining({ parent: 'second', video: 'playing' }))
    expect(afterMove?.videoObjects).toBeGreaterThan(moved?.videoObjects ?? 0)

    // Act
    await page.evaluate(() => window.remoteBroadcastE2E.move('camera', 'off-screen'))
    await page.waitForTimeout(DELIVERY_WAIT_MS)
    const hidden = await camera(page)
    await page.waitForTimeout(DELIVERY_WAIT_MS)
    const stillHidden = await camera(page)

    // Assert: a view off screen receives no video
    expect(stillHidden?.videoObjects).toBe(hidden?.videoObjects)

    // Act: bring the view back on screen
    await page.evaluate(() => window.remoteBroadcastE2E.move('camera', 'first'))

    // Assert: its video is subscribed again
    await expect
      .poll(async () => (await camera(page))?.videoObjects, { timeout: 20_000 })
      .toBeGreaterThan(stillHidden?.videoObjects ?? 0)
    await expect.poll(async () => (await camera(page))?.video, { timeout: 20_000 }).toBe('playing')

    // Act / Assert: a shared screen adds a view without audio, and stopping it removes the view
    await session.publisher.shareScreenButton.click()
    await expect
      .poll(async () => (await views(page)).map(({ name, audio }) => `${name}:${audio}`), {
        timeout: 20_000
      })
      .toEqual(['camera:playing', 'screen:off'])
    await session.publisher.shareScreenButton.click()
    await expect.poll(async () => (await views(page)).map(({ name }) => name), { timeout: 20_000 }).toEqual(['camera'])

    // Assert: each source was added once, and the screen's element left the page before onSourceRemove
    await expect
      .poll(() => page.evaluate(() => window.remoteBroadcastE2E.events()))
      .toEqual(['add camera', 'add screen', 'remove screen attached=false'])
  } finally {
    await session.context.close()
  }
})

test('watchBroadcasts plays the broadcasts under a prefix but its own and the filtered ones', async ({ browser }) => {
  // Arrange: the page publishes its own broadcast and a bot's next to the media publisher's, under a prefix of their own
  // as a session of an earlier test may still hold its namespace
  const session = await arrangeMediaE2ESession(browser)
  const page = await session.context.newPage()
  await page.goto(PAGE_PATH)
  const prefix = [...mediaE2EConfig.namespace.split('/'), 'watch']
  const publisher = [...prefix, 'publisher']
  const own = [...prefix, 'own']
  const bot = [...prefix, 'bot']

  try {
    // Act
    await page.evaluate(({ url, prefix, own, bot }) => window.remoteBroadcastE2E.startWatching(url, prefix, own, bot), {
      url: mediaE2EConfig.moqtUrl,
      prefix,
      own,
      bot
    })
    await startPublishing(session.publisher, publisher)

    // Assert
    await expect
      .poll(() => page.evaluate(() => window.remoteBroadcastE2E.watched()), { timeout: 20_000 })
      .toEqual([`add ${publisher.join('/')}`])

    // Act
    await session.publisher.closeButton.click()

    // Assert: the closed publisher's namespace is withdrawn and its broadcast removed
    await expect
      .poll(() => page.evaluate(() => window.remoteBroadcastE2E.watched()), { timeout: 20_000 })
      .toEqual([`add ${publisher.join('/')}`, `remove ${publisher.join('/')}`])
  } finally {
    await session.context.close()
  }
})
