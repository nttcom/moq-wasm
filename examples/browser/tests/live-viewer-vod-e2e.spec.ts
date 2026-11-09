import { expect } from '@playwright/test'
import { setTimeout as sleep } from 'node:timers/promises'
import { test } from './recorder-e2e-arrange'
import {
  arrangeLiveViewerE2ESession,
  closeLiveViewerE2ESession,
  elapsedAtPosition,
  parseClock
} from './live-viewer-e2e-arrange'

/// Long enough to span several review windows of 4 groups.
const RECORDED_SECONDS = 20

test.setTimeout(180_000)

test('live viewer plays a recorder replay as VOD, through FETCH alone, to its end', async ({
  recorderServe,
  browser
}) => {
  // Arrange: the live-viewer runner publishes anon/live/e2e
  const recording = await recorderServe.start('anon/live', 'combined', false, 'moq')
  await sleep(RECORDED_SECONDS * 1_000)
  await recorderServe.stop(recording)
  const replay = await recorderServe.publish(recording)
  const namespace = `anon/live/recordings/${recording.id}/e2e/camera/0`
  const session = await arrangeLiveViewerE2ESession(browser, namespace)
  const { viewer } = session

  try {
    // Act
    await viewer.watchButton.click()

    // Assert: playback starts, with its sound, and there is no live edge to return to
    await expect(viewer.playbackStatus).toContainText('Playing', { timeout: 30_000 })
    await expect(viewer.liveButton).toBeHidden()
    await expect(viewer.rewindStatus).toContainText(/^VOD · A\/V [+-]\d+ ms/, { timeout: 20_000 })
    await expect
      .poll(async () => viewer.reviewCanvas.evaluate((element) => (element as HTMLCanvasElement).width))
      .toBeGreaterThan(0)
    await expect(viewer.seekElapsed).toHaveText(/^\d+:\d{2} \/ \d+:\d{2}$/)
    const [startedAt, end] = (await viewer.seekElapsed.innerText()).split(' / ').map(parseClock)
    // Assert: the axis spans the recording, which starts once the recorder has joined
    expect(end - parseClock(await viewer.seekStart.innerText())).toBeGreaterThanOrEqual(RECORDED_SECONDS - 7)
    await expect
      .poll(async () => elapsedAtPosition(await viewer.seekElapsed.innerText()), { timeout: 20_000 })
      .toBeGreaterThan(startedAt + 9)

    // Assert: it stops at the end, paused
    await expect(viewer.playbackStatus).toHaveText('Ended', { timeout: (RECORDED_SECONDS + 30) * 1_000 })
    await expect(viewer.playPauseButton).toHaveAttribute('aria-pressed', 'true')
    expect(elapsedAtPosition(await viewer.seekElapsed.innerText())).toBeGreaterThanOrEqual(end - 3)

    // Assert: the catalog and the media timeline are the only subscriptions
    const subscribed = (await viewer.logPanel.locator('.log-entry').allInnerTexts())
      .map((entry) => entry.match(/\ssubscribed (\S+)$/)?.[1])
      .filter((track) => track !== undefined)
    expect(subscribed.sort()).toEqual([`${namespace}/catalog`, `${namespace}/video_timeline`])

    // Act
    await viewer.playPauseButton.click()

    // Assert: playing again starts over
    await expect(viewer.playbackStatus).toContainText('Playing')
    await expect(viewer.playPauseButton).toHaveAttribute('aria-pressed', 'false')
    expect(elapsedAtPosition(await viewer.seekElapsed.innerText())).toBeLessThan(startedAt + 5)
  } finally {
    await closeLiveViewerE2ESession(session)
    await replay.close()
  }
})
