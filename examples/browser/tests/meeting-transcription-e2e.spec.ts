import { type Browser, expect, test } from '@playwright/test'
import {
  arrangeMeetingClient,
  getMemberCard,
  joinRoom,
  openChat,
  type MeetingClientPageModel
} from './meeting-e2e-arrange'
import { FAKE_ASR_TEXT, startFakeAsr } from './fake-asr'

// Set by tests/browser-e2e/run-meeting-transcription-e2e.mjs, with speech on every page's fake microphone. The
// pages start the transcriber through this temporary-apps service, which calls the bot service.
const TEMPORARY_APPS_URL = process.env.MEETING_TRANSCRIPTION_E2E_TEMPORARY_APPS_URL!
// The bot's `--asr granite-speech-5-2b=http://127.0.0.1:<port>` points here; the test serves it.
const ASR_PORT = Number(process.env.MEETING_TRANSCRIPTION_E2E_ASR_PORT)
// The bot service starts the bot's process, which connects and loads Whisper.
const BOT_JOIN_TIMEOUT_MS = 120_000
// Speech is transcribed once the VAD closes an utterance.
const TRANSCRIPT_TIMEOUT_MS = 60_000

test.setTimeout(300_000)

async function expectCaptionsShown(model: MeetingClientPageModel, shown: boolean) {
  await model.page.getByTestId('activities-button').click()
  await expect(model.page.getByTestId('captions-show')).toHaveAttribute('aria-checked', String(shown))
  await model.page.keyboard.press('Escape')
}

/// A new temporary app, whose token every page of a test joins with.
async function newTemporaryApp(): Promise<string> {
  const response = await fetch(`${TEMPORARY_APPS_URL}/tokens`, { method: 'POST' })
  expect(response.ok).toBe(true)
  return ((await response.json()) as { token: string }).token
}

function arrangeAppClient(browser: Browser, token: string) {
  return arrangeMeetingClient(browser, { params: { temporaryAppsUrl: TEMPORARY_APPS_URL }, fragment: `jwt=${token}` })
}

async function joinWithCaptionsOn(browser: Browser, token: string, room: string, user: string) {
  const client = await arrangeAppClient(browser, token)
  await joinRoom(client.page, room, user, 'a')
  await client.page.page.getByTestId('activities-button').click()
  await client.page.page.getByTestId('captions-whisper').click()
  return client
}

async function openTranscript(model: MeetingClientPageModel) {
  await openChat(model)
  await model.page.getByTestId('transcript-tab').click()
  return model.page.getByTestId('transcript-record')
}

