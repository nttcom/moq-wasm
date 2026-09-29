import { expect, test } from '@playwright/test'
import {
  MP4_FIXTURE,
  arrangeLiveViewerE2ESession,
  ensureMp4Fixture,
  expectVideoDecoded,
  liveViewerE2EConfig,
  mp4FixtureFileName,
  parseSeconds,
  syncOffsetMs
} from './live-viewer-e2e-arrange'

const resolution = `${MP4_FIXTURE.width}x${MP4_FIXTURE.height}`

test('live viewer publishes an uploaded MP4 from the browser and plays it back', async ({ browser }) => {
  // Arrange
  const fixturePath = ensureMp4Fixture('aac')
  const { context, viewer } = await arrangeLiveViewerE2ESession(browser, liveViewerE2EConfig.mp4Namespace)

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
    await expect(viewer.playbackStatus).toContainText('Playing video')
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

    // Act: rewind into the groups the relay has cached from the browser publisher
    await expect.poll(async () => parseSeconds(viewer.rewindBuffer), { timeout: 30_000 }).toBeGreaterThan(3)
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
    await context.close()
  }
})

test('live viewer publishes the MP3 audio of an uploaded MP4 as it is', async ({ browser }) => {
  // Arrange
  const fixturePath = ensureMp4Fixture('mp3')
  const { context, viewer } = await arrangeLiveViewerE2ESession(browser, liveViewerE2EConfig.mp4Namespace)

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
    await context.close()
  }
})

function publishingStatus(fileName: string, namespace: string, audioCodec: string): RegExp {
  const escape = (text: string) => text.replace(/[.*+?^${}()|[\]\\/]/g, '\\$&')
  return new RegExp(
    `^Publishing ${escape(fileName)} \\(${resolution} avc1\\.[0-9A-F]{6}, ${escape(audioCodec)}\\) to ${escape(namespace)}$`
  )
}
