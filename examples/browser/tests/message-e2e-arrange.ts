import type { Browser, BrowserContext, Page } from '@playwright/test'
import { MESSAGE_INDEX_PATH } from '../playwright.helpers'

const moqtUrl = process.env.MESSAGE_E2E_MOQT_URL ?? 'https://127.0.0.1:4433'
const namespace = process.env.MESSAGE_E2E_NAMESPACE ?? 'anon/e2e/moqt-message'

export const messageE2EConfig = {
  moqtUrl,
  namespace
}

function buildPagePath(moqtUrl: string): string {
  const params = new URLSearchParams({ moqtUrl })
  return `${MESSAGE_INDEX_PATH}?${params.toString()}`
}

function createMessagePageModel(page: Page) {
  return {
    urlInput: page.locator('#url'),
    connectButton: page.locator('#connectBtn'),
    closeButton: page.locator('#closeBtn'),
    logPanel: page.locator('#logPanel'),
    sendStatus: page.locator('#send-status'),
    received: page.locator('#received-text'),
    announceNamespaceInput: page.locator('#announce-namespace'),
    announceButton: page.locator('#announceBtn'),
    withdrawButton: page.locator('#withdrawBtn'),
    watchPrefixInput: page.locator('#watch-prefix'),
    watchButton: page.locator('#watchBtn'),
    unwatchButton: page.locator('#unwatchBtn'),
    publishNamespaceInput: page.locator('#publish-namespace'),
    publishTrackInput: page.locator('#publish-track'),
    publishTrackButton: page.locator('#publishTrackBtn'),
    writePayloadInput: page.locator('#write-payload'),
    writeGroupButton: page.locator('#writeGroupBtn'),
    subscribeNamespaceInput: page.locator('#subscribe-namespace'),
    subscribeTrackInput: page.locator('#subscribe-track'),
    subscribeForwardCheckbox: page.locator('#subscribe-forward'),
    subscribeButton: page.locator('#subscribeBtn'),
    setForwardButton: page.locator('#setForwardBtn'),
    unsubscribeButton: page.locator('#unsubscribeBtn'),
    fetchNamespaceInput: page.locator('#fetch-namespace'),
    fetchTrackNameInput: page.locator('#fetch-track'),
    fetchStartGroupInput: page.locator('#fetch-start-group'),
    fetchStartObjectInput: page.locator('#fetch-start-object'),
    fetchEndGroupInput: page.locator('#fetch-end-group'),
    fetchEndObjectInput: page.locator('#fetch-end-object'),
    fetchButton: page.locator('#fetchBtn'),
    trackStatusNamespaceInput: page.locator('#track-status-namespace'),
    trackStatusTrackInput: page.locator('#track-status-track'),
    trackStatusButton: page.locator('#trackStatusBtn')
  }
}

export type MessagePageModel = ReturnType<typeof createMessagePageModel>

export async function arrangeMessageE2ESession(browser: Browser) {
  const context = await browser.newContext()
  return { context, client: await openMessagePage(context, moqtUrl) }
}

export async function openMessagePage(context: BrowserContext, relayUrl: string): Promise<MessagePageModel> {
  const page = await context.newPage()
  await page.goto(buildPagePath(relayUrl), { waitUntil: 'domcontentloaded' })
  return createMessagePageModel(page)
}
