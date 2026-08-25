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
  publishNamespaceButton: Locator
  publishNamespaceInput: Locator
  publishNamespaceDoneButton: Locator
  publishNamespaceDoneInput: Locator
  subscribeNamespaceButton: Locator
  subscribeNamespaceInput: Locator
  unsubscribeNamespaceButton: Locator
  unsubscribeNamespaceInput: Locator
  subscribeButton: Locator
  subscribeNamespaceFieldInput: Locator
  subscribeTrackNameInput: Locator
  unsubscribeButton: Locator
  publishButton: Locator
  publishMessageNamespaceInput: Locator
  fetchButton: Locator
  fetchNamespaceInput: Locator
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
    publishNamespaceButton: page.locator('#sendPublishNamespaceBtn'),
    publishNamespaceInput: page.locator('#publish-track-namespace'),
    publishNamespaceDoneButton: page.locator('#sendPublishNamespaceDoneBtn'),
    publishNamespaceDoneInput: page.locator('#publish-namespace-done-track-namespace'),
    subscribeNamespaceButton: page.locator('#sendSubscribeNamespaceBtn'),
    subscribeNamespaceInput: page.locator('#track-namespace-prefix'),
    unsubscribeNamespaceButton: page.locator('#sendUnsubscribeNamespaceBtn'),
    unsubscribeNamespaceInput: page.locator('#unsubscribe-namespace-prefix'),
    subscribeButton: page.locator('#sendSubscribeBtn'),
    subscribeNamespaceFieldInput: page.locator('#subscribe-track-namespace'),
    subscribeTrackNameInput: page.locator('input[name="track-name"]'),
    unsubscribeButton: page.locator('#sendUnsubscribeBtn'),
    publishButton: page.locator('#sendPublishBtn'),
    publishMessageNamespaceInput: page.locator('#publish-track-namespace-2'),
    fetchButton: page.locator('#sendFetchBtn'),
    fetchNamespaceInput: page.locator('#fetch-track-namespace'),
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
