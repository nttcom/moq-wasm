import { expect } from '@playwright/test'
import { readFile } from 'node:fs/promises'
import { join } from 'node:path'
import { setTimeout as sleep } from 'node:timers/promises'
import { test } from './recorder-e2e-arrange'
import { createLargeRecording } from './large-recording'
import { arrangeMeetingClient, enableMedia, joinRoom, sendChatMessage, startScreenShare } from './meeting-e2e-arrange'

test.setTimeout(120_000)

test('records a room and plays and seeks through the relay', async ({ recorderServe, page }) => {
  // Arrange: the live-viewer runner publishes a broadcast under anon/live
  const recording = await recorderServe.start('anon/live', 'combined', false, 'moq')
  expect(recording.format).toBe('moq')
  await sleep(37_000)

  // Act
  await recorderServe.stop(recording)
  const room = JSON.parse(await readFile(join(recorderServe.storeDir, recording.id, 'manifest.json'), 'utf8'))
  const member = room.members.find((entry: { namespace: string }) => entry.namespace === 'anon/live/e2e')

  // Assert
  expect(room.format).toBe('moq')
  expect(member.camera.length).toBeGreaterThan(0)
  const archive = JSON.parse(await readFile(join('/', member.camera[0]), 'utf8'))
  const names = archive.tracks.map((track: { name: string }) => track.name)
  expect(names).toContain('catalog')
  expect(names).toContain('video')
  expect(names).toContain('audio')

  await page.addInitScript(() => {
    const start = AudioBufferSourceNode.prototype.start
    AudioBufferSourceNode.prototype.start = function (...args) {
      if (this.buffer?.getChannelData(0).some((sample) => Math.abs(sample) > 0.001))
        document.documentElement.dataset.audioPlayed = 'true'
      return start.apply(this, args)
    }
  })
  await page.goto(
    `/moq-wasm/examples/recording-player/?recorder=${encodeURIComponent(recorderServe.url)}&id=${recording.id}`
  )
  await page.locator('#load').click()
  await expect(page.locator('#status')).toHaveText('Ready', { timeout: 30_000 })
  await page.locator('#play').click()
  const canvas = page.locator('canvas').first()
  await expect
    .poll(async () => Number(await canvas.getAttribute('data-capture-us')), { timeout: 20_000 })
    .toBeGreaterThan(0)
  await expect(page.locator('html')).toHaveAttribute('data-audio-played', 'true', { timeout: 15_000 })
  const beforeSeek = Number(await canvas.getAttribute('data-capture-us'))
  await page.locator('#seek').evaluate((element: HTMLInputElement) => {
    element.value = '32'
    element.dispatchEvent(new Event('change', { bubbles: true }))
  })
  await expect
    .poll(async () => Number(await canvas.getAttribute('data-capture-us')), { timeout: 15_000 })
    .toBeGreaterThan(beforeSeek + 30_000_000)

  // Assert: a logical recording larger than 256 MiB opens and seeks without downloading it all.
  const large = await createLargeRecording(recorderServe.storeDir, recording.id)
  expect(large.bytes).toBeGreaterThan(256 * 1024 * 1024)
  await page.goto(
    `/moq-wasm/examples/recording-player/?recorder=${encodeURIComponent(recorderServe.url)}&id=${large.id}`
  )
  await page.evaluate(async () => {
    const moduleUrl = '/moq-wasm/tests/recording-publisher.ts'
    const { measureFetchBytes } = await import(/* @vite-ignore */ moduleUrl)
    measureFetchBytes()
  })
  await page.locator('#load').click()
  await expect(page.locator('#status')).toHaveText('Ready', { timeout: 30_000 })
  expect(await page.locator('#seek').getAttribute('max')).not.toBe('0')
  await page.locator('#seek').evaluate((element: HTMLInputElement, seconds) => {
    element.value = String(seconds)
    element.dispatchEvent(new Event('change', { bubbles: true }))
  }, large.periodSeconds - 1)
  await expect
    .poll(async () => Number(await page.locator('canvas').first().getAttribute('data-capture-us')), {
      timeout: 20_000
    })
    .toBeGreaterThan(beforeSeek + large.periodSeconds * 1_000_000 + 500_000)
  await expect(page.locator('html')).toHaveAttribute('data-audio-played', 'true', { timeout: 15_000 })
  await page.locator('#seek').evaluate((element: HTMLInputElement) => {
    element.value = String(Number(element.max) - 3)
    element.dispatchEvent(new Event('change', { bubbles: true }))
  })
  await expect
    .poll(async () => Number(await page.locator('canvas').first().getAttribute('data-capture-us')), { timeout: 20_000 })
    .toBeGreaterThan(beforeSeek + 60_000_000)
  expect(await page.locator('html').evaluate((element) => Number(element.dataset.fetchBytes))).toBeLessThan(
    32 * 1024 * 1024
  )
})

