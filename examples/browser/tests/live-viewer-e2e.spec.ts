import { expect, test, type Locator } from '@playwright/test'
import {
  arrangeLiveViewerE2ESession,
  closeLiveViewerE2ESession,
  expectVideoDecoded,
  type LiveViewerPageModel,
  rewindableSeconds,
  syncOffsetMs
} from './live-viewer-e2e-arrange'

test('live viewer plays the ingested stream, switches renditions and rewinds', async ({ browser }) => {
  // Arrange
  const session = await arrangeLiveViewerE2ESession(browser)
  const { viewer } = session

  try {
    // Assert
    await expect(viewer.seekbar).toBeDisabled()
    await expect(viewer.seekElapsed).toBeHidden()

    // Act
    await viewer.watchButton.click()

    // Assert
    await expect(viewer.connectionStatus).toContainText('Connected:')
    await expect(viewer.catalogStatus).toContainText(/Catalog loaded: [1-9]/)
    await expect(viewer.playbackStatus).toContainText('Playing')
    await expect(viewer.liveButton).not.toHaveClass(/reviewing/)
    await expectVideoDecoded(viewer.video)

    // Assert: A/V offset stays within one frame
    await expect.poll(async () => syncOffsetMs(viewer), { timeout: 20_000 }).toBeLessThan(40)

    // Act
    await expect.poll(async () => rewindableSeconds(viewer), { timeout: 60_000 }).toBeGreaterThan(10)
    await expect(viewer.seekbar).toBeEnabled()
    await expect(viewer.seekPosition).toHaveText('LIVE')
    await expect(viewer.seekElapsed).toBeHidden()

    // Assert: the axis starts at the oldest replayable group, labelled by how far it is behind live
    await expect(viewer.seekStart).toHaveText(/^-\d+:\d{2}$/)

    await viewer.seekbar.focus()
    await viewer.seekbar.press('Home')

    // Assert: the rewound audio plays with the rewound picture
    await expect(viewer.rewindStatus).toContainText(/Rewound \d/)
    await expect(viewer.rewindStatus).toContainText(/A\/V [+-]\d+ ms/, { timeout: 20_000 })
    await expect(viewer.playbackStatus).toContainText('Reviewing')
    await expect(viewer.liveButton).toHaveClass(/reviewing/)
    await expect(viewer.reviewCanvas).toBeVisible()

    // Assert: the rewound picture has the width of the rendition the Auto option names; rewinding between an ABR
    // switch and the new rendition's first frame leaves the live video at the previous rendition's width
    await expect
      .poll(async () => viewer.reviewCanvas.evaluate((element) => (element as HTMLCanvasElement).width), {
        timeout: 30_000
      })
      .toBe(await autoRenditionWidth(viewer))

    await expect(viewer.seekPosition).toHaveText(/-\d+\.\ds/)

    const thumbBefore = await thumbValue(viewer)
    const playedBefore = await playedWidth(viewer)

    // Assert: playback continues beyond one fetch window (4 groups = 8 s)
    await expect.poll(async () => reviewedSeconds(viewer), { timeout: 30_000 }).toBeGreaterThan(9)
    await expect(viewer.logPanel).toContainText(/fetched \d+ objects/)

    // Assert: the thumb stays at the seek position while the fill shows playback progress
    expect(await thumbValue(viewer)).toBeCloseTo(thumbBefore, 1)
    expect(await playedWidth(viewer)).toBeGreaterThan(playedBefore)

    // Act
    await viewer.seekbar.press('End')

    // Assert
    await expect(viewer.rewindStatus).toContainText('Live')
    await expect(viewer.seekPosition).toHaveText('LIVE')
    await expect(viewer.liveButton).not.toHaveClass(/reviewing/)
    await expect(viewer.video).toBeVisible()
    await expect(viewer.playbackStatus).toContainText('Playing')

    // Act
    const slider = await viewer.seekbar.boundingBox()
    expect(slider).not.toBeNull()
    await viewer.page.mouse.move(slider!.x + slider!.width - 8, slider!.y + slider!.height / 2)
    await viewer.page.mouse.down()
    await viewer.page.mouse.move(slider!.x + slider!.width / 2, slider!.y + slider!.height / 2, { steps: 5 })

    // Assert
    await expect(viewer.seekPosition).toHaveText(/-\d+\.\ds/)
    await expect(viewer.rewindStatus).toHaveText('Live')

    // Act
    await viewer.page.mouse.up()

    // Assert
    await expect(viewer.rewindStatus).toContainText(/Rewound \d/)

    // Act
    await viewer.liveButton.click()
    await viewer.seekbar.hover()
    await viewer.playPauseButton.click()

    // Assert
    await expect(viewer.playPauseButton).toHaveAttribute('aria-pressed', 'true')
    await expect.poll(async () => mediaProp(viewer.video, 'paused')).toBe(true)

    // Act
    await viewer.playPauseButton.click()

    // Assert
    await expect.poll(async () => mediaProp(viewer.video, 'paused')).toBe(false)
    await expect.poll(async () => syncOffsetMs(viewer), { timeout: 20_000 }).toBeLessThan(40)

    // Act: focus seekbar so ArrowDown steps 5 s back instead of moving the volume slider
    await viewer.seekbar.focus()
    await viewer.page.keyboard.press('ArrowDown')

    // Assert
    await expect(viewer.playbackStatus).toContainText('Reviewing')
    await expect(viewer.seekPosition).toHaveText(/-[5-7]\.\ds/)
    const skippedTo = await thumbValue(viewer)
    expect(skippedTo).toBeLessThan(Number(await viewer.seekbar.getAttribute('max')))
    await expect.poll(async () => reviewedSeconds(viewer), { timeout: 15_000 }).toBeGreaterThan(0)

    // Act
    await viewer.page.keyboard.press('ArrowRight')

    // Assert
    await expect.poll(async () => thumbValue(viewer)).toBeGreaterThan(skippedTo)

    // Act: overlay buttons become clickable on hover
    const beforeBack = await thumbValue(viewer)
    await viewer.seekbar.hover()
    await viewer.skipBack5Button.click()

    // Assert
    await expect.poll(async () => thumbValue(viewer)).toBeLessThan(beforeBack)

    // Act: repeat ArrowUp because the live edge keeps advancing while paused
    for (let presses = 0; presses < 6 && (await viewer.seekPosition.innerText()) !== 'LIVE'; presses += 1) {
      await viewer.page.keyboard.press('ArrowUp')
    }

    // Assert
    await expect(viewer.seekPosition).toHaveText('LIVE')
    await expect(viewer.reviewCanvas).toBeHidden()

    // Act
    await viewer.qualityButton.click()
    await expect(viewer.qualityMenu).toBeVisible()
    await expect(viewer.videoTrackSelect).toHaveValue('@auto')
    await expect(viewer.playbackStatus).toContainText('Playing ')
    const playing = (await viewer.playbackStatus.innerText()).replace('Playing ', '')
    const rendition = await renditionOption(viewer, playing)
    await viewer.videoTrackSelect.selectOption(rendition)

    // Assert: the rendition is subscribed first and the relay ends the playing track at the group boundary (SUBSCRIPTION_ENDED)
    await expect(viewer.logPanel).toContainText(new RegExp(`subscribed \\S*/${rendition}`))
    await expect(viewer.logPanel).toContainText(`${playing} ended by the publisher (status SUBSCRIPTION_ENDED)`)
    // Assert: the renditions number their groups alike, so the splice keeps the seek timeline
    expect(await rewindableSeconds(viewer)).toBeGreaterThan(10)
    await expect(viewer.seekPosition).toHaveText('LIVE')
    await expect
      .poll(async () => viewer.video.evaluate((element) => (element as HTMLVideoElement).videoWidth), {
        timeout: 20_000
      })
      .toBeLessThan(1280)

    // Act
    await viewer.stopButton.click()

    // Assert
    await expect(viewer.seekAvailableWindow).toHaveAttribute('data-seconds', '0.0')
    await expect(viewer.seekbar).toBeDisabled()
    await expect(viewer.seekPosition).toHaveText('LIVE')
    await expect(viewer.seekElapsed).toBeHidden()
  } finally {
    await closeLiveViewerE2ESession(session)
  }
})

