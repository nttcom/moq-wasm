import { expect } from '@playwright/test'
import { readFile } from 'node:fs/promises'
import { join } from 'node:path'
import { setTimeout as sleep } from 'node:timers/promises'
import { test } from './recorder-e2e-arrange'
import { liveViewerE2EConfig } from './live-viewer-e2e-arrange'
import { arrangeMeetingClient, enableMedia, joinRoom, sendChatMessage, startScreenShare } from './meeting-e2e-arrange'

test.setTimeout(120_000)

type Track = { name: string; isLive: boolean; trackDuration?: number; role?: string }
type TimelineRecord = [number, [number, number], number]

test('records a room and replays it through the relay as a VOD broadcast', async ({ recorderServe, page }) => {
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
})

test('records participants, screen share, chat and transcript', async ({ recorderServe, browser }) => {
  // Arrange
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

    // Act
    await recorderServe.stop(recording)

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
    await alice.page.page.getByTestId('activities-button').click()
    await expect(alice.page.page.getByTestId('record-moq')).toBeVisible()
    await expect(alice.page.page.getByTestId('record-play')).toBeVisible()
  } finally {
    await alice.context.close()
    await bob.context.close()
  }
})
