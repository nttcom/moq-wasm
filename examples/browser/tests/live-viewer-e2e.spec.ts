import { expect, test, type Locator } from '@playwright/test'
import { arrangeLiveViewerE2ESession, type LiveViewerPageModel } from './live-viewer-e2e-arrange'

test('live viewer plays the ingested stream, switches renditions and rewinds', async ({ browser }) => {
  // Arrange
  const { context, viewer } = await arrangeLiveViewerE2ESession(browser)

  try {
    // Act
    await viewer.watchButton.click()

    // Assert
    await expect(viewer.connectionStatus).toContainText('Connected:')
    await expect(viewer.catalogStatus).toContainText(/Catalog loaded: [1-9]/)
    await expect(viewer.playbackStatus).toContainText('Playing')
    await expectVideoDecoded(viewer)

    // Act: relay のキャッシュが 10 秒分たまるのを待って巻き戻す
    await expect.poll(async () => parseSeconds(viewer.rewindBuffer), { timeout: 60_000 }).toBeGreaterThan(10)
    await viewer.rewind10Button.click()

    // Assert
    await expect(viewer.rewindStatus).toContainText(/Rewound \d/)
    await expect(viewer.playbackStatus).toContainText('Reviewing')
    await expect(viewer.reviewCanvas).toBeVisible()
    await expect
      .poll(async () => viewer.reviewCanvas.evaluate((element) => (element as HTMLCanvasElement).width), {
        timeout: 30_000
      })
      .toBeGreaterThan(0)

    // Act
    await viewer.liveButton.click()

    // Assert
    await expect(viewer.rewindStatus).toContainText('Live')
    await expect(viewer.video).toBeVisible()
    await expect(viewer.playbackStatus).toContainText('Playing')

    // Act: 下位画質へ切り替える
    const renditions = await viewer.videoTrackSelect.locator('option').allInnerTexts()
    expect(renditions.length).toBeGreaterThan(1)
    await viewer.videoTrackSelect.selectOption({ index: 1 })

    // Assert: 元の track を解除して rendition を購読し直す
    await expect(viewer.logPanel).toContainText('unsubscribed video')
    await expect(viewer.logPanel).toContainText(/subscribed \S*\/video_\d+p/)
    await expect(viewer.rewindBuffer).toHaveText('0.0s')
  } finally {
    await context.close()
  }
})

async function expectVideoDecoded(viewer: LiveViewerPageModel): Promise<void> {
  await expect
    .poll(async () => viewer.video.evaluate((element) => (element as HTMLVideoElement).readyState), {
      timeout: 30_000
    })
    .toBeGreaterThanOrEqual(2)
  await expect
    .poll(async () => viewer.video.evaluate((element) => (element as HTMLVideoElement).videoWidth))
    .toBeGreaterThan(0)
}

async function parseSeconds(locator: Locator): Promise<number> {
  return Number.parseFloat((await locator.innerText()).replace('s', ''))
}