test('live viewer plays and reviews CMAF tracks through MSE', async ({ browser }) => {
  // Arrange
  const session = await arrangeLiveViewerE2ESession(browser)
  const { viewer } = session

  try {
    // Act
    await viewer.watchButton.click()
    await expect(viewer.playbackStatus).toContainText('Playing')
    await viewer.qualityButton.click()
    await viewer.packagingSelect.selectOption('cmaf')

    // Assert: a video is on screen after the switch (LOC, or MSE once it has presented a frame), then the visible one is blob-backed and plays
    await expect(viewer.logPanel).toContainText(/subscribed \S*\/video\S*_cmaf/)
    await expect(viewer.visibleVideo).toBeVisible()
    await expect.poll(async () => mediaProp(viewer.visibleVideo, 'src'), { timeout: 20_000 }).toMatch(/^blob:/)
    await expectVideoDecoded(viewer.visibleVideo)
    await expect.poll(async () => mediaProp(viewer.visibleVideo, 'currentTime'), { timeout: 15_000 }).toBeGreaterThan(1)

    // Act
    await viewer.volumeSlider.fill('0.3')

    // Assert
    await expect.poll(async () => mediaProp(viewer.visibleVideo, 'volume')).toBeCloseTo(0.3, 5)

    // Act: a quality change re-opens the MediaSource with the rendition's init segment
    await expect(viewer.playbackStatus).toContainText('Playing ')
    const playing = (await viewer.playbackStatus.innerText()).replace('Playing ', '').replace(/_cmaf$/, '')
    const rendition = await renditionOption(viewer, playing)
    const previousSource = await mediaProp(viewer.visibleVideo, 'src')
    await viewer.videoTrackSelect.selectOption(rendition)

    // Assert
    await expect(viewer.logPanel).toContainText(new RegExp(`subscribed \\S*/${rendition}_cmaf`))
    await expect.poll(async () => mediaProp(viewer.visibleVideo, 'src'), { timeout: 20_000 }).not.toBe(previousSource)
    await expect
      .poll(async () => viewer.visibleVideo.evaluate((element) => (element as HTMLVideoElement).videoWidth), {
        timeout: 20_000
      })
      .toBeLessThan(1280)
    const liveSource = await mediaProp(viewer.visibleVideo, 'src')
    const liveVideo = viewer.page.locator(`.viewer-stage video[src="${liveSource}"]`)

    // Act
    await expect.poll(async () => rewindableSeconds(viewer), { timeout: 60_000 }).toBeGreaterThan(10)
    await viewer.page.keyboard.press('Escape')
    await viewer.seekbar.focus()
    await viewer.seekbar.press('Home')

    // Assert: review plays from its own MediaSource while the live one stays hidden
    await expect(viewer.rewindStatus).toContainText(/Rewound \d/)
    await expect(viewer.playbackStatus).toContainText('Reviewing')
    await expect(viewer.reviewCanvas).toBeHidden()
    await expect.poll(async () => mediaProp(viewer.visibleVideo, 'src'), { timeout: 20_000 }).not.toBe(liveSource)
    await expect(liveVideo).toBeHidden()
    await expect.poll(async () => mediaProp(viewer.visibleVideo, 'currentTime'), { timeout: 30_000 }).toBeGreaterThan(3)
    await expect(viewer.logPanel).toContainText(/fetched \d+ objects from group/)

    // Act: seeking again keeps the current picture until the new MediaSource has presented
    const reviewSource = await mediaProp(viewer.visibleVideo, 'src')
    await viewer.page.keyboard.press('ArrowLeft')

    // Assert
    await expect(viewer.visibleVideo).toHaveCount(1)
    await expect.poll(async () => mediaProp(viewer.visibleVideo, 'src'), { timeout: 20_000 }).not.toBe(reviewSource)
    await expect(viewer.visibleVideo).toHaveCount(1)
    const liveTimeWhileReviewing = await mediaProp(liveVideo, 'currentTime')

    // Act / Assert
    await viewer.seekbar.hover()
    await viewer.playPauseButton.click()
    await expect.poll(async () => mediaProp(viewer.visibleVideo, 'paused')).toBe(true)
    await viewer.playPauseButton.click()
    await expect.poll(async () => mediaProp(viewer.visibleVideo, 'paused')).toBe(false)

    // Act
    await expect(viewer.speedSelect).toBeEnabled()
    await viewer.speedSelect.selectOption('2')

    // Assert
    await expect.poll(async () => mediaProp(viewer.visibleVideo, 'playbackRate')).toBe(2)

    // Act
    await viewer.liveButton.click()

    // Assert: the live element received nothing while reviewing and moves past where it stopped once delivery resumes
    await expect(viewer.rewindStatus).toContainText('Live')
    await expect(viewer.liveButton).not.toHaveClass(/reviewing/)
    await expect(viewer.speedSelect).toBeDisabled()
    await expect(liveVideo).toBeVisible()
    await expect(viewer.visibleVideo).toHaveCount(1)
    await expect
      .poll(async () => mediaProp(liveVideo, 'currentTime'), { timeout: 15_000 })
      .toBeGreaterThan(liveTimeWhileReviewing)
  } finally {
    await closeLiveViewerE2ESession(session)
  }
})

