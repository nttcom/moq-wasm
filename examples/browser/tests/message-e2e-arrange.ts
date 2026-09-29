import type { Browser, BrowserContext, Locator, Page } from '@playwright/test'
import { MESSAGE_INDEX_PATH } from '../playwright.helpers'

const moqtUrl = process.env.MESSAGE_E2E_MOQT_URL ?? 'https://127.0.0.1:4433'
const namespace = process.env.MESSAGE_E2E_NAMESPACE ?? 'anon/e2e/moqt-message'

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
  fetchRequestIdInput: Locator
  fetchNamespaceInput: Locator
  fetchTrackNameInput: Locator
  fetchStartGroupInput: Locator
  fetchStartObjectInput: Locator
  fetchEndGroupInput: Locator
  fetchEndObjectInput: Locator
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

function buildPagePath(moqtUrl: string): string {
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
    fetchRequestIdInput: page.locator('#fetch-request-id'),
    fetchNamespaceInput: page.locator('#fetch-track-namespace'),
    fetchTrackNameInput: page.locator('#fetch-track-name'),
    fetchStartGroupInput: page.locator('#fetch-start-group'),
    fetchStartObjectInput: page.locator('#fetch-start-object'),
    fetchEndGroupInput: page.locator('#fetch-end-group'),
    fetchEndObjectInput: page.locator('#fetch-end-object'),
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
  return { context, client: await openMessagePage(context, moqtUrl) }
}

export async function openMessagePage(context: BrowserContext, relayUrl: string): Promise<MessagePageModel> {
  const page = await context.newPage()
  await page.goto(buildPagePath(relayUrl), { waitUntil: 'domcontentloaded' })
  return createMessagePageModel(page)
}
