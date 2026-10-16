import { expect, test } from '@playwright/test'
import { setTimeout as sleep } from 'node:timers/promises'
import { listenForRelayMove, processAlive, relayAPid } from './goaway-e2e-arrange'
import { liveViewerE2EConfig } from './live-viewer-e2e-arrange'
import { RELAY_B_URL, arrangeMeetingClient, enableMedia, joinRoom } from './meeting-e2e-arrange'
import { longestRecording, probeRecording, startRecorderServe } from './recorder-e2e-arrange'

const STEADY_SECONDS = 5
const AFTER_EXIT_SECONDS = 5
/// Below relay A's 30 s drain timeout, after which A closes the sessions still on it. Until then the participant also
/// serves the recorder's subscriptions on A, so only an exit before the timeout shows that the recorder left A.
const RELAY_EXIT_TIMEOUT_MS = 20_000
/// The combined layout starts a second file at the next keyframe when the microphone turns on after the camera.
const MAX_FILES_PER_TRACK = 2
/// Fails when a group is lost across the move. Video: about 4 frame intervals of the 15 fps fake camera (67 ms), below
/// the 1 s keyframe interval of the meeting E2E pages. Audio: Opus sends a packet every 20 ms.
const MAX_FRAME_GAP_SECONDS = 0.3
/// The longest file starts at the first keyframe after the microphone is on, up to one 1 s keyframe interval after
/// `mediaOnAt`.
const START_MARGIN_SECONDS = 1.5

test.setTimeout(120_000)

test('recorder serve keeps recording a meeting participant when the relay drains with GOAWAY', async ({ browser }) => {
  // Arrange: recorder serve and the participant both use relay A
  expect(Number.isInteger(relayAPid)).toBe(true)
  const recorder = await startRecorderServe()
  const client = await arrangeMeetingClient(browser, { params: { relayAUrl: liveViewerE2EConfig.moqtUrl } })

  try {
    const room = `recorder-goaway-${Date.now()}`
    const recording = await recorder.start(`anon/${room}`, 'combined')
    await joinRoom(client.page, room, 'alice', 'a')
    const participantMove = listenForRelayMove(client.page.page)
    await enableMedia(client.page)
    const mediaOnAt = Date.now()
    await sleep(STEADY_SECONDS * 1_000)

    // Act: relay A stops and sends GOAWAY naming relay B; the recording goes on after A has exited
    process.kill(relayAPid, 'SIGINT')
    const participantRelayUrl = await participantMove()
    await expect.poll(() => processAlive(relayAPid), { timeout: RELAY_EXIT_TIMEOUT_MS }).toBe(false)
    await sleep(AFTER_EXIT_SECONDS * 1_000)
    const stoppedAt = Date.now()
    const files = await recorder.stop(recording)

    // Assert: the participant moved to relay B
    expect(participantRelayUrl).toBe(RELAY_B_URL)
    const probes = files.map(probeRecording)
    const recordedSeconds = (stoppedAt - mediaOnAt) / 1_000
    for (const track of ['video', 'audio'] as const) {
      const trackFiles = probes.filter((probe) => probe.packetSeconds[track].length > 0)
      expect(trackFiles.length, `${track} files`).toBeLessThanOrEqual(MAX_FILES_PER_TRACK)
      const file = longestRecording(trackFiles, track)
      const times = file.packetSeconds[track]
      expect(file.decodeErrors, `${track} decodes`).toBe('')
      // Assert: the track goes on until the stop, past relay A's exit, without a gap at the move
      expect(times.at(-1)! - times[0], `${track} recorded seconds`).toBeGreaterThan(
        recordedSeconds - START_MARGIN_SECONDS
      )
      expect(file.largestGapSeconds[track], `${track} gap`).toBeLessThan(MAX_FRAME_GAP_SECONDS)
    }
  } finally {
    await client.context.close()
    await recorder.close()
  }
})
