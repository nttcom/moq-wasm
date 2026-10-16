import { type BrowserContext, expect, test } from '@playwright/test'
import { setTimeout as sleep } from 'node:timers/promises'
import { type RecordingLayout } from '../examples/meeting/src/recording/recorderApi'
import { liveViewerE2EConfig } from './live-viewer-e2e-arrange'
import { type MeetingClientPageModel, arrangeMeetingClient, enableMedia, joinRoom } from './meeting-e2e-arrange'
import { longestRecording, probeRecording, splitAtGaps, startRecorderServe } from './recorder-e2e-arrange'

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

test.setTimeout(180_000)

test('recorder serve keeps recording a meeting participant who mutes and unmutes the microphone', async ({
  browser
}) => {
  // Arrange
  const recorder = await startRecorderServe()
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
    await recorder.close()
  }
})

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
