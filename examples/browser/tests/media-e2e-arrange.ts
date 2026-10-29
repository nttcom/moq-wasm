import type { Browser, Page } from '@playwright/test'
import { MEDIA_PUBLISHER_PATH, MEDIA_SUBSCRIBER_PATH } from '../playwright.helpers'

const moqtUrl = process.env.MEDIA_E2E_MOQT_URL ?? 'https://127.0.0.1:4433'
const namespace = process.env.MEDIA_E2E_NAMESPACE ?? 'anon/e2e/moqt-media'
const videoCodec = process.env.MEDIA_E2E_VIDEO_CODEC ?? 'avc1.42001F'
const videoEncoderHardwareAcceleration = process.env.MEDIA_E2E_VIDEO_HARDWARE_ACCELERATION ?? 'prefer-software'
// `run-media-e2e.mjs --through-tcp-proxy` sets `websocket`.
const expectedTransport = process.env.MEDIA_E2E_EXPECTED_TRANSPORT ?? 'webtransport'

export const mediaE2EConfig = {
  moqtUrl,
  namespace,
  videoCodec,
  videoEncoderHardwareAcceleration,
  expectedTransport
}

const SESSION_TRANSPORT_LOG = /^\[moqt\] session established over (\w+) in \d+ ms$/

/** Fills as `MoqSession` and `MoqtClientWrapper` log each established session. */
function recordSessionTransports(page: Page): string[] {
  const transports: string[] = []
  page.on('console', (message) => {
    const match = SESSION_TRANSPORT_LOG.exec(message.text())
    if (match) {
      transports.push(match[1])
    }
  })
  return transports
}

function buildPagePath(path: string): string {
  const params = new URLSearchParams({
    moqtUrl,
    trackNamespace: namespace,
    videoCodec,
    videoEncoderHardwareAcceleration
  })
  return `${path}?${params.toString()}`
}

async function openExamplePage(page: Page, path: string): Promise<void> {
  await page.goto(buildPagePath(path), { waitUntil: 'domcontentloaded' })
}

function createPublisherPageModel(page: Page) {
  return {
    sessionTransports: recordSessionTransports(page),
    urlInput: page.getByTestId('publisher-url-input'),
    namespaceInput: page.getByTestId('publisher-namespace-input'),
    connectButton: page.getByTestId('publisher-connect-button'),
    connectionStatus: page.getByTestId('publisher-connection-status'),
    getUserMediaButton: page.getByTestId('publisher-get-user-media-button'),
    captureStatus: page.getByTestId('publisher-capture-status'),
    sendButton: page.getByTestId('publisher-send-button'),
    sendStatus: page.getByTestId('publisher-send-status')
  }
}

function createSubscriberPageModel(page: Page) {
  return {
    sessionTransports: recordSessionTransports(page),
    urlInput: page.getByTestId('subscriber-url-input'),
    namespaceInput: page.getByTestId('subscriber-namespace-input'),
    connectButton: page.getByTestId('subscriber-connect-button'),
    connectionStatus: page.getByTestId('subscriber-connection-status'),
    setupButton: page.getByTestId('subscriber-setup-button'),
    setupStatus: page.getByTestId('subscriber-setup-status'),
    catalogSubscribeButton: page.getByTestId('subscriber-catalog-subscribe-button'),
    catalogStatus: page.getByTestId('subscriber-catalog-status'),
    trackSubscribeButton: page.getByTestId('subscriber-track-subscribe-button'),
    receiveStatus: page.getByTestId('subscriber-receive-status'),
    playbackStatus: page.getByTestId('subscriber-playback-status'),
    video: page.getByTestId('subscriber-video')
  }
}

export type PublisherPageModel = ReturnType<typeof createPublisherPageModel>
export type SubscriberPageModel = ReturnType<typeof createSubscriberPageModel>

export async function arrangeMediaE2ESession(browser: Browser) {
  const context = await browser.newContext({
    ignoreHTTPSErrors: true,
    permissions: ['camera', 'microphone']
  })
  const publisherPage = await context.newPage()
  const subscriberPage = await context.newPage()

  await Promise.all([
    openExamplePage(publisherPage, MEDIA_PUBLISHER_PATH),
    openExamplePage(subscriberPage, MEDIA_SUBSCRIBER_PATH)
  ])

  return {
    context,
    publisher: createPublisherPageModel(publisherPage),
    subscriber: createSubscriberPageModel(subscriberPage)
  }
}
