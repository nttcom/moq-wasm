import type { Browser, BrowserContext, Locator, Page } from '@playwright/test'
import { MESSAGE_INDEX_PATH } from '../playwright.helpers'

const moqtUrl = process.env.MESSAGE_E2E_MOQT_URL ?? 'https://127.0.0.1:4433'
const namespace = process.env.MESSAGE_E2E_NAMESPACE ?? 'e2e/moqt-message'

export const messageE2EConfig = {
  moqtUrl,
  namespace
}

export interface MessagePageModel {
  page: Page
  urlInput: Locator
  connectButton: Locator
  logPanel: Locator
  sendStatus: Locator
  setupButton: Locator
  goAwayButton: Locator
  goAwayUriInput: Locator
  maxRequestIdButton: Locator
  requestsBlockedButton: Locator
  fetchCancelButton: Locator
  subscribeUpdateButton: Locator
  publishDoneButton: Locator
  publishNamespaceCancelButton: Locator
  publishNamespaceCancelInput: Locator
  trackStatusButton: Locator
  trackStatusNamespaceInput: Locator
}

export interface MessageE2ESession {
  context: BrowserContext
  client: MessagePageModel
}

function buildPagePath(): string {
  const params = new URLSearchParams({ moqtUrl })
  return `${MESSAGE_INDEX_PATH}?${params.toString()}`
}

function createMessagePageModel(page: Page): MessagePageModel {
  return {
    page,
    urlInput: page.locator('#url'),
    connectButton: page.locator('#connectBtn'),
    logPanel: page.locator('#logPanel'),
    sendStatus: page.locator('#send-status'),
    setupButton: page.locator('#sendSetupBtn'),
    goAwayButton: page.locator('#sendGoAwayBtn'),
    goAwayUriInput: page.locator('#goaway-new-session-uri'),
    maxRequestIdButton: page.locator('#sendMaxRequestIdBtn'),
    requestsBlockedButton: page.locator('#sendRequestsBlockedBtn'),
    fetchCancelButton: page.locator('#sendFetchCancelBtn'),
    subscribeUpdateButton: page.locator('#sendSubscribeUpdateBtn'),
    publishDoneButton: page.locator('#sendPublishDoneBtn'),
    publishNamespaceCancelButton: page.locator('#sendPublishNamespaceCancelBtn'),
    publishNamespaceCancelInput: page.locator('#publish-namespace-cancel-track-namespace'),
    trackStatusButton: page.locator('#sendTrackStatusBtn'),
    trackStatusNamespaceInput: page.locator('#track-status-track-namespace')
  }
}

export async function arrangeMessageE2ESession(browser: Browser): Promise<MessageE2ESession> {
  const context = await browser.newContext()
  const page = await context.newPage()
  await page.goto(buildPagePath(), { waitUntil: 'domcontentloaded' })

  return { context, client: createMessagePageModel(page) }
}