test('records and replays participants, screen share, chat and transcript', async ({
  recorderServe,
  browser,
  page
}) => {
  const roomName = `archive-${Date.now()}`
  const recording = await recorderServe.start(`anon/${roomName}`, 'combined', false, 'moq')
  const alice = await arrangeMeetingClient(browser, { params: { recorderUrl: recorderServe.url } })
  const bob = await arrangeMeetingClient(browser, { params: { recorderUrl: recorderServe.url } })
  try {
    await joinRoom(alice.page, roomName, 'alice', 'a')
    await enableMedia(alice.page)
    await joinRoom(bob.page, roomName, 'bob', 'a')
    await enableMedia(bob.page)
    await startScreenShare(alice.page)
    await sleep(5_000)
    await sendChatMessage(alice.page, 'archive chat message')
    await alice.page.page.evaluate(
      async ({ roomName }) => {
        const moduleUrl = '/moq-wasm/tests/recording-publisher.ts'
        const publisher = await import(/* @vite-ignore */ moduleUrl)
        await publisher.publishTranscript('https://127.0.0.1:4433', ['anon', roomName, 'bots', 'transcriber'])
      },
      { roomName }
    )
    await sleep(3_000)
    await recorderServe.stop(recording)

    const room = JSON.parse(await readFile(join(recorderServe.storeDir, recording.id, 'manifest.json'), 'utf8'))
    const aliceMember = room.members.find(
      (member: { namespace: string }) => member.namespace === `anon/${roomName}/alice`
    )
    const bobMember = room.members.find((member: { namespace: string }) => member.namespace === `anon/${roomName}/bob`)
    expect(aliceMember?.camera.length).toBeGreaterThan(0)
    expect(aliceMember?.screen_share.length).toBeGreaterThan(0)
    expect(bobMember?.camera.length).toBeGreaterThan(0)
    const archive = JSON.parse(await readFile(join('/', aliceMember.camera[0]), 'utf8'))
    const chat = archive.tracks.find((track: { name: string }) => track.name === 'chat')
    expect(chat).toBeTruthy()
    const payloads = []
    for (const chunk of chat.chunks) {
      const data = await readFile(join('/', chunk.data))
      const entries = JSON.parse(await readFile(join('/', chunk.index), 'utf8'))
      for (const entry of entries) {
        if (entry.type === 'object') {
          payloads.push(data.subarray(Number(entry.offset), Number(entry.offset) + Number(entry.length)).toString())
        }
      }
    }
    expect(payloads.some((payload) => payload.includes('archive chat message'))).toBe(true)

    await alice.page.page.getByTestId('activities-button').click()
    await expect(alice.page.page.getByTestId('record-moq')).toBeVisible()
    await expect(alice.page.page.getByTestId('record-play')).toBeVisible()

    await page.goto(
      `/moq-wasm/examples/recording-player/?recorder=${encodeURIComponent(recorderServe.url)}&id=${recording.id}`
    )
    await page.locator('#load').click()
    await expect(page.locator('#status')).toHaveText('Ready', { timeout: 30_000 })
    expect(await page.locator('canvas').count()).toBeGreaterThanOrEqual(3)
    await expect(page.locator('#chat li')).toHaveCount(0)
    await expect(page.locator('#transcript li')).toHaveCount(0)
    await page.locator('#seek').evaluate((element: HTMLInputElement) => {
      element.value = String(Math.max(0, Number(element.max) - 1))
      element.dispatchEvent(new Event('change', { bubbles: true }))
    })
    await expect(page.locator('#chat')).toContainText('archive chat message')
    await expect(page.locator('#transcript')).toContainText('archived transcript')
    await expect
      .poll(async () => page.locator('canvas[data-capture-us]').count(), { timeout: 15_000 })
      .toBeGreaterThanOrEqual(3)
    await page.locator('#seek').evaluate((element: HTMLInputElement) => {
      element.value = '0'
      element.dispatchEvent(new Event('change', { bubbles: true }))
    })
    await expect(page.locator('#chat li')).toHaveCount(0)
    await expect(page.locator('#transcript li')).toHaveCount(0)
  } finally {
    await alice.context.close()
    await bob.context.close()
  }
})
