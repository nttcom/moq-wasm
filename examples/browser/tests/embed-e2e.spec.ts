import { expect, test } from '@playwright/test'
import { EMBED_PATH } from '../playwright.helpers'
import { expectVideoDecoded, liveViewerE2EConfig } from './live-viewer-e2e-arrange'

test('<moq-player> plays the ingested stream, shows controls on its attribute and stops when removed', async ({
  browser
}) => {
  // Arrange
  const context = await browser.newContext({ ignoreHTTPSErrors: true })
  const page = await context.newPage()
  const params = new URLSearchParams({
    moqtUrl: liveViewerE2EConfig.moqtUrl,
    trackNamespace: liveViewerE2EConfig.namespace
  })
  await page.goto(`${EMBED_PATH}?${params.toString()}`, { waitUntil: 'domcontentloaded' })
  const player = page.getByTestId('embed-player')
  const seekbar = player.getByTestId('live-player-seekbar')

  try {
    // Act
    await page.getByTestId('embed-play-button').click()

    // Assert
    await expectVideoDecoded(player.getByTestId('live-player-video'))
    await expect(seekbar).toHaveCount(1)

    // Act
    await player.evaluate((element) => element.removeAttribute('controls'))

    // Assert: the controls leave, the picture stays
    await expect(seekbar).toHaveCount(0)
    await expect(player).not.toHaveClass(/viewer-stage/)
    await expectVideoDecoded(player.getByTestId('live-player-video'))

    // Act
    await page.getByTestId('embed-stop-button').click()

    // Assert: without `src` the player is disposed and its pictures removed
    await expect(player.getByTestId('live-player-video')).toHaveCount(0)
    await expect
      .poll(() => player.evaluate((element) => (element as HTMLElementTagNameMap['moq-player']).player))
      .toBeUndefined()
  } finally {
    await context.close()
  }
})
