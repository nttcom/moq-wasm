import { expect, test, type Locator } from '@playwright/test'
import {
  MP4_FIXTURE,
  arrangeLiveViewerE2ESession,
  closeLiveViewerE2ESession,
  ensureMp4Fixture,
  expectVideoDecoded,
  liveViewerE2EConfig,
  mp4FixtureFileName,
  receivedCellsAhead,
  rewindableSeconds,
  syncOffsetMs
} from './live-viewer-e2e-arrange'
import { type MessagePageModel, openMessagePage } from './message-e2e-arrange'

const resolution = `${MP4_FIXTURE.width}x${MP4_FIXTURE.height}`
/// Well under the 10 s the relay waits for a FETCH response the browser publisher never sends.
const FIRST_VIDEO_TIMEOUT_MS = 5_000
/// Two seconds of samples cover two 1 s audio groups of the fixture; an AAC frame
/// is 21 ms, so the 200 ms playout buffer is about 10 cells and a whole group 47.
const AUDIO_AHEAD_SAMPLES = 20
const AUDIO_AHEAD_SAMPLE_INTERVAL_MS = 100
const AUDIO_CELLS_AHEAD_LIMIT = 30
const LARGEST_VARINT = '4611686018427387903'
/// The MP3 test closes its page without stopping the publish, so the relay keeps that publisher's
/// upstream subscriptions on the shared namespace until the QUIC idle timeout.
const FETCH_NAMESPACE = `${liveViewerE2EConfig.mp4Namespace}-fetch`

test('live viewer publishes an uploaded MP4 from the browser and plays it back', async ({ browser }) => {
  // Arrange
  const fixturePath = ensureMp4Fixture('aac')
  const session = await arrangeLiveViewerE2ESession(browser, liveViewerE2EConfig.mp4Namespace)
  const { viewer } = session

  try {
    // Act
    await viewer.mp4FileInput.setInputFiles(fixturePath)
    await viewer.publishButton.click()

    // Assert
    await expect(viewer.publishStatus).toHaveText(
      publishingStatus(mp4FixtureFileName('aac'), liveViewerE2EConfig.mp4Namespace, 'mp4a.40.2')
    )

    // Act
    await viewer.watchButton.click()

    // Assert
    await expect(viewer.connectionStatus).toContainText('Connected:')
    await expect(viewer.catalogStatus).toHaveText('Catalog loaded: 1 video / 1 audio')
    // Assert: the relay has cached the catalog since the publisher's PUBLISH, so the FETCH never reaches the browser
    await expect(viewer.logPanel).toContainText('fetched catalog')
    await expect(viewer.playbackStatus).toContainText('Playing video', { timeout: FIRST_VIDEO_TIMEOUT_MS })
    await expectVideoDecoded(viewer.video)
    await expect(viewer.videoStats).toContainText(resolution)

    // Assert: the streams the publisher sends to the relay are drawn per track
    await expect(viewer.publishStreams).toBeVisible()
    await expect(viewer.publishStreamStats).toHaveText(/^\d+ open · \d+ finished · \d+ kbps$/)
    await expect(viewer.publishStreamMonitor.locator('text.stream-label', { hasText: /^video / }).first()).toBeVisible()
    await expect(viewer.publishStreamMonitor.locator('text.stream-label', { hasText: /^audio / }).first()).toBeVisible()
    await expect(
      viewer.publishStreamMonitor.locator('text.stream-label', { hasText: /^catalog / }).first()
    ).toBeVisible()

    // Assert: the page shows the picture it sends and how far the viewer runs behind it
    await expect(viewer.publishPreview).toBeVisible()
    await expect
      .poll(async () => viewer.publishPreview.evaluate((element) => (element as HTMLCanvasElement).width))
      .toBe(MP4_FIXTURE.width)
    await expect(viewer.publishLatency).toHaveText(/^viewer delay \d+ ms$/)
    await expect(viewer.videoStats).toContainText(/delay \d+ ms/)

    // Assert: the audio of the file plays on the same clock as its picture
    await expect.poll(async () => syncOffsetMs(viewer), { timeout: 20_000 }).toBeLessThan(40)

    // Assert: the audio row of the delivery overlay starts at the audio being heard, so only the
    // playout buffer is ahead of it rather than the whole group played so far
    const audioCellsAhead: number[] = []
    for (let sample = 0; sample < AUDIO_AHEAD_SAMPLES; sample++) {
      audioCellsAhead.push(await receivedCellsAhead(viewer, 'audio'))
      await viewer.page.waitForTimeout(AUDIO_AHEAD_SAMPLE_INTERVAL_MS)
    }
    expect(Math.max(...audioCellsAhead)).toBeLessThan(AUDIO_CELLS_AHEAD_LIMIT)

    // Act: rewind into the groups the relay has cached from the browser publisher
    await expect.poll(async () => rewindableSeconds(viewer), { timeout: 30_000 }).toBeGreaterThan(3)
    await expect(viewer.seekElapsed).toHaveText(/^\d+:\d{2} \/ \d+:\d{2}$/)
    await viewer.seekbar.focus()
    await viewer.seekbar.press('Home')

    // Assert
    await expect(viewer.rewindStatus).toContainText(/Rewound \d/)
    await expect(viewer.playbackStatus).toContainText('Reviewing')
    await expect(viewer.reviewCanvas).toBeVisible()
    await expect(viewer.logPanel).toContainText(/fetched \d+ objects/)

    // Act
    await viewer.liveButton.click()

    // Assert
    await expect(viewer.liveButton).not.toHaveClass(/reviewing/)
    await expect(viewer.playbackStatus).toContainText('Playing video')

    // Act
    await viewer.stopPublishButton.click()

    // Assert
    await expect(viewer.publishStatus).toHaveText('Publish stopped')
    await expect(viewer.publishStreams).toBeHidden()
    await expect(viewer.publishPreview).toBeHidden()
  } finally {
    await closeLiveViewerE2ESession(session)
  }
})