test('captions bring the transcriber, reach a late joiner and stay until someone stops them', async ({ browser }) => {
  // Arrange: alice speaks; a room name with a space reaches the bot in its namespace
  const room = `captions room ${Date.now() % 100_000}`
  const token = await newTemporaryApp()
  const alice = await arrangeAppClient(browser, token)
  await joinRoom(alice.page, room, 'alice', 'a')
  await alice.page.toggleMicrophoneButton.click()
  await expect(alice.page.toggleMicrophoneButton).toHaveAccessibleName('マイクをオフ', { timeout: 15_000 })

  // Act
  const bob = await joinWithCaptionsOn(browser, token, room, 'bob')

  // Assert: everyone sees that the room is transcribed, and bob reads alice
  await expect(bob.page.page.getByTestId('transcriber-badge')).toBeVisible({ timeout: BOT_JOIN_TIMEOUT_MS })
  await expect(alice.page.page.getByTestId('transcriber-badge')).toHaveText('CC', {
    timeout: BOT_JOIN_TIMEOUT_MS
  })
  await expect(getMemberCard(bob.page.page, 'alice').getByTestId('tile-caption')).toHaveText(/\S/, {
    timeout: TRANSCRIPT_TIMEOUT_MS
  })
  const bobRecords = await openTranscript(bob.page)
  await expect.poll(() => bobRecords.count(), { timeout: TRANSCRIPT_TIMEOUT_MS }).toBeGreaterThanOrEqual(2)
  // Assert: alice, who never asked for captions, sees them and reads the transcript too
  await expectCaptionsShown(alice.page, true)
  await expect(getMemberCard(alice.page.page, 'local').getByTestId('tile-caption')).toHaveText(/\S/, {
    timeout: TRANSCRIPT_TIMEOUT_MS
  })
  const aliceRecords = await openTranscript(alice.page)
  await expect.poll(() => aliceRecords.count(), { timeout: TRANSCRIPT_TIMEOUT_MS }).toBeGreaterThanOrEqual(2)

  // Act
  await alice.page.page.getByTestId('activities-button').click()
  await alice.page.page.getByTestId('captions-hide').click()

  // Assert: only alice's page hides them; the transcriber stays
  await expectCaptionsShown(alice.page, false)
  await expect(getMemberCard(alice.page.page, 'local').getByTestId('tile-caption')).toBeHidden()
  await expectCaptionsShown(bob.page, true)
  await expect(bob.page.page.getByTestId('transcriber-badge')).toBeVisible()

  await alice.page.toggleMicrophoneButton.click()
  await expect(alice.page.toggleMicrophoneButton).toHaveAccessibleName('マイクをオン')
  const saidBefore = await bobRecords.count()

  // Act: carol joins while alice is muted, so only the joining FETCH can bring alice's records
  const carol = await arrangeAppClient(browser, token)
  await joinRoom(carol.page, room, 'carol', 'a')

  // Assert: carol gets every record bob saw before she joined
  const carolRecords = await openTranscript(carol.page)
  await expect.poll(() => carolRecords.count(), { timeout: TRANSCRIPT_TIMEOUT_MS }).toBeGreaterThanOrEqual(saidBefore)

  // Act: bob, who asked the transcriber in, leaves
  await bob.page.leaveButton.click()
  await bob.context.close()
  await alice.page.page.waitForTimeout(5_000)

  // Assert
  await expect(alice.page.page.getByTestId('transcriber-badge')).toBeVisible()

  // Act
  const dialogMessages: string[] = []
  carol.page.page.once('dialog', (dialog) => {
    dialogMessages.push(dialog.message())
    void dialog.dismiss()
  })
  await carol.page.page.getByTestId('activities-button').click()
  await carol.page.page.getByTestId('captions-stop').click()
  await alice.page.page.waitForTimeout(5_000)

  // Assert: cancelling keeps the transcriber
  expect(dialogMessages).toEqual(['全員の文字起こしを停止しますか？'])
  await expect(alice.page.page.getByTestId('transcriber-badge')).toBeVisible()
  await expectCaptionsShown(carol.page, true)

  // Act
  carol.page.page.once('dialog', (dialog) => void dialog.accept())
  await carol.page.page.getByTestId('activities-button').click()
  await carol.page.page.getByTestId('captions-stop').click()

  // Assert: the transcriber leaves at its next heartbeat rather than after the grace period, on every page
  await expect(alice.page.page.getByTestId('transcriber-badge')).toBeHidden({ timeout: 15_000 })
  await expect(carol.page.page.getByTestId('transcriber-badge')).toBeHidden()

  for (const client of [carol, alice]) {
    await client.page.leaveButton.click()
    await client.context.close()
  }
})

test('a GPU recognizer that is starting is announced, then its text reaches the captions and the transcript', async ({
  browser
}) => {
  // Arrange: /health fails for 8 s, past the bot's 2 s notice; of its polls at 0, 5 and 10 s, the third passes
  const asr = await startFakeAsr(ASR_PORT)
  asr.unhealthyFor(8_000)
  const room = `granite room ${Date.now() % 100_000}`
  const token = await newTemporaryApp()
  const alice = await arrangeAppClient(browser, token)
  await joinRoom(alice.page, room, 'alice', 'a')
  const badge = alice.page.page.getByTestId('transcriber-badge')

  // Act
  await alice.page.page.getByTestId('activities-button').click()
  await alice.page.page.getByTestId('captions-granite-speech-5-2b').click()

  // Assert: the badge and one transcript record say the recognizer is starting
  await expect(badge).toHaveText('CC starting', { timeout: BOT_JOIN_TIMEOUT_MS })
  const aliceRecords = await openTranscript(alice.page)
  await expect(aliceRecords).toHaveCount(1)
  await expect(aliceRecords.first()).toContainText('transcriber')
  await expect(aliceRecords.first()).toContainText('granite-speech-5-2b を起動しています')
  await expect(badge).toHaveText('CC', { timeout: 30_000 })
  await expect(aliceRecords).toHaveCount(1)

  // Act
  const bob = await arrangeAppClient(browser, token)
  await joinRoom(bob.page, room, 'bob', 'a')
  await bob.page.toggleMicrophoneButton.click()
  await expect(bob.page.toggleMicrophoneButton).toHaveAccessibleName('マイクをオフ', { timeout: 15_000 })

  // Assert
  await expect(getMemberCard(alice.page.page, 'bob').getByTestId('tile-caption')).toContainText(FAKE_ASR_TEXT, {
    timeout: TRANSCRIPT_TIMEOUT_MS
  })
  await expect(aliceRecords.nth(1)).toContainText('bob')
  await expect(aliceRecords.nth(1)).toContainText(FAKE_ASR_TEXT)
  expect(asr.requests.length).toBeGreaterThan(0)
  for (const request of asr.requests) {
    expect(request.model).toBe('granite-speech-5-2b')
    expect(request.prompt).toContain('The speech is in Japanese')
  }

  for (const client of [bob, alice]) {
    await client.page.leaveButton.click()
    await client.context.close()
  }
  asr.close()
})
