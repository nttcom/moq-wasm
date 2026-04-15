import { expect, test, type Page } from '@playwright/test'
import { MEDIA_PUBLISHER_PATH, MEDIA_SUBSCRIBER_PATH } from '../playwright.helpers'

const moqtUrl = process.env.MEDIA_E2E_MOQT_URL ?? 'https://127.0.0.1:4433'
const namespace = process.env.MEDIA_E2E_NAMESPACE ?? 'e2e/moqt-media'

function buildPagePath(path: string): string {
  const params = new URLSearchParams({
    moqtUrl,
    trackNamespace: namespace
  })
  return `${path}?${params.toString()}`
}

async function openExamplePage(page: Page, path: string): Promise<void> {
  await page.goto(buildPagePath(path), { waitUntil: 'domcontentloaded' })
}

test('publisher streams video to subscriber over local MoQT server', async ({ browser }) => {
  const context = await browser.newContext({
    ignoreHTTPSErrors: true,
    permissions: ['camera', 'microphone']
  })
  const publisherPage = await context.newPage()
  const subscriberPage = await context.newPage()

  await openExamplePage(publisherPage, MEDIA_PUBLISHER_PATH)
  await openExamplePage(subscriberPage, MEDIA_SUBSCRIBER_PATH)

  await expect(publisherPage.getByTestId('publisher-url-input')).toHaveValue(moqtUrl)
  await expect(subscriberPage.getByTestId('subscriber-url-input')).toHaveValue(moqtUrl)
  await expect(publisherPage.getByTestId('publisher-namespace-input')).toHaveValue(namespace)
  await expect(subscriberPage.getByTestId('subscriber-namespace-input')).toHaveValue(namespace)

  await publisherPage.getByTestId('publisher-connect-button').click()
  await expect(publisherPage.getByTestId('publisher-connection-status')).toContainText('Connected:')
  await publisherPage.getByTestId('publisher-setup-button').click()
  await expect(publisherPage.getByTestId('publisher-setup-status')).toContainText('Setup acknowledged')
  await publisherPage.getByTestId('publisher-announce-button').click()
  await expect(publisherPage.getByTestId('publisher-announce-status')).toContainText(`Announced: ${namespace}`)

  await subscriberPage.getByTestId('subscriber-connect-button').click()
  await expect(subscriberPage.getByTestId('subscriber-connection-status')).toContainText('Connected:')
  await subscriberPage.getByTestId('subscriber-setup-button').click()
  await expect(subscriberPage.getByTestId('subscriber-setup-status')).toContainText('Setup acknowledged')
  await subscriberPage.getByTestId('subscriber-catalog-subscribe-button').click()
  await expect(subscriberPage.getByTestId('subscriber-catalog-status')).toContainText('Catalog loaded:')
  await subscriberPage.getByTestId('subscriber-track-subscribe-button').click()
  await expect(publisherPage.getByTestId('publisher-send-status')).toContainText('Subscriber ready:')

  await publisherPage.getByTestId('publisher-get-user-media-button').click()
  await expect(publisherPage.getByTestId('publisher-capture-status')).toContainText('Media ready')
  await publisherPage.getByTestId('publisher-send-button').click()
  await expect(publisherPage.getByTestId('publisher-send-status')).toHaveText(/Publishing started|Streaming video/)

  await expect(subscriberPage.getByTestId('subscriber-receive-status')).toHaveText(/Received video objects: [1-9]/)
  await expect(subscriberPage.getByTestId('subscriber-playback-status')).toContainText('Playing')

  const videoState = subscriberPage.getByTestId('subscriber-video')
  await expect.poll(async () => videoState.evaluate((video) => video.readyState)).toBeGreaterThanOrEqual(2)
  await expect.poll(async () => videoState.evaluate((video) => video.currentTime)).toBeGreaterThan(0)
  await expect.poll(async () => videoState.evaluate((video) => video.paused)).toBe(false)

  const playback = await videoState.evaluate((video) => ({
    currentTime: video.currentTime,
    readyState: video.readyState
  }))
  expect(playback.readyState).toBeGreaterThanOrEqual(2)
  expect(playback.currentTime).toBeGreaterThan(0)

  await context.close()
})
