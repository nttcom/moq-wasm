import { expect, test } from '@playwright/test'
import {
  arrangeMeetingClient,
  getMemberCard,
  joinRoom,
  openChat,
  type MeetingClientPageModel
} from './meeting-e2e-arrange'

// Set by tests/browser-e2e/run-meeting-transcription-e2e.mjs, with speech on every page's fake microphone.
const BOT_URL = process.env.MEETING_TRANSCRIPTION_E2E_BOT_URL!
// The bot connects and loads Whisper when a page asks for it.
const BOT_JOIN_TIMEOUT_MS = 90_000
// Speech is transcribed once the VAD closes an utterance.
const TRANSCRIPT_TIMEOUT_MS = 60_000

test.setTimeout(300_000)

async function joinWithCaptionsOn(browser: import('@playwright/test').Browser, room: string, user: string) {
  const client = await arrangeMeetingClient(browser, { params: { transcriberUrl: BOT_URL } })
  await joinRoom(client.page, room, user, 'a')
  await client.page.page.getByTestId('captions-button').click()
  await client.page.page.getByTestId('captions-whisper').click()
  return client
}

async function openTranscript(model: MeetingClientPageModel) {
  await openChat(model)
  await model.page.getByTestId('transcript-tab').click()
  return model.page.getByTestId('transcript-record')
}

test('captions bring the transcriber, show what a participant says and reach a late joiner', async ({ browser }) => {
  // Arrange: alice speaks; a room name with a space reaches the bot through the URL
  const room = `captions room ${Date.now() % 100_000}`
  const alice = await arrangeMeetingClient(browser)
  await joinRoom(alice.page, room, 'alice', 'a')
  await alice.page.toggleMicrophoneButton.click()
  await expect(alice.page.toggleMicrophoneButton).toHaveAccessibleName('Turn microphone off', { timeout: 15_000 })

  // Act
  const bob = await joinWithCaptionsOn(browser, room, 'bob')

  // Assert: everyone sees that the room is transcribed, and bob reads alice
  await expect(bob.page.page.getByTestId('transcriber-badge')).toBeVisible({ timeout: BOT_JOIN_TIMEOUT_MS })
  await expect(alice.page.page.getByTestId('transcriber-badge')).toHaveText('CC · Whisper (CPU)', {
    timeout: BOT_JOIN_TIMEOUT_MS
  })
  await expect(getMemberCard(bob.page.page, 'alice').getByTestId('tile-caption')).toHaveText(/\S/, {
    timeout: TRANSCRIPT_TIMEOUT_MS
  })
  const bobRecords = await openTranscript(bob.page)
  await expect.poll(() => bobRecords.count(), { timeout: TRANSCRIPT_TIMEOUT_MS }).toBeGreaterThanOrEqual(2)
  await alice.page.toggleMicrophoneButton.click()
  await expect(alice.page.toggleMicrophoneButton).toHaveAccessibleName('Turn microphone on')
  const saidBefore = await bobRecords.count()

  // Act: carol turns captions on while alice is muted, so only the joining FETCH can bring her records
  const carol = await joinWithCaptionsOn(browser, room, 'carol')

  // Assert: carol gets every record bob saw before she joined
  const carolRecords = await openTranscript(carol.page)
  await expect.poll(() => carolRecords.count(), { timeout: TRANSCRIPT_TIMEOUT_MS }).toBeGreaterThanOrEqual(saidBefore)

  for (const client of [carol, bob, alice]) {
    await client.page.leaveButton.click()
    await client.context.close()
  }
})