test('live viewer publishes the MP3 audio of an uploaded MP4 as it is', async ({ browser }) => {
  // Arrange
  const fixturePath = ensureMp4Fixture('mp3')
  const session = await arrangeLiveViewerE2ESession(browser, liveViewerE2EConfig.mp4Namespace)
  const { viewer } = session

  try {
    // Act
    await viewer.mp4FileInput.setInputFiles(fixturePath)
    await viewer.publishButton.click()

    // Assert
    await expect(viewer.publishStatus).toHaveText(
      publishingStatus(mp4FixtureFileName('mp3'), liveViewerE2EConfig.mp4Namespace, 'mp3')
    )

    // Act
    await viewer.watchButton.click()

    // Assert
    await expect(viewer.catalogStatus).toHaveText('Catalog loaded: 1 video / 1 audio')
    await expectVideoDecoded(viewer.video)
    await expect.poll(async () => syncOffsetMs(viewer), { timeout: 20_000 }).toBeLessThan(40)
  } finally {
    await closeLiveViewerE2ESession(session)
  }
})

test('a FETCH reaching before the relay cache is answered by the browser MP4 publisher', async ({ browser }) => {
  // Arrange
  const fixturePath = ensureMp4Fixture('aac')
  const session = await arrangeLiveViewerE2ESession(browser, FETCH_NAMESPACE)
  const { viewer } = session
  let fetcher: MessagePageModel | undefined

  try {
    await viewer.mp4FileInput.setInputFiles(fixturePath)
    await viewer.publishButton.click()
    await expect(viewer.publishStatus).toContainText('Publishing')
    await viewer.watchButton.click()
    await expect(viewer.playbackStatus).toContainText('Playing video')
    fetcher = await openMessagePage(session.context, liveViewerE2EConfig.moqtUrl)
    await fetcher.connectButton.click()
    await expect(fetcher.logPanel).toContainText('[moqt][wt] connected')
    await fetcher.setupButton.click()
    await expect(fetcher.sendStatus).toHaveText('Sent CLIENT_SETUP')
    await fetcher.fetchRequestIdInput.fill('0')
    await fetcher.fetchNamespaceInput.fill(FETCH_NAMESPACE)
    await fetcher.fetchTrackNameInput.fill('video')
    await fetcher.fetchStartGroupInput.fill('0')
    await fetcher.fetchStartObjectInput.fill('0')
    await fetcher.fetchEndGroupInput.fill(LARGEST_VARINT)
    await fetcher.fetchEndObjectInput.fill('0')

    // Act
    await fetcher.fetchButton.click()

    // Assert: the relay has cached the track only since the viewer subscribed, so it forwards the FETCH upstream
    await expect(fetcher.sendStatus).toHaveText('Sent FETCH')
    await expect(viewer.logPanel).toContainText(/answered FETCH video 0:0-\d+:\d+/)
    // Assert: the replayed groups match what the relay cached from the subscription
    await expect(viewer.logPanel).not.toContainText(/malformed/i)
  } finally {
    await fetcher?.closeButton.click()
    await closeLiveViewerE2ESession(session)
  }
})

