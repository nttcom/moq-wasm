import { expect, test } from '@playwright/test'
import { ONVIF_PATH } from '../playwright.helpers'
import { expectVideoDecoded, liveViewerE2EConfig } from './live-viewer-e2e-arrange'

test('the ONVIF page plays the camera broadcast with controls and stops on disconnect', async ({ browser }) => {
  // Arrange
  const context = await browser.newContext({ ignoreHTTPSErrors: true })
  const page = await context.newPage()
  await page.goto(`${ONVIF_PATH}?${new URLSearchParams({ moqtUrl: liveViewerE2EConfig.moqtUrl }).toString()}`, {
    waitUntil: 'domcontentloaded'
  })
  await page.locator('#subscribe-namespace').evaluate((input, namespace) => {
    ;(input as HTMLInputElement).value = namespace
  }, liveViewerE2EConfig.namespace)
  const stage = page.locator('#stage')

  try {
    // Act
    await page.locator('#connect-btn').click()

    // Assert
    await expectVideoDecoded(stage.getByTestId('live-player-video'))
    await expect(stage.getByTestId('live-player-seekbar')).toHaveCount(1)
    await expect(page.locator('#video-stats')).not.toHaveText('-')

    // Act
    await page.locator('#disconnect-btn').click()

    // Assert
    await expect(stage.getByTestId('live-player-video')).toHaveCount(0)
    await expect(stage.getByTestId('live-player-seekbar')).toHaveCount(0)
  } finally {
    await context.close()
  }
})
