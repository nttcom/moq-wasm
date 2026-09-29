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

const FFMPEG_AUDIO_ENCODERS: Record<Mp4FixtureAudio, string> = { aac: 'aac', mp3: 'libmp3lame' }

export function mp4FixtureFileName(audio: Mp4FixtureAudio): string {
  return `moqt-live-viewer-mp4-e2e-${audio}.mp4`
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
    'baseline',
    '-pix_fmt',
    'yuv420p',
    '-g',
    String(MP4_FIXTURE.fps),
    '-c:a',
    FFMPEG_AUDIO_ENCODERS[audio],
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
  rewindBuffer: Locator
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
    videoTrackSelect: page.getByTestId('live-viewer-video-track-select'),
    skipBack5Button: page.getByTestId('live-viewer-skip-back-5-button'),
    skipForward1Button: page.getByTestId('live-viewer-skip-forward-1-button'),
    playPauseButton: page.getByTestId('live-viewer-play-pause-button'),
    volumeSlider: page.getByTestId('live-viewer-volume'),
    videoStats: page.getByTestId('live-viewer-video-stats'),
    liveButton: page.getByTestId('live-viewer-live-button'),
    rewindStatus: page.getByTestId('live-viewer-rewind-status'),
    rewindBuffer: page.getByTestId('live-viewer-rewind-buffer'),
    seekbar: page.getByTestId('live-viewer-seekbar'),
    seekPosition: page.getByTestId('live-viewer-seek-position'),
    seekElapsed: page.getByTestId('live-viewer-seek-elapsed'),
    seekStart: page.getByTestId('live-viewer-seek-start'),
    seekReviewProgress: page.getByTestId('live-viewer-seek-review-progress'),
    qualityButton: page.getByTestId('live-viewer-quality-button'),
    qualityMenu: page.getByTestId('live-viewer-quality-menu'),
    packagingSelect: page.getByTestId('live-viewer-packaging-select'),
    speedSelect: page.getByTestId('live-viewer-speed-select'),
    seekAvailableWindow: page.getByTestId('live-viewer-seek-available-window'),
    video: page.getByTestId('live-viewer-video'),
    reviewCanvas: page.getByTestId('live-viewer-review-canvas'),
    visibleVideo: page.locator('.viewer-stage video:visible'),
    logPanel: page.getByTestId('live-viewer-log-panel'),
    mp4FileInput: page.getByTestId('live-viewer-mp4-file-input'),
    mp4LoopInput: page.getByTestId('live-viewer-mp4-loop-input'),
    publishButton: page.getByTestId('live-viewer-publish-button'),
    stopPublishButton: page.getByTestId('live-viewer-stop-publish-button'),
    publishStatus: page.getByTestId('live-viewer-publish-status'),
    publishStreams: page.getByTestId('live-viewer-publish-streams'),
    publishStreamMonitor: page.getByTestId('live-viewer-publish-stream-monitor'),
    publishStreamStats: page.getByTestId('live-viewer-publish-stream-stats')
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

/// The stats line reads `A/V +12 ms` once both media have been presented and
/// `A/V --` until then.
export async function syncOffsetMs(viewer: LiveViewerPageModel): Promise<number> {
  const match = (await viewer.videoStats.innerText()).match(/A\/V ([+-]\d+) ms/)
  return match ? Math.abs(Number(match[1])) : Number.POSITIVE_INFINITY
}

export async function parseSeconds(locator: Locator): Promise<number> {
  return Number.parseFloat((await locator.innerText()).replace('s', ''))
}
