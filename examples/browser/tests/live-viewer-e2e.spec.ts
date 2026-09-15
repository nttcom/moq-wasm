import { expect, test, type Locator } from '@playwright/test'
import { arrangeLiveViewerE2ESession, type LiveViewerPageModel } from './live-viewer-e2e-arrange'

test('live viewer plays the ingested stream, switches renditions and rewinds', async ({ browser }) => {
  // Arrange
  const { context, viewer } = await arrangeLiveViewerE2ESession(browser)

  try {
    // Assert
    await expect(viewer.seekbar).toBeDisabled()
    await expect(viewer.seekElapsed).toHaveText('--:-- / --:--')

    // Act
    await viewer.watchButton.click()

    // Assert
    await expect(viewer.connectionStatus).toContainText('Connected:')
    await expect(viewer.catalogStatus).toContainText(/Catalog loaded: [1-9]/)
    await expect(viewer.playbackStatus).toContainText('Playing')
    await expectVideoDecoded(viewer)

    // Act: relay のキャッシュが 10 秒分たまるのを待って巻き戻す
    await expect.poll(async () => parseSeconds(viewer.rewindBuffer), { timeout: 60_000 }).toBeGreaterThan(10)
    await expect(viewer.seekbar).toBeEnabled()
    await expect(viewer.seekPosition).toHaveText('LIVE')
    await expect(viewer.seekElapsed).toHaveText(/^\d+:\d{2} \/ \d+:\d{2}$/)
    await viewer.seekbar.focus()
    await viewer.seekbar.press('Home')

    // Assert
    await expect(viewer.rewindStatus).toContainText(/Rewound \d/)
    await expect(viewer.playbackStatus).toContainText('Reviewing')
    await expect(viewer.reviewCanvas).toBeVisible()
    await expect
      .poll(async () => viewer.reviewCanvas.evaluate((element) => (element as HTMLCanvasElement).width), {
        timeout: 30_000
      })
      .toBe(await viewer.video.evaluate((element) => (element as HTMLVideoElement).videoWidth))

    await expect(viewer.seekPosition).toHaveText(/-\d+\.\ds/)

    // Assert: MSF media timeline が配信開始からの経過を返し、シーク位置がライブより手前になる
    const [atPosition, broadcast] = (await viewer.seekElapsed.innerText()).split(' / ').map(parseClock)
    expect(atPosition).toBeLessThan(broadcast)

    // Act
    await viewer.seekbar.press('End')

    // Assert
    await expect(viewer.rewindStatus).toContainText('Live')
    await expect(viewer.seekPosition).toHaveText('LIVE')
    await expect(viewer.video).toBeVisible()
    await expect(viewer.playbackStatus).toContainText('Playing')

    // Act
    const slider = await viewer.seekbar.boundingBox()
    expect(slider).not.toBeNull()
    await viewer.page.mouse.move(slider!.x + slider!.width - 8, slider!.y + slider!.height / 2)
    await viewer.page.mouse.down()
    await viewer.page.mouse.move(slider!.x + slider!.width / 2, slider!.y + slider!.height / 2, { steps: 5 })

    // Assert
    await expect(viewer.seekPosition).toHaveText(/-\d+\.\ds/)
    await expect(viewer.rewindStatus).toHaveText('Live')

    // Act
    await viewer.page.mouse.up()

    // Assert
    await expect(viewer.rewindStatus).toContainText(/Rewound \d/)

    // Act
    await viewer.liveButton.click()
    await viewer.rewind10Button.click()
    await viewer.liveButton.click()

    // Assert
    await expect(viewer.seekPosition).toHaveText('LIVE')
    await expect(viewer.reviewCanvas).toBeHidden()

    // Act: 下位画質へ切り替える
    const renditions = await viewer.videoTrackSelect.locator('option').allInnerTexts()
    expect(renditions.length).toBeGreaterThan(1)
    await viewer.videoTrackSelect.selectOption({ index: 1 })

    // Assert: 元の track を解除して rendition を購読し直す
    await expect(viewer.logPanel).toContainText('unsubscribed video')
    await expect(viewer.logPanel).toContainText(/subscribed \S*\/video_\d+p/)
    await expect(viewer.seekPosition).toHaveText('LIVE')

    // Act
    await viewer.stopButton.click()

    // Assert
    await expect(viewer.rewindBuffer).toHaveText('0.0s')
    await expect(viewer.seekbar).toBeDisabled()
    await expect(viewer.seekPosition).toHaveText('LIVE')
    await expect(viewer.seekElapsed).toHaveText('--:-- / --:--')
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

function parseClock(text: string): number {
  return text.split(':').reduce((total, part) => total * 60 + Number(part), 0)
}
