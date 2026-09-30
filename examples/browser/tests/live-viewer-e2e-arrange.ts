import { type Browser, type BrowserContext, type Locator, type Page, expect } from '@playwright/test'
import { execFileSync } from 'node:child_process'
import { existsSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { LIVE_VIEWER_PATH } from '../playwright.helpers'

const moqtUrl = process.env.MEDIA_E2E_MOQT_URL ?? 'https://127.0.0.1:4433'
const namespace = process.env.LIVE_VIEWER_E2E_NAMESPACE ?? 'anon/live/e2e'
const mp4Namespace = process.env.LIVE_VIEWER_MP4_E2E_NAMESPACE ?? `${namespace}-mp4`

export const liveViewerE2EConfig = {
  moqtUrl,
  namespace,
  mp4Namespace
}

export type Mp4FixtureAudio = 'aac' | 'mp3'

export const MP4_FIXTURE = { width: 320, height: 180, fps: 15, seconds: 6 }

/// The AAC file carries B-frames so that the publisher's decode-order pacing is exercised.
const FIXTURE_ENCODINGS: Record<Mp4FixtureAudio, { audioEncoder: string; videoProfile: string; bFrames: number }> = {
  aac: { audioEncoder: 'aac', videoProfile: 'main', bFrames: 2 },
  mp3: { audioEncoder: 'libmp3lame', videoProfile: 'baseline', bFrames: 0 }
}

export function mp4FixtureFileName(audio: Mp4FixtureAudio): string {
  return `moqt-live-viewer-mp4-e2e-${audio}-${FIXTURE_ENCODINGS[audio].videoProfile}.mp4`
}

export function ensureMp4Fixture(audio: Mp4FixtureAudio): string {
  const filePath = join(tmpdir(), mp4FixtureFileName(audio))
  if (existsSync(filePath)) {
    return filePath
  }
  execFileSync('ffmpeg', [
    '-hide_banner',
    '-loglevel',
    'error',
    '-f',
    'lavfi',
    '-i',
    `testsrc=size=${MP4_FIXTURE.width}x${MP4_FIXTURE.height}:rate=${MP4_FIXTURE.fps}`,
    '-f',
    'lavfi',
    '-i',
    'sine=frequency=1000:sample_rate=48000',
    '-t',
    String(MP4_FIXTURE.seconds),
    '-c:v',
    'libx264',
    '-preset',
    'veryfast',
    '-profile:v',
    FIXTURE_ENCODINGS[audio].videoProfile,
    '-bf',
    String(FIXTURE_ENCODINGS[audio].bFrames),
    '-pix_fmt',
    'yuv420p',
    '-g',
    String(MP4_FIXTURE.fps),
    '-c:a',
    FIXTURE_ENCODINGS[audio].audioEncoder,
    '-ac',
    '1',
    '-movflags',
    '+faststart',
    filePath
  ])
  return filePath
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
  skipBack5Button: Locator
  skipForward1Button: Locator
  playPauseButton: Locator
  volumeSlider: Locator
  videoStats: Locator
  liveButton: Locator
  rewindStatus: Locator
  seekbar: Locator
  seekPosition: Locator
  seekElapsed: Locator
  seekStart: Locator
  seekReviewProgress: Locator
  qualityButton: Locator
  qualityMenu: Locator
  packagingSelect: Locator
  speedSelect: Locator
  seekAvailableWindow: Locator
  video: Locator
  reviewCanvas: Locator
  visibleVideo: Locator
  logPanel: Locator
  mp4FileInput: Locator
  mp4LoopInput: Locator
  publishButton: Locator
  stopPublishButton: Locator
  publishStatus: Locator
  publishStreams: Locator
  publishStreamMonitor: Locator
  publishStreamStats: Locator
  publishPreview: Locator
  publishLatency: Locator
}

export interface LiveViewerE2ESession {
  context: BrowserContext
  viewer: LiveViewerPageModel
}

function buildPagePath(path: string, trackNamespace: string): string {
  const params = new URLSearchParams({ moqtUrl, trackNamespace })
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
    videoTrackSelect: page.getByTestId('live-player-video-track-select'),
    skipBack5Button: page.getByTestId('live-player-skip-back-5-button'),
    skipForward1Button: page.getByTestId('live-player-skip-forward-1-button'),
    playPauseButton: page.getByTestId('live-player-play-pause-button'),
    volumeSlider: page.getByTestId('live-player-volume'),
    videoStats: page.getByTestId('live-viewer-video-stats'),
    liveButton: page.getByTestId('live-player-live-button'),
    rewindStatus: page.getByTestId('live-viewer-rewind-status'),
    seekbar: page.getByTestId('live-player-seekbar'),
    seekPosition: page.getByTestId('live-player-seek-position'),
    seekElapsed: page.getByTestId('live-player-seek-elapsed'),
    seekStart: page.getByTestId('live-player-seek-start'),
    seekReviewProgress: page.getByTestId('live-player-seek-review-progress'),
    qualityButton: page.getByTestId('live-player-quality-button'),
    qualityMenu: page.getByTestId('live-player-quality-menu'),
    packagingSelect: page.getByTestId('live-player-packaging-select'),
    speedSelect: page.getByTestId('live-player-speed-select'),
    seekAvailableWindow: page.getByTestId('live-player-seek-available-window'),
    video: page.getByTestId('live-player-video'),
    reviewCanvas: page.getByTestId('live-player-review-canvas'),
    visibleVideo: page.locator('.viewer-stage video:visible'),
    logPanel: page.getByTestId('live-viewer-log-panel'),
    mp4FileInput: page.getByTestId('live-viewer-mp4-file-input'),
    mp4LoopInput: page.getByTestId('live-viewer-mp4-loop-input'),
    publishButton: page.getByTestId('live-viewer-publish-button'),
    stopPublishButton: page.getByTestId('live-viewer-stop-publish-button'),
    publishStatus: page.getByTestId('live-viewer-publish-status'),
    publishStreams: page.getByTestId('live-viewer-publish-streams'),
    publishStreamMonitor: page.getByTestId('live-viewer-publish-stream-monitor'),
    publishStreamStats: page.getByTestId('live-viewer-publish-stream-stats'),
    publishPreview: page.getByTestId('live-viewer-publish-preview'),
    publishLatency: page.getByTestId('live-viewer-publish-latency')
  }
}