function publishingStatus(fileName: string, namespace: string, audioCodec: string): RegExp {
  const escape = (text: string) => text.replace(/[.*+?^${}()|[\]\\/]/g, '\\$&')
  return new RegExp(
    `^Publishing ${escape(fileName)} \\(${resolution} avc1\\.[0-9A-F]{6}, ${escape(audioCodec)}\\) to ${escape(namespace)}$`
  )
}

test('a paused viewer steps through still frames and resumes from the last one', async ({ browser }) => {
  // Arrange
  const fixturePath = ensureMp4Fixture('aac')
  const namespace = `${liveViewerE2EConfig.mp4Namespace}-paused`
  const session = await arrangeLiveViewerE2ESession(browser, namespace)
  const { viewer } = session
  const skipBack1Button = viewer.page.getByTestId('live-player-skip-back-1-button')

  try {
    await viewer.mp4FileInput.setInputFiles(fixturePath)
    await viewer.publishButton.click()
    await expect(viewer.publishStatus).toHaveText(publishingStatus(mp4FixtureFileName('aac'), namespace, 'mp4a.40.2'))
    await viewer.watchButton.click()
    await expect(viewer.connectionStatus).toContainText('Connected:')
    await expect(viewer.playbackStatus).toContainText('Playing video', { timeout: FIRST_VIDEO_TIMEOUT_MS })
    await expect.poll(async () => rewindableSeconds(viewer), { timeout: 30_000 }).toBeGreaterThan(3)
    await viewer.playPauseButton.click()
    await expect(viewer.playPauseButton).toHaveAttribute('aria-pressed', 'true')

    // Act
    await skipBack1Button.click()

    // Assert: the review opens on the frame at the position and holds it while paused
    await expect(viewer.rewindStatus).toContainText(/Rewound \d/)
    await expect(viewer.playPauseButton).toHaveAttribute('aria-pressed', 'true')
    await expect.poll(async () => reviewCanvasWidth(viewer.reviewCanvas)).toBeGreaterThan(0)
    const firstStill = await canvasDigest(viewer.reviewCanvas)
    await viewer.page.waitForTimeout(STILL_FRAME_HOLD_MS)
    expect(await canvasDigest(viewer.reviewCanvas)).toBe(firstStill)

    // Act
    await skipBack1Button.click()

    // Assert: the next step shows another frame and holds it as well
    await expect.poll(async () => canvasDigest(viewer.reviewCanvas), { timeout: 10_000 }).not.toBe(firstStill)
    const secondStill = await canvasDigest(viewer.reviewCanvas)
    await viewer.page.waitForTimeout(STILL_FRAME_HOLD_MS)
    expect(await canvasDigest(viewer.reviewCanvas)).toBe(secondStill)

    // Act
    await viewer.playPauseButton.click()

    // Assert: resuming plays on from the still frame with the audio of that position
    await expect(viewer.playPauseButton).toHaveAttribute('aria-pressed', 'false')
    await expect.poll(async () => canvasDigest(viewer.reviewCanvas), { timeout: 10_000 }).not.toBe(secondStill)
    await expect(viewer.rewindStatus).toContainText(/A\/V [+-]\d+ ms/, { timeout: 20_000 })
  } finally {
    await closeLiveViewerE2ESession(session)
  }
})

const STILL_FRAME_HOLD_MS = 1_500

function reviewCanvasWidth(canvas: Locator): Promise<number> {
  return canvas.evaluate((element) => (element as HTMLCanvasElement).width)
}

/// A checksum over a sample of the canvas pixels, enough to tell two frames apart.
function canvasDigest(canvas: Locator): Promise<string> {
  return canvas.evaluate((element) => {
    const pixels = element as HTMLCanvasElement
    const { data } = pixels.getContext('2d')!.getImageData(0, 0, pixels.width, pixels.height)
    let sum = 0
    for (let index = 0; index < data.length; index += 97) {
      sum = (sum + data[index] * (index % 251)) % 1_000_000_007
    }
    return `${pixels.width}x${pixels.height}:${sum}`
  })
}
