import type { Browser, BrowserContext, Locator, Page } from '@playwright/test'
import { MEDIA_PUBLISHER_PATH, MEDIA_SUBSCRIBER_PATH } from '../playwright.helpers'

const moqtUrl = process.env.MEDIA_E2E_MOQT_URL ?? 'https://127.0.0.1:4433'
const namespace = process.env.MEDIA_E2E_NAMESPACE ?? 'e2e/moqt-media'

export const mediaE2EConfig = {
  moqtUrl,
  namespace
}

export interface PublisherPageModel {
  page: Page
  urlInput: Locator
  namespaceInput: Locator
  connectButton: Locator
  connectionStatus: Locator
  setupButton: Locator
  setupStatus: Locator
  announceButton: Locator
  announceStatus: Locator
  getUserMediaButton: Locator
  captureStatus: Locator
  sendButton: Locator
  sendStatus: Locator
}

export interface SubscriberPageModel {
  page: Page
  urlInput: Locator
  namespaceInput: Locator
  connectButton: Locator
  connectionStatus: Locator
  setupButton: Locator
  setupStatus: Locator
  catalogSubscribeButton: Locator
  catalogStatus: Locator
  trackSubscribeButton: Locator
  receiveStatus: Locator
  playbackStatus: Locator
  video: Locator
}

export interface MediaE2ESession {
  context: BrowserContext
  publisher: PublisherPageModel
  subscriber: SubscriberPageModel
}

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

function createPublisherPageModel(page: Page): PublisherPageModel {
  return {
    page,
    urlInput: page.getByTestId('publisher-url-input'),
    namespaceInput: page.getByTestId('publisher-namespace-input'),
    connectButton: page.getByTestId('publisher-connect-button'),
    connectionStatus: page.getByTestId('publisher-connection-status'),
    setupButton: page.getByTestId('publisher-setup-button'),
    setupStatus: page.getByTestId('publisher-setup-status'),
    announceButton: page.getByTestId('publisher-announce-button'),
    announceStatus: page.getByTestId('publisher-announce-status'),
    getUserMediaButton: page.getByTestId('publisher-get-user-media-button'),
    captureStatus: page.getByTestId('publisher-capture-status'),
    sendButton: page.getByTestId('publisher-send-button'),
    sendStatus: page.getByTestId('publisher-send-status')
  }
}

function createSubscriberPageModel(page: Page): SubscriberPageModel {
  return {
    page,
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

export async function arrangeMediaE2ESession(browser: Browser): Promise<MediaE2ESession> {
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