export async function arrangeLiveViewerE2ESession(
  browser: Browser,
  trackNamespace = namespace
): Promise<LiveViewerE2ESession> {
  const context = await browser.newContext({ ignoreHTTPSErrors: true })
  const page = await context.newPage()
  await page.goto(buildPagePath(LIVE_VIEWER_PATH, trackNamespace), { waitUntil: 'domcontentloaded' })
  return { context, viewer: createLiveViewerPageModel(page) }
}

export async function expectVideoDecoded(video: Locator): Promise<void> {
  await expect
    .poll(async () => video.evaluate((element) => (element as HTMLVideoElement).readyState), {
      timeout: 30_000
    })
    .toBeGreaterThanOrEqual(2)
  await expect
    .poll(async () => video.evaluate((element) => (element as HTMLVideoElement).videoWidth))
    .toBeGreaterThan(0)
}

/// Cells of a delivery-overlay row that are already received, counted from the
/// row's first cell up to the first one still missing.
export async function receivedCellsAhead(viewer: LiveViewerPageModel, label: string): Promise<number> {
  return viewer.page.getByTestId('live-viewer-delivery-grid').evaluate((svg, rowLabel) => {
    const titles = Array.from(svg.querySelectorAll('rect.delivery-cell title'), (title) => title.textContent ?? '')
    const ofRow = titles.filter((title) => title.startsWith(`${rowLabel} `))
    const firstMissing = ofRow.findIndex((title) => !title.endsWith(': received'))
    return firstMissing === -1 ? ofRow.length : firstMissing
  }, label)
}

/// The stats line reads `A/V +12 ms` once both media have been presented and
/// `A/V --` until then.
export async function syncOffsetMs(viewer: LiveViewerPageModel): Promise<number> {
  const match = (await viewer.videoStats.innerText()).match(/A\/V ([+-]\d+) ms/)
  return match ? Math.abs(Number(match[1])) : Number.POSITIVE_INFINITY
}

export async function rewindableSeconds(viewer: LiveViewerPageModel): Promise<number> {
  return Number.parseFloat((await viewer.seekAvailableWindow.getAttribute('data-seconds')) ?? '0')
}