test('a second viewer joining a subscription the relay already holds gets the catalog by FETCH', async ({
  browser
}) => {
  // Arrange
  const first = await arrangeLiveViewerE2ESession(browser)
  const second = await arrangeLiveViewerE2ESession(browser)

  try {
    await first.viewer.watchButton.click()
    await expect(first.viewer.playbackStatus).toContainText('Playing')

    // Act
    await second.viewer.watchButton.click()

    // Assert: the catalog is resent every 30 s, so a catalog loaded within the timeout came through the joining FETCH
    await expect(second.viewer.catalogStatus).toContainText(/Catalog loaded: [1-9]/)
    await expect(second.viewer.playbackStatus).toContainText('Playing')
    await expectVideoDecoded(second.viewer.video)
  } finally {
    await closeLiveViewerE2ESession(second)
    await closeLiveViewerE2ESession(first)
  }
})

test('live viewer rewinds to groups from before it started watching, placed by the media timeline', async ({
  browser
}) => {
  test.setTimeout(180_000)
  // Arrange: the bridge has published for about the 60 s its media timeline lists
  const session = await arrangeLiveViewerE2ESession(browser)
  const { viewer } = session

  try {
    await viewer.watchButton.click()
    await expect.poll(async () => rewindableSeconds(viewer), { timeout: 90_000 }).toBeGreaterThan(55)
    await viewer.stopButton.click()
    await expect(viewer.seekbar).toBeDisabled()

    // Act
    await viewer.watchButton.click()
    await expect(viewer.playbackStatus).toContainText('Playing')
    const watchedAt = Date.now()

    // Assert: the timeline lists at once what live playback alone would take 40 s to observe
    await expect.poll(async () => rewindableSeconds(viewer), { timeout: 10_000 }).toBeGreaterThan(40)

    // Assert: the live axis is exactly the window FETCH can replay
    const replayable = await viewer.seekAvailableWindow.evaluate((element) => ({
      offset: Number.parseFloat((element as HTMLElement).style.left),
      width: Number.parseFloat((element as HTMLElement).style.width)
    }))
    expect(replayable.offset).toBeCloseTo(0, 1)
    expect(replayable.width).toBeCloseTo(100, 1)

    // Act: round to an integer so the range input does not rewrite the value
    await viewer.seekbar.fill(String(Math.round(Number(await viewer.seekbar.getAttribute('max')) - 40)))

    // Assert
    await expect(viewer.rewindStatus).toContainText(/Rewound \d/)
    expect(rewoundSeconds(await viewer.rewindStatus.innerText())).toBeGreaterThan((Date.now() - watchedAt) / 1_000 + 20)
    await expect(viewer.logPanel).toContainText(/fetched \d+ objects from group/)
    await expect.poll(async () => reviewedSeconds(viewer), { timeout: 20_000 }).toBeGreaterThan(3)

    // Act
    await viewer.seekbar.focus()
    await viewer.seekbar.press('Home')

    // Assert
    await expect
      .poll(async () => rewoundSeconds(await viewer.rewindStatus.innerText()), { timeout: 20_000 })
      .toBeGreaterThan(45)
    await expect.poll(async () => reviewedSeconds(viewer), { timeout: 20_000 }).toBeGreaterThan(3)
  } finally {
    await closeLiveViewerE2ESession(session)
  }
})

