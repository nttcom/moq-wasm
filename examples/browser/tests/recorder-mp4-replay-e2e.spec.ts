import { expect } from '@playwright/test'
import { setTimeout as sleep } from 'node:timers/promises'
import { createProgressiveRecording, recordingPlayerPath, test } from './recorder-e2e-arrange'

test.setTimeout(120_000)

test('replays recorded fMP4 and progressive MP4 with audio and seeking', async ({ recorderServe, page }) => {
  // Arrange
  const recording = await recorderServe.start('anon/live', 'per-track', false, 'mp4')
  await sleep(20_000)
  await recorderServe.stop(recording)
  const progressiveId = await createProgressiveRecording(recorderServe.storeDir, recording)
  await page.addInitScript(() => {
    const start = AudioBufferSourceNode.prototype.start
    AudioBufferSourceNode.prototype.start = function (...args) {
      if (this.buffer?.getChannelData(0).some((sample) => Math.abs(sample) > 0.001))
        document.documentElement.dataset.audioPlayed = 'true'
      return start.apply(this, args)
    }
  })

  for (const id of [recording.id, progressiveId]) {
    // Act
    await page.goto(recordingPlayerPath(recorderServe.url, id))
    await page.locator('#load').click()
    await expect(page.locator('#status')).toHaveText('Ready', { timeout: 30_000 })
    await page.locator('#play').click()

    // Assert
    const canvas = page.locator('canvas').first()
    await expect
      .poll(async () => Number(await canvas.getAttribute('data-capture-us')), { timeout: 15_000 })
      .toBeGreaterThan(0)
    await expect(page.locator('html')).toHaveAttribute('data-audio-played', 'true', { timeout: 15_000 })
    const beforeSeek = Number(await canvas.getAttribute('data-capture-us'))
    const seconds = await page.locator('#seek').evaluate((element: HTMLInputElement) => Number(element.max) * 0.6)
    await page.locator('#seek').evaluate((element: HTMLInputElement, seconds) => {
      element.value = String(seconds)
      element.dispatchEvent(new Event('change', { bubbles: true }))
    }, seconds)
    await expect
      .poll(async () => Number(await canvas.getAttribute('data-capture-us')), { timeout: 15_000 })
      .toBeGreaterThan(beforeSeek + seconds * 1_000_000 - 500_000)
  }
})
