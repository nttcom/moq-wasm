import { expect, test } from '@playwright/test'
import { RequestError } from '../core/requestError'
import { type TranscriptRecord, subscribeTranscript } from './transcript'
import { FakeTextTrackSession } from './player/testing/fakeTextTrackSession'

const ROOM = ['app', 'room']

const record = (t: number, speaker: string, text: string) => JSON.stringify([{ T: t, data: { speaker, text } }])

/// Lets the reader of a subscription take what was delivered.
function settle(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0))
}

test('the transcript of the transcriber is read from the current group', async () => {
  // Arrange
  const fake = new FakeTextTrackSession()
  const heard: TranscriptRecord[][] = []

  // Act
  await subscribeTranscript(fake.session, ROOM, (records) => heard.push(records))

  // Assert
  expect(fake.subscriptions.map(({ namespace, track }) => [namespace, track])).toEqual([
    [['app', 'room', 'bots', 'transcriber'], 'transcript']
  ])
  expect(heard).toEqual([])
})

test('records up to the largest object came with history and later ones are live', async () => {
  // Arrange
  const fake = new FakeTextTrackSession()
  fake.largest = { groupId: 10n, objectId: 1n }
  const heard: TranscriptRecord[][] = []
  await subscribeTranscript(fake.session, ROOM, (records) => heard.push(records))
  const [subscription] = fake.open('transcript')

  // Act
  subscription.deliver(10n, record(1_000, 'alice', 'こんにちは'), 0n)
  subscription.deliver(10n, record(2_000, 'bob', 'どうも'), 1n)
  subscription.deliver(10n, record(3_000, 'carol', 'よろしく'), 2n)
  await settle()

  // Assert: every change hands the whole timeline
  expect(heard.map((records) => records.map(({ speaker, live }) => `${speaker}:${live}`))).toEqual([
    ['alice:false'],
    ['alice:false', 'bob:false'],
    ['alice:false', 'bob:false', 'carol:true']
  ])
})

test('a SUBSCRIBE the relay rejects rejects with its RequestError', async () => {
  // Arrange
  const fake = new FakeTextTrackSession()
  fake.absent = true

  // Act
  const subscribed = subscribeTranscript(fake.session, ROOM, () => {})

  // Assert
  await expect(subscribed).rejects.toBeInstanceOf(RequestError)
})

test('after close no records are handed, even from a read pending at close, and the track is unsubscribed', async () => {
  // Arrange
  const fake = new FakeTextTrackSession()
  const heard: TranscriptRecord[][] = []
  const transcript = await subscribeTranscript(fake.session, ROOM, (records) => heard.push(records))
  const [subscription] = fake.subscriptions
  await settle()

  // Act
  const closing = transcript.close()
  subscription.deliver(10n, record(1_000, 'alice', 'こんにちは'))
  await closing
  await settle()

  // Assert
  expect(subscription.closed).toBe(true)
  expect(heard).toEqual([])
})

test('done resolves with the PUBLISH_DONE the transcriber ends the transcript with', async () => {
  // Arrange
  const fake = new FakeTextTrackSession()
  const transcript = await subscribeTranscript(fake.session, ROOM, () => {})

  // Act
  fake.subscriptions[0].end({ code: 2n, reason: 'track ended' })

  // Assert
  expect(await transcript.done).toEqual({ code: 2n, reason: 'track ended' })
})

test('done resolves to undefined when the session ends without PUBLISH_DONE', async () => {
  // Arrange
  const fake = new FakeTextTrackSession()
  const transcript = await subscribeTranscript(fake.session, ROOM, () => {})

  // Act
  fake.subscriptions[0].end(undefined)

  // Assert
  expect(await transcript.done).toBeUndefined()
})
