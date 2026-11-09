import { expect, type Locator, type Page } from '@playwright/test'
import { readFile } from 'node:fs/promises'
import { join } from 'node:path'
import { setTimeout as sleep } from 'node:timers/promises'
import { recordingPlayerPath, test } from './recorder-e2e-arrange'
import { liveViewerE2EConfig } from './live-viewer-e2e-arrange'
import { arrangeMeetingClient, enableMedia, joinRoom, sendChatMessage, startScreenShare } from './meeting-e2e-arrange'

test.setTimeout(180_000)

const SCREEN_SHARE_SETTLE_MS = 3_000

type Track = { name: string; isLive: boolean; trackDuration?: number; role?: string }
type TimelineRecord = [number, [number, number], number]

/// Seeks the recording player as a viewer letting go of the seek bar would.
async function seekTo(page: Page, seconds: number): Promise<void> {
  await page.locator('#seek').evaluate((element: HTMLInputElement, value) => {
    element.value = String(value)
    element.dispatchEvent(new Event('change', { bubbles: true }))
  }, seconds)
}

/// Seeks to a capture timestamp, which the seek bar counts from the recording's start.
async function seekToCapture(page: Page, captureMicros: number): Promise<void> {
  const startMicros = Number(await page.locator('#seek').getAttribute('data-start-us'))
  await seekTo(page, (captureMicros - startMicros) / 1_000_000)
}

/// The capture timestamp of the frame a tile showed last, NaN before one.
async function shownMicros(tile: Locator): Promise<number> {
  return Number((await tile.getAttribute('data-capture-us')) ?? Number.NaN)
}

/// Returns once every replayed broadcast is loaded.
async function openInPlayer(page: Page, recorderUrl: string, id: string): Promise<void> {
  await page.goto(recordingPlayerPath(recorderUrl, id))
  await page.locator('#load').click()
  await expect(page.locator('#status')).toHaveText('Ready', { timeout: 30_000 })
}

test('records a room and plays its VOD replay through the relay to the end', async ({ recorderServe, page }) => {
  // Arrange: the live-viewer runner publishes a broadcast under anon/live
  const recording = await recorderServe.start('anon/live', 'combined', false, 'moq')
  expect(recording.format).toBe('moq')
  await sleep(37_000)
  await recorderServe.stop(recording)
  const room = JSON.parse(await readFile(join(recorderServe.storeDir, recording.id, 'manifest.json'), 'utf8'))
  const member = room.members.find((entry: { namespace: string }) => entry.namespace === 'anon/live/e2e')
  expect(room.format).toBe('moq')
  expect(member.camera.length).toBeGreaterThan(0)
  const archive = JSON.parse(await readFile(join('/', member.camera[0]), 'utf8'))
  expect(archive.tracks.map((track: { name: string }) => track.name)).toEqual(
    expect.arrayContaining(['catalog', 'video', 'audio', 'video_timeline'])
  )

  // Act
  const replay = await recorderServe.publish(recording)
  try {
    await page.goto('/moq-wasm/')
    const prefix = `anon/live/recordings/${recording.id}`
    const [catalog, timeline] = await page.evaluate(
      async ({ relay, prefix, namespace }) => {
        const moduleUrl = '/moq-wasm/tests/recording-replay.ts'
        const { readReplay } = await import(/* @vite-ignore */ moduleUrl)
        return readReplay(relay, prefix.split('/'), namespace.split('/'), ['catalog', 'video_timeline'])
      },
      { relay: liveViewerE2EConfig.moqtUrl, prefix, namespace: `${prefix}/e2e/camera/0` }
    )

    // Assert: the catalog and the timeline cover the recording
    expect(replay.answer).toEqual({
      namespace_prefix: prefix,
      renew_after_seconds: 2700
    })
    expect(catalog.isComplete).toBe(true)
    expect(catalog.generatedAt).toBeUndefined()
    const tracks: Track[] = catalog.tracks
    expect(tracks.map((track) => track.name)).toEqual(expect.arrayContaining(['video', 'audio', 'video_timeline']))
    for (const track of tracks) {
      expect(track.isLive, track.name).toBe(false)
      expect(track.trackDuration, track.name).toBeGreaterThan(30_000)
    }
    const records: TimelineRecord[] = timeline
    const wallclocks = records.map(([, , wallclock]) => wallclock)
    expect(Math.max(...wallclocks) - Math.min(...wallclocks)).toBeGreaterThan(30_000)
  } finally {
    await replay.close()
  }

  // Arrange: a sound with a sample above silence marks the page as having played audio
  await page.addInitScript(() => {
    const start = AudioBufferSourceNode.prototype.start
    AudioBufferSourceNode.prototype.start = function (...args) {
      if (this.buffer?.getChannelData(0).some((sample) => Math.abs(sample) > 0.001))
        document.documentElement.dataset.audioPlayed = 'true'
      return start.apply(this, args)
    }
  })
  await openInPlayer(page, recorderServe.url, recording.id)
  const tile = page.locator('#videos [data-participant="e2e"][data-content="camera"]')

  // Act
  await page.locator('#play').click()

  // Assert: frames and sound play
  await expect.poll(() => shownMicros(tile), { timeout: 20_000 }).toBeGreaterThan(0)
  await expect(page.locator('html')).toHaveAttribute('data-audio-played', 'true', { timeout: 15_000 })
  await expect(page.locator('#status')).toHaveText('Playing')

  // Act
  const beforeSeek = await shownMicros(tile)
  await seekTo(page, 32)

  // Assert: the frames jump to the position
  await expect.poll(() => shownMicros(tile), { timeout: 15_000 }).toBeGreaterThan(beforeSeek + 25_000_000)

  // Act
  await seekTo(page, Number(await page.locator('#seek').getAttribute('max')) - 3)

  // Assert: playback stops, paused, at the end
  await expect(page.locator('#status')).toHaveText('Finished', { timeout: 20_000 })
  await expect(page.locator('#play')).toHaveAttribute('aria-label', 'Play')
  const max = Number(await page.locator('#seek').getAttribute('max'))
  expect(Number(await page.locator('#seek').inputValue())).toBeGreaterThan(max - 1)
})

