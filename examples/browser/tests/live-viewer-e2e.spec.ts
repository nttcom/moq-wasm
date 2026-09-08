import { expect, test, type Page } from '@playwright/test'

const PAGE_PATH = '/moq-wasm/examples/live-viewer/index.html'
const MOQT_URL = process.env.MEDIA_E2E_MOQT_URL ?? 'https://127.0.0.1:4433'
const NAMESPACE = process.env.LIVE_VIEWER_E2E_NAMESPACE ?? 'live'

test('live viewer plays the ingested stream and switches renditions', async ({ page }) => {
  // Arrange
  await page.goto(`${PAGE_PATH}?moqtUrl=${encodeURIComponent(MOQT_URL)}&trackNamespace=${NAMESPACE}`, {
    waitUntil: 'domcontentloaded'
  })

  // Act
  await page.getByTestId('live-viewer-watch-button').click()

  // Assert
  await expect(page.getByTestId('live-viewer-connection-status')).toContainText('Connected:')
  await expect(page.getByTestId('live-viewer-catalog-status')).toContainText(/Catalog loaded: [1-9]/)
  await expect(page.getByTestId('live-viewer-playback-status')).toContainText('Playing')
  await expectVideoPlaying(page)

  // Act: 下位画質へ切り替える
  const select = page.getByTestId('live-viewer-video-track-select')
  const renditions = await select.locator('option').allInnerTexts()
  expect(renditions.length).toBeGreaterThan(1)
  await select.selectOption({ index: 1 })

  // Assert
  await expect(page.getByTestId('live-viewer-playback-status')).toContainText('video_')
  await expectVideoPlaying(page)
})

async function expectVideoPlaying(page: Page): Promise<void> {
  const video = page.getByTestId('live-viewer-video')
  await expect
    .poll(async () => video.evaluate((element) => (element as HTMLVideoElement).readyState), { timeout: 30_000 })
    .toBeGreaterThanOrEqual(2)
  await expect
    .poll(async () => video.evaluate((element) => (element as HTMLVideoElement).videoWidth))
    .toBeGreaterThan(0)
}
