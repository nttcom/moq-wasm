import { type BrowserContext, type Page, expect } from '@playwright/test'
import { setTimeout as sleep } from 'node:timers/promises'
import { type RecordingLayout } from '../examples/meeting/src/recording/recorderApi'
import { liveViewerE2EConfig } from './live-viewer-e2e-arrange'
import {
  type MeetingClientPageModel,
  arrangeMeetingClient,
  enableMedia,
  joinRoom,
  startScreenShare
} from './meeting-e2e-arrange'
import { longestRecording, probeRecording, meanColor, splitAtGaps, test } from './recorder-e2e-arrange'

/// The first mute ends within moqt's 10 s PUBLISH_DONE grace, the second one past it.
const MUTE_SECONDS = [4, 14]
const UNMUTED_SECONDS = 3
const LAYOUTS: RecordingLayout[] = ['combined', 'per-track']
/// In the combined layout the microphone, turned on after the camera, starts a second file at the
/// next keyframe; the mutes and unmutes all land in the longest file of each track.
const MAX_FILES_PER_TRACK = 2
/// About 4 frame intervals of the 15 fps fake camera (67 ms); Opus sends a packet every 20 ms.
const MAX_FRAME_GAP_SECONDS = 0.3
/// Unmuting starts a new encoder, so the audio comes back later than the unmute click.
const MUTE_GAP_MARGIN_SECONDS = 2
const COMPOSITE_RECORDING_SECONDS = 6
/// The composite's right edge holds the strip of cameras beside the picture in focus, 1/5 of its width.
const COMPOSITE_STRIP_LEFT = 4 / 5

test.setTimeout(180_000)

test('recorder serve keeps recording a meeting participant who mutes and unmutes the microphone', async ({
  browser,
  recorderServe: recorder
}) => {
  // Arrange
  const contexts: BrowserContext[] = []

  try {
    const participants = await Promise.all(
      LAYOUTS.map(async (layout) => {
        const room = `recorder-${layout}-${Date.now()}`
        const recording = await recorder.start(`anon/${room}`, layout)
        const client = await arrangeMeetingClient(browser, { params: { relayAUrl: liveViewerE2EConfig.moqtUrl } })
        contexts.push(client.context)
        await joinRoom(client.page, room, 'alice', 'a')
        await enableMedia(client.page)
        return { layout, recording, page: client.page }
      })
    )
    await sleep(UNMUTED_SECONDS * 1_000)

    // Act
    await Promise.all(participants.map(({ page }) => muteAndUnmute(page)))
    const files = await Promise.all(participants.map(({ recording }) => recorder.stop(recording)))

    // Assert
    for (const [index, { layout }] of participants.entries()) {
      const probes = files[index].map(probeRecording)
      const videoFiles = probes.filter((probe) => probe.packetSeconds.video.length > 0)
      const audioFiles = probes.filter((probe) => probe.packetSeconds.audio.length > 0)
      expect(videoFiles.length, `${layout} video files`).toBeLessThanOrEqual(MAX_FILES_PER_TRACK)
      expect(audioFiles.length, `${layout} audio files`).toBeLessThanOrEqual(MAX_FILES_PER_TRACK)
      const video = longestRecording(videoFiles, 'video')
      const audio = longestRecording(audioFiles, 'audio')
      expect(video.decodeErrors, `${layout} video decodes`).toBe('')
      expect(audio.decodeErrors, `${layout} audio decodes`).toBe('')
      // Assert: the video goes on through both mutes
      expect(video.largestGapSeconds.video, `${layout} video gap`).toBeLessThan(MAX_FRAME_GAP_SECONDS)
      // Assert: the audio pauses for each mute only and comes back after each unmute
      const audioRuns = splitAtGaps(audio.packetSeconds.audio, MAX_FRAME_GAP_SECONDS)
      expect(audioRuns, `${layout} audio runs`).toHaveLength(MUTE_SECONDS.length + 1)
      for (const [mute, seconds] of MUTE_SECONDS.entries()) {
        const gap = audioRuns[mute + 1][0] - audioRuns[mute].at(-1)!
        expect(gap, `${layout} audio gap of mute ${mute + 1}`).toBeGreaterThan(seconds - MAX_FRAME_GAP_SECONDS)
        expect(gap, `${layout} audio gap of mute ${mute + 1}`).toBeLessThan(seconds + MUTE_GAP_MARGIN_SECONDS)
      }
      for (const run of audioRuns.slice(1)) {
        expect(run.at(-1)! - run[0], `${layout} audio after an unmute`).toBeGreaterThan(UNMUTED_SECONDS - 1)
      }
    }
  } finally {
    await Promise.all(contexts.map((context) => context.close()))
  }
})

