import { type BrowserContext, expect, test, type Page } from '@playwright/test'
import { LIVE_TRANSLATE_STREAMER_PATH } from '../playwright.helpers'

type ViewerStats = {
  language: string
  forwardedAudioObjects: Record<string, number>
  droppedLateAudio: number
  playingAudioMicros: number | undefined
  syncOffsetMs: number | undefined
  translationOffsetMs: number | undefined
  viewerDelayMs: number | undefined
}

const relayUrl = process.env.MEDIA_E2E_MOQT_URL ?? 'https://127.0.0.1:4433'
const videoCodec = process.env.MEDIA_E2E_VIDEO_CODEC ?? 'vp8'
const PLAYBACK_DELAY_MS = 5_000
const BACK_TO_ORIGINAL_MAX_MS = 1_000

function viewerStats(viewer: Page): Promise<ViewerStats> {
  return viewer.evaluate(() => window.liveTranslateViewer!.stats())
}

async function startStreamer(context: BrowserContext): Promise<string> {
  const streamer = await context.newPage()
  await streamer.goto(
    `${LIVE_TRANSLATE_STREAMER_PATH}?relayUrl=${encodeURIComponent(relayUrl)}&videoCodec=${videoCodec}&videoEncoderHardwareAcceleration=prefer-software`
  )
  await streamer.getByTestId('streamer-start-button').click()
  const viewerLink = streamer.getByTestId('streamer-viewer-link')
  await expect(viewerLink).toHaveAttribute('href', /id=[a-z]+-[a-z]+/)
  return (await viewerLink.getAttribute('href'))!
}

async function watch(context: BrowserContext, viewerHref: string): Promise<Page> {
  const viewer = await context.newPage()
  await viewer.goto(viewerHref)
  await viewer.getByTestId('viewer-watch-button').click()
  await expect(viewer.getByTestId('viewer-language-option')).toHaveCount(6, { timeout: 30_000 })
  return viewer
}

function languageButton(viewer: Page, lang: string) {
  return viewer.locator(`[data-testid="viewer-language-option"][data-lang="${lang}"]`)
}

async function listenTo(viewer: Page, lang: string): Promise<void> {
  await languageButton(viewer, lang).click()
  await expect.poll(async () => (await viewerStats(viewer)).language, { timeout: 30_000 }).toBe(lang)
}

test('a viewer switches from the original audio to English, hears it with the picture and returns at once', async ({
  browser
}) => {
  // Arrange
  const context = await browser.newContext()
  const viewer = await watch(context, await startStreamer(context))
  await expect
    .poll(async () => (await viewerStats(viewer)).playingAudioMicros !== undefined, { timeout: 30_000 })
    .toBe(true)

  try {
    // Act
    await languageButton(viewer, 'en').click()

    // Assert
    await expect.poll(async () => (await viewerStats(viewer)).language, { timeout: 30_000 }).toBe('en')
    await expect
      .poll(async () => (await viewerStats(viewer)).forwardedAudioObjects['audio_en'] ?? 0, { timeout: 30_000 })
      .toBeGreaterThan(100)
    const before = (await viewerStats(viewer)).playingAudioMicros ?? 0
    await expect
      .poll(async () => ((await viewerStats(viewer)).playingAudioMicros ?? 0) - before, { timeout: 10_000 })
      .toBeGreaterThan(1_000_000)
    await expect(viewer.getByTestId('viewer-language-status')).toHaveText('聞いている言語: en')

    // Assert: the picture runs the fixed playback delay behind the streamer and the translation plays with it.
    // The first picture shows a few seconds after the sound: the video waits for a keyframe and is held
    // until just before its time, so the A/V offset is read once a frame has been shown.
    await expect
      .poll(async () => (await viewerStats(viewer)).viewerDelayMs ?? 0, { timeout: 30_000 })
      .toBeGreaterThan(PLAYBACK_DELAY_MS - 200)
    await expect
      .poll(async () => (await viewerStats(viewer)).syncOffsetMs !== undefined, { timeout: 30_000 })
      .toBe(true)
    const stats = await viewerStats(viewer)
    expect(stats.viewerDelayMs).toBeLessThan(PLAYBACK_DELAY_MS + 1_000)
    expect(Math.abs(stats.syncOffsetMs ?? Number.POSITIVE_INFINITY)).toBeLessThan(50)
    expect(Math.abs(stats.translationOffsetMs ?? Number.POSITIVE_INFINITY)).toBeLessThan(50)

    // Act
    const switchedBackAt = Date.now()
    await languageButton(viewer, 'ja').click()

    // Assert
    await expect.poll(async () => (await viewerStats(viewer)).language, { timeout: 5_000 }).toBe('ja')
    expect(Date.now() - switchedBackAt).toBeLessThan(BACK_TO_ORIGINAL_MAX_MS)
  } finally {
    await context.close()
  }
})

test('a viewer who joins after the translation started hears the original audio', async ({ browser }) => {
  // Arrange
  const context = await browser.newContext()
  const viewerHref = await startStreamer(context)
  await listenTo(await watch(context, viewerHref), 'en')

  try {
    // Act
    const lateViewer = await watch(context, viewerHref)

    // Assert
    await expect
      .poll(async () => (await viewerStats(lateViewer)).forwardedAudioObjects['camera_audio'] ?? 0, { timeout: 30_000 })
      .toBeGreaterThan(100)
    await expect
      .poll(async () => (await viewerStats(lateViewer)).playingAudioMicros !== undefined, { timeout: 10_000 })
      .toBe(true)
    await expect
      .poll(async () => (await viewerStats(lateViewer)).viewerDelayMs ?? 0, { timeout: 30_000 })
      .toBeGreaterThan(PLAYBACK_DELAY_MS - 200)
    expect((await viewerStats(lateViewer)).viewerDelayMs).toBeLessThan(PLAYBACK_DELAY_MS + 1_000)
  } finally {
    await context.close()
  }
})
