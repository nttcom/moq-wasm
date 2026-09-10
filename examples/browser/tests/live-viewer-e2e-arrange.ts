import type { Browser, BrowserContext, Locator, Page } from '@playwright/test'
import { LIVE_VIEWER_PATH } from '../playwright.helpers'

const moqtUrl = process.env.MEDIA_E2E_MOQT_URL ?? 'https://127.0.0.1:4433'
const namespace = process.env.LIVE_VIEWER_E2E_NAMESPACE ?? 'live/e2e'

export const liveViewerE2EConfig = {
  moqtUrl,
  namespace
}

export interface LiveViewerPageModel {
  page: Page
  urlInput: Locator
  namespaceInput: Locator
  watchButton: Locator
  stopButton: Locator
  connectionStatus: Locator
  catalogStatus: Locator
  playbackStatus: Locator
  videoTrackSelect: Locator
  rewind10Button: Locator
  rewind30Button: Locator
  liveButton: Locator
  rewindStatus: Locator
  rewindBuffer: Locator
  seekbar: Locator
  seekPosition: Locator
  seekElapsed: Locator
  video: Locator
  reviewCanvas: Locator
  logPanel: Locator
}

export interface LiveViewerE2ESession {
  context: BrowserContext
  viewer: LiveViewerPageModel
}

function buildPagePath(path: string): string {
  const params = new URLSearchParams({ moqtUrl, trackNamespace: namespace })
  return `${path}?${params.toString()}`
}

function createLiveViewerPageModel(page: Page): LiveViewerPageModel {
  return {
    page,
    urlInput: page.getByTestId('live-viewer-url-input'),
    namespaceInput: page.getByTestId('live-viewer-namespace-input'),
    watchButton: page.getByTestId('live-viewer-watch-button'),
    stopButton: page.getByTestId('live-viewer-stop-button'),
    connectionStatus: page.getByTestId('live-viewer-connection-status'),
    catalogStatus: page.getByTestId('live-viewer-catalog-status'),
    playbackStatus: page.getByTestId('live-viewer-playback-status'),
    videoTrackSelect: page.getByTestId('live-viewer-video-track-select'),
    rewind10Button: page.getByTestId('live-viewer-rewind-10-button'),
    rewind30Button: page.getByTestId('live-viewer-rewind-30-button'),
    liveButton: page.getByTestId('live-viewer-live-button'),
    rewindStatus: page.getByTestId('live-viewer-rewind-status'),
    rewindBuffer: page.getByTestId('live-viewer-rewind-buffer'),
    seekbar: page.getByTestId('live-viewer-seekbar'),
    seekPosition: page.getByTestId('live-viewer-seek-position'),
    seekElapsed: page.getByTestId('live-viewer-seek-elapsed'),
    video: page.getByTestId('live-viewer-video'),
    reviewCanvas: page.getByTestId('live-viewer-review-canvas'),
    logPanel: page.getByTestId('live-viewer-log-panel')
  }
}

export async function arrangeLiveViewerE2ESession(browser: Browser): Promise<LiveViewerE2ESession> {
  const context = await browser.newContext({ ignoreHTTPSErrors: true })
  const page = await context.newPage()
  await page.goto(buildPagePath(LIVE_VIEWER_PATH), { waitUntil: 'domcontentloaded' })
  return { context, viewer: createLiveViewerPageModel(page) }
}
