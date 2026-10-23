import { expect, test } from '@playwright/test'
import { TranscriptTimeline, freshCaptions, parseTranscriberStatus, transcriptRecordKey } from './transcript'

const encode = (records: { t: number; speaker: string; text: string }[]) =>
  new TextEncoder().encode(JSON.stringify(records.map(({ t, speaker, text }) => ({ t, data: { speaker, text } }))))
const ALICE = { t: 1_000, speaker: 'alice', text: 'こんにちは' }
const BOB = { t: 2_000, speaker: 'bob', text: '今日は良い天気ですね' }
const CAROL = { t: 3_000, speaker: 'carol', text: 'よろしくお願いします' }

test('the first object of a group holds the history and later objects add to it', () => {
  // Arrange
  const timeline = new TranscriptTimeline()

  // Act
  timeline.add(10n, 0n, encode([ALICE]), false)
  timeline.add(10n, 1n, encode([BOB]), false)

  // Assert
  expect(timeline.records().map((record) => record.speaker)).toEqual(['alice', 'bob'])
})

test('a live object that arrives before the fetched first object waits for it', () => {
  // Arrange
  const timeline = new TranscriptTimeline()

  // Act
  const early = timeline.add(10n, 1n, encode([BOB]), false)
  timeline.add(10n, 0n, encode([ALICE]), false)

  // Assert
  expect(early).toBe(false)
  expect(timeline.records().map((record) => record.speaker)).toEqual(['alice', 'bob'])
})

test('a live object stays live when the fetched history it waited for arrives after it', () => {
  // Arrange
  const timeline = new TranscriptTimeline()

  // Act
  timeline.add(10n, 1n, encode([BOB]), true)
  timeline.add(10n, 0n, encode([ALICE]), false)

  // Assert
  expect(timeline.records().map(({ speaker, live }) => [speaker, live])).toEqual([
    ['alice', false],
    ['bob', true]
  ])
})

test('a new group replaces the history and an older one is ignored', () => {
  // Arrange
  const timeline = new TranscriptTimeline()
  timeline.add(10n, 0n, encode([ALICE]), false)
  timeline.add(10n, 1n, encode([BOB]), false)

  // Act
  timeline.add(11n, 0n, encode([ALICE, BOB, CAROL]), false)
  const stale = timeline.add(10n, 2n, encode([CAROL]), false)

  // Assert
  expect(stale).toBe(false)
  expect(timeline.records()).toEqual([
    { startedAtMs: 1_000, speaker: 'alice', text: 'こんにちは', live: false },
    { startedAtMs: 2_000, speaker: 'bob', text: '今日は良い天気ですね', live: false },
    { startedAtMs: 3_000, speaker: 'carol', text: 'よろしくお願いします', live: false }
  ])
})

test('the transcriber status names its recognizer and state, and anything else reads as none', () => {
  // Arrange
  const encodeJson = (value: unknown) => new TextEncoder().encode(JSON.stringify(value))

  // Act / Assert
  expect(parseTranscriberStatus(encodeJson({ asr: 'qwen3-asr', state: 'starting' }))).toEqual({
    asr: 'qwen3-asr',
    state: 'starting'
  })
  expect(parseTranscriberStatus(encodeJson({ asr: 'qwen3-asr', state: 'loading' }))).toBeUndefined()
})

test("a speaker's records that arrive within a second of each other all stay in the caption", () => {
  // Arrange
  const fragments = ['音声を', '聞いてみたいと', '思います'].map((text, i) => ({
    startedAtMs: 54_000 + i * 600,
    speaker: 'alice',
    text,
    live: true
  }))
  const arrivals = new Map(fragments.map((record, i) => [transcriptRecordKey(record), 60_000 + i * 600]))

  // Act
  const captions = freshCaptions(fragments, arrivals, 61_500)

  // Assert
  expect(captions.get('alice')).toBe('音声を 聞いてみたいと 思います')
})

test('a caption keeps the end of a long run of records', () => {
  // Arrange
  const records = Array.from({ length: 10 }, (_, i) => ({
    startedAtMs: i * 1_000,
    speaker: 'bob',
    text: `${i}`.repeat(9),
    live: true
  }))
  const arrivals = new Map(records.map((record) => [transcriptRecordKey(record), 10_000]))

  // Act
  const caption = freshCaptions(records, arrivals, 10_000).get('bob')

  // Assert
  expect(caption).toHaveLength(60)
  expect(caption?.endsWith(' 999999999')).toBe(true)
})