async function mediaProp<K extends 'src' | 'currentTime' | 'playbackRate' | 'paused' | 'volume'>(
  media: Locator,
  key: K
): Promise<HTMLMediaElement[K]> {
  return media.evaluate((element, property) => (element as HTMLMediaElement)[property], key)
}

function rewoundSeconds(rewindStatus: string): number {
  const match = rewindStatus.match(/Rewound (\d+\.\d)s/)
  return match ? Number(match[1]) : 0
}

async function thumbValue(viewer: LiveViewerPageModel): Promise<number> {
  return viewer.seekbar.evaluate((element) => (element as HTMLInputElement).valueAsNumber)
}

async function playedWidth(viewer: LiveViewerPageModel): Promise<number> {
  return viewer.seekReviewProgress.evaluate((element) => Number.parseFloat((element as HTMLElement).style.width))
}

/// Seconds played since the last seek: the review progress drawn on the axis.
async function reviewedSeconds(viewer: LiveViewerPageModel): Promise<number> {
  const axisSeconds =
    Number(await viewer.seekbar.getAttribute('max')) - Number(await viewer.seekbar.getAttribute('min'))
  return ((await playedWidth(viewer)) / 100) * axisSeconds
}

async function autoRenditionWidth(viewer: LiveViewerPageModel): Promise<number> {
  const options = await viewer.videoTrackSelect.locator('option').allTextContents()
  const label = options[0].match(/^Auto \((.+)\)$/)?.[1]
  const width = options.find((option) => option.startsWith(`${label} (`))?.match(/\((\d+)x\d+\)$/)?.[1]
  expect(width).toBeDefined()
  return Number(width)
}

/// The first transcoded rendition of the quality menu other than `except`.
async function renditionOption(viewer: LiveViewerPageModel, except: string): Promise<string> {
  const values = await viewer.videoTrackSelect
    .locator('option')
    .evaluateAll((options) => options.map((option) => (option as HTMLOptionElement).value))
  const rendition = values.find((value) => /^video_\d+p$/.test(value) && value !== except)
  expect(rendition).toBeDefined()
  return rendition ?? ''
}
