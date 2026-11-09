import { expect } from '@playwright/test'
import { spawn } from 'node:child_process'
import { setTimeout as sleep } from 'node:timers/promises'
import { liveViewerE2EConfig } from './live-viewer-e2e-arrange'
import type { PlaybackGroupSnapshot } from './playback-group/main'
import { test } from './recorder-e2e-arrange'

const PAGE_PATH = '/moq-wasm/tests/playback-group/index.html'
const PEER_NAMESPACE = `${liveViewerE2EConfig.namespace}-peer`
const RTMP_ADDRESS = process.env.LIVE_VIEWER_E2E_RTMP_ADDR ?? '127.0.0.1:1935'
/// live-ingest announces a stream once it has detected its codecs.
const PEER_START_SECONDS = 5
/// Long enough for a seek, a player rewinding alone and its return.
const RECORDED_SECONDS = 30
const SECOND = 1_000_000
/// Between the two players: a frame at 30 fps each, and the main thread's timer slack.
const IN_STEP_MICROS = 100_000
/// Between a player and the group's position, which is what is due now.
const AT_POSITION_MICROS = 150_000

test.setTimeout(240_000)

/// A second participant next to the runner's.
function spawnPeer() {
  return spawn(
    'ffmpeg',
    [
      '-hide_banner',
      '-loglevel',
      'warning',
      '-re',
      '-f',
      'lavfi',
      '-i',
      'testsrc2=size=640x360:rate=30',
      '-f',
      'lavfi',
      '-i',
      'sine=frequency=440:sample_rate=48000',
      '-c:v',
      'libx264',
      '-preset',
      'veryfast',
      '-profile:v',
      'baseline',
      '-pix_fmt',
      'yuv420p',
      '-g',
      '60',
      '-sc_threshold',
      '0',
      '-c:a',
      'aac',
      '-ar',
      '48000',
      '-ac',
      '2',
      '-t',
      '120',
      '-f',
      'flv',
      `rtmp://${RTMP_ADDRESS}/${PEER_NAMESPACE}/stream`
    ],
    { stdio: 'ignore' }
  )
}

function bothPlaying({ group, players }: PlaybackGroupSnapshot, fromMicros: number): boolean {
  return (
    group.mode === 'review' &&
    !group.paused &&
    players.every(
      ({ inRange, presentedMicros, shownAgoMs }) =>
        inRange &&
        presentedMicros !== undefined &&
        presentedMicros >= fromMicros &&
        shownAgoMs !== undefined &&
        shownAgoMs < 200
    )
  )
}

function presentedGap({ players: [first, second] }: PlaybackGroupSnapshot): number {
  return Math.abs((first.presentedMicros ?? Number.NaN) - (second.presentedMicros ?? Number.NaN))
}

function inStep(current: PlaybackGroupSnapshot, fromMicros: number): boolean {
  const [first] = current.players
  return (
    bothPlaying(current, fromMicros) &&
    presentedGap(current) < IN_STEP_MICROS &&
    Math.abs((first.presentedMicros ?? 0) - (current.group.positionMicros ?? 0)) < AT_POSITION_MICROS
  )
}

/// A player that has just started shows the frames that are due already as
/// fast as it decodes them, so both are in step once it has caught up.
async function expectInStep(snapshot: () => Promise<PlaybackGroupSnapshot>, fromMicros: number): Promise<void> {
  await expect.poll(async () => inStep(await snapshot(), fromMicros), { timeout: 15_000 }).toBe(true)
  for (let sample = 0; sample < 5; sample += 1) {
    await sleep(500)
    const current = await snapshot()
    expect(inStep(current, fromMicros), JSON.stringify(current)).toBe(true)
  }
}

test('a playback group keeps two recorded participants at one position across a seek and a rejoin', async ({
  recorderServe,
  browser
}) => {
  // Arrange: the runner publishes anon/live/e2e, and the peer joins it before the recording starts
  const peer = spawnPeer()
  const context = await browser.newContext({ ignoreHTTPSErrors: true })
  let replay: Awaited<ReturnType<typeof recorderServe.publish>> | undefined
  try {
    await sleep(PEER_START_SECONDS * 1_000)
    const recording = await recorderServe.start('anon/live', 'combined', false, 'moq')
    await sleep(RECORDED_SECONDS * 1_000)
    await recorderServe.stop(recording)
    peer.kill('SIGINT')
    replay = await recorderServe.publish(recording)
    const prefix = `anon/live/recordings/${recording.id}`
    const page = await context.newPage()
    page.on('console', (message) => {
      if (message.type() === 'warning' || message.type() === 'error') {
        console.log(`[page] ${message.text()}`)
      }
    })
    await page.goto(PAGE_PATH)
    const snapshot = () => page.evaluate(() => window.playbackGroupE2E.snapshot())

    try {
      // Act
      await page.evaluate(({ url, namespaces }) => window.playbackGroupE2E.start(url, namespaces), {
        url: liveViewerE2EConfig.moqtUrl,
        namespaces: [`${prefix}/e2e/camera/0`, `${prefix}/${PEER_NAMESPACE.split('/').at(-1)}/camera/0`]
      })

      // Assert
      await expect.poll(async () => bothPlaying(await snapshot(), 0), { timeout: 30_000 }).toBe(true)

      // Act
      const { players } = await snapshot()
      const seekMicros = Math.max(...players.map(({ startMicros }) => startMicros)) + 5 * SECOND
      expect(seekMicros).toBeLessThan(Math.min(...players.map(({ endMicros }) => endMicros)) - 15 * SECOND)
      await page.evaluate((captureMicros) => window.playbackGroupE2E.seek(captureMicros), seekMicros)

      // Assert
      await expectInStep(snapshot, seekMicros)

      // Act: the peer leaves the group and plays alone 5 s behind it
      const position = (await snapshot()).group.positionMicros ?? 0
      await page.evaluate(
        (captureMicros) => window.playbackGroupE2E.removeAndSeek(1, captureMicros),
        position - 5 * SECOND
      )
      await expect.poll(async () => presentedGap(await snapshot()), { timeout: 15_000 }).toBeGreaterThan(3 * SECOND)
      const joinedAtMicros = (await page.evaluate(() => window.playbackGroupE2E.add(1))) ?? 0

      // Assert: rejoining aligns it to the group's position again
      await expectInStep(snapshot, joinedAtMicros)
    } finally {
      await page.evaluate(() => window.playbackGroupE2E.stop())
    }
  } finally {
    peer.kill('SIGINT')
    await replay?.close()
    await context.close()
  }
})
