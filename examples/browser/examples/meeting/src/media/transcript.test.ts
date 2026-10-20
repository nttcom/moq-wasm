import { expect, test } from '@playwright/test'
import { TranscriptTimeline, parseTranscriberStatus } from './transcript'

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