test('records participants, screen share, chat and transcript and replays them in the recording player', async ({
  recorderServe,
  browser,
  page
}) => {
  // Arrange
  const roomName = `archive-${Date.now()}`
  const alice = await arrangeMeetingClient(browser, { params: { recorderUrl: recorderServe.url } })
  const bob = await arrangeMeetingClient(browser, { params: { recorderUrl: recorderServe.url } })
  try {
    await joinRoom(alice.page, roomName, 'alice', 'a')
    await enableMedia(alice.page)
    await joinRoom(bob.page, roomName, 'bob', 'a')
    await enableMedia(bob.page)
    await startScreenShare(alice.page)
    // Start recording once the screen share settles: it lists its 720p rendition only after its first
    // group, and a replay of a recording that switched renditions is not played whole
    // (docs/todos/high/recorder-replay-rendition-switch.md).
    await sleep(SCREEN_SHARE_SETTLE_MS)
    const recording = await recorderServe.start(`anon/${roomName}`, 'combined', false, 'moq')
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

    // Act
    await recorderServe.stop(recording)
    const replay = await recorderServe.publish(recording)
    const prefix = `anon/${roomName}/recordings/${recording.id}`
    const { aliceCatalog, aliceTranscript, namespaces } = await alice.page.page
      .evaluate(
        async ({ relay, prefix, alice }) => {
          const moduleUrl = '/moq-wasm/tests/recording-replay.ts'
          const { readReplay, announcedNamespaces } = await import(/* @vite-ignore */ moduleUrl)
          const [aliceCatalog, aliceTranscript] = await readReplay(relay, prefix.split('/'), alice.split('/'), [
            'catalog',
            'transcript'
          ])
          return {
            aliceCatalog,
            aliceTranscript,
            namespaces: await announcedNamespaces(relay, prefix.split('/'), 2_000)
          }
        },
        { relay: liveViewerE2EConfig.moqtUrl, prefix, alice: `${prefix}/alice/camera/0` }
      )
      .finally(() => replay.close())

    // Assert
    const room = JSON.parse(await readFile(join(recorderServe.storeDir, recording.id, 'manifest.json'), 'utf8'))
    const aliceMember = room.members.find(
      (member: { namespace: string }) => member.namespace === `anon/${roomName}/alice`
    )
    const bobMember = room.members.find((member: { namespace: string }) => member.namespace === `anon/${roomName}/bob`)
    expect(aliceMember?.camera.length).toBeGreaterThan(0)
    expect(aliceMember?.screen_share.length).toBeGreaterThan(0)
    expect(bobMember?.camera.length).toBeGreaterThan(0)
    const archive = JSON.parse(await readFile(join('/', aliceMember.camera[0]), 'utf8'))
    const names = archive.tracks.map((track: { name: string }) => track.name)
    expect(names).toContain('video_timeline')
    const share = JSON.parse(await readFile(join('/', aliceMember.screen_share[0]), 'utf8'))
    expect(share.tracks.map((track: { name: string }) => track.name)).toContain('screenshare_timeline')
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
    const aliceTracks: Track[] = aliceCatalog.tracks
    const aliceAudio = aliceTracks.filter((track) => track.role === 'audio').map((track) => track.name)
    expect(aliceAudio.length).toBeGreaterThan(0)
    expect(aliceTracks.find((track) => track.name === 'transcript')).toMatchObject({
      packaging: 'eventtimeline',
      role: 'subtitle',
      eventType: 'com.skyway.transcript.v1',
      depends: aliceAudio,
      isLive: false
    })
    expect(aliceTranscript).toEqual([
      expect.objectContaining({ data: { speaker: 'alice', text: 'archived transcript' } })
    ])
    expect(namespaces).toContain(`${prefix}/alice/camera/0`)
    expect(namespaces.filter((namespace: string) => namespace.startsWith(`${prefix}/bots/`))).toEqual([])
    await alice.page.page.getByTestId('activities-button').click()
    await expect(alice.page.page.getByTestId('record-moq')).toBeVisible()
    await expect(alice.page.page.getByTestId('record-play')).toBeVisible()

    // Act
    await openInPlayer(page, recorderServe.url, recording.id)

    // Assert: one tile per participant's camera and screen share, and nothing said yet at the start
    const tiles = page.locator('#videos [data-participant]')
    await expect(tiles).toHaveCount(3)
    const aliceCamera = page.locator('#videos [data-participant="alice"][data-content="camera"]')
    const bobCamera = page.locator('#videos [data-participant="bob"][data-content="camera"]')
    const aliceShare = page.locator('#videos [data-participant="alice"][data-content="screen_share"]')
    await expect(page.locator('#chat li')).toHaveCount(0)
    await expect(page.locator('#transcript li')).toHaveCount(0)

    // Act
    await seekTo(page, Number(await page.locator('#seek').getAttribute('max')) - 1)

    // Assert: the chat and the transcript up to the end
    await expect(page.locator('#chat')).toContainText('archive chat message')
    await expect(page.locator('#transcript')).toContainText('archived transcript')
    const chatMicros = Number(await page.locator('#chat li').first().getAttribute('data-time-us'))
    const transcriptMicros = Number(await page.locator('#transcript li').first().getAttribute('data-time-us'))

    // Act / Assert: each appears once the position reaches its time
    await seekToCapture(page, chatMicros - 1_000_000)
    await expect(page.locator('#chat li')).toHaveCount(0)
    await seekToCapture(page, chatMicros + 500_000)
    await expect(page.locator('#chat li')).toHaveCount(1)
    await seekToCapture(page, transcriptMicros + 1_000_000)
    await expect(page.locator('#transcript li')).toHaveCount(1)
    await expect(aliceCamera.getByText('archived transcript')).toBeVisible()
    await expect(bobCamera.getByText('archived transcript')).toBeHidden()

    // Act
    await page.locator('#play').click()

    // Assert: every participant's pictures play from the position
    for (const tile of [aliceCamera, bobCamera, aliceShare]) {
      await expect.poll(() => shownMicros(tile), { timeout: 20_000 }).toBeGreaterThan(transcriptMicros)
    }
    for (const tile of [aliceCamera, bobCamera]) {
      const shown = await shownMicros(tile)
      await expect.poll(() => shownMicros(tile), { timeout: 5_000 }).toBeGreaterThan(shown + 500_000)
    }

    // Assert: playback stops at the end
    await expect(page.locator('#status')).toHaveText('Finished', { timeout: 30_000 })
  } finally {
    await alice.context.close()
    await bob.context.close()
  }
})