test('recorder serve composites a screen share in focus beside the cameras of a room', async ({
  browser,
  recorderServe: recorder
}) => {
  // Arrange
  const contexts: BrowserContext[] = []

  try {
    const room = `recorder-composite-${Date.now()}`
    const recording = await recorder.start(`anon/${room}`, 'combined', true)
    const clients: MeetingClientPageModel[] = []
    for (const name of ['alice', 'bob']) {
      const client = await arrangeMeetingClient(browser, { params: { relayAUrl: liveViewerE2EConfig.moqtUrl } })
      contexts.push(client.context)
      await joinRoom(client.page, room, name, 'a')
      await enableMedia(client.page)
      clients.push(client.page)
    }
    await startScreenShare(clients[0])
    await fillWithMagenta(clients[0].page)
    await sleep(COMPOSITE_RECORDING_SECONDS * 1_000)

    // Act
    await recorder.stop(recording)
    const composite = await recorder.composite(recording)

    // Assert: alice's screen share is recorded apart from the camera
    const alice = recorder.manifest(recording).members.find((member) => member.namespace.endsWith('/alice'))
    expect(alice?.screen_share_files.length, 'alice screen share files').toBeGreaterThan(0)
    // Assert: one H.264 video and the mixed AAC, lasting about as long as the recording
    const probe = probeRecording(composite)
    expect(probe.codecs.sort()).toEqual(['aac', 'h264'])
    expect(probe.decodeErrors).toBe('')
    expect(probe.durationSeconds).toBeGreaterThan(COMPOSITE_RECORDING_SECONDS - 2)
    expect(probe.largestGapSeconds.video).toBeLessThan(MAX_FRAME_GAP_SECONDS)
    // Assert: the shared magenta tab fills the focus on the left, the cameras the strip on the right
    const focus = meanColor(composite, 1, [0.3, 0.3, 0.5, 0.7])
    const strip = meanColor(composite, 1, [COMPOSITE_STRIP_LEFT + 0.02, 0.02, 0.98, 0.18])
    expect(magentaness(focus), `focus color ${focus}`).toBeGreaterThan(150)
    expect(magentaness(strip), `strip color ${strip}`).toBeLessThan(50)
  } finally {
    await Promise.all(contexts.map((context) => context.close()))
  }
})

/// The color flickers so that the page keeps repainting: a tab capture sends a frame only when the tab changes.
async function fillWithMagenta(page: Page): Promise<void> {
  await page.addStyleTag({
    content: `
      @keyframes e2e-flicker { to { background: #f0e } }
      body::after { content: ''; position: fixed; inset: 0; z-index: 2147483647; background: #f0f;
        animation: e2e-flicker 100ms infinite alternate }`
  })
}

/// How far red and blue both stand above green: 255 for magenta, near 0 for grey.
function magentaness([red, green, blue]: [number, number, number]): number {
  return Math.min(red, blue) - green
}

async function muteAndUnmute(model: MeetingClientPageModel): Promise<void> {
  for (const seconds of MUTE_SECONDS) {
    await model.toggleMicrophoneButton.click()
    await expect(model.toggleMicrophoneButton).toHaveAccessibleName('Turn microphone on')
    await sleep(seconds * 1_000)
    await model.toggleMicrophoneButton.click()
    await expect(model.toggleMicrophoneButton).toHaveAccessibleName('Turn microphone off')
    await sleep(UNMUTED_SECONDS * 1_000)
  }
}
