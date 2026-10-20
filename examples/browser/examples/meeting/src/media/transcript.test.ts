import { expect, test } from '@playwright/test'
import { TranscriptTimeline } from './transcript'

const encode = (records: { t: number; speaker: string; text: string }[]) =>
  new TextEncoder().encode(JSON.stringify(records.map(({ t, speaker, text }) => ({ t, data: { speaker, text } }))))
const ALICE = { t: 1_000, speaker: 'alice', text: 'こんにちは' }
const BOB = { t: 2_000, speaker: 'bob', text: '今日は良い天気ですね' }
const CAROL = { t: 3_000, speaker: 'carol', text: 'よろしくお願いします' }

test('the first object of a group holds the history and later objects add to it', () => {
  // Arrange
  const timeline = new TranscriptTimeline()

  // Act
  timeline.add(10n, 0n, encode([ALICE]))
  timeline.add(10n, 1n, encode([BOB]))

  // Assert
  expect(timeline.records().map((record) => record.speaker)).toEqual(['alice', 'bob'])
})

test('a live object that arrives before the fetched first object waits for it', () => {
  // Arrange
  const timeline = new TranscriptTimeline()

  // Act
  const early = timeline.add(10n, 1n, encode([BOB]))
  timeline.add(10n, 0n, encode([ALICE]))

  // Assert
  expect(early).toBe(false)
  expect(timeline.records().map((record) => record.speaker)).toEqual(['alice', 'bob'])
})

test('a new group replaces the history and an older one is ignored', () => {
  // Arrange
  const timeline = new TranscriptTimeline()
  timeline.add(10n, 0n, encode([ALICE]))
  timeline.add(10n, 1n, encode([BOB]))

  // Act
  timeline.add(11n, 0n, encode([ALICE, BOB, CAROL]))
  const stale = timeline.add(10n, 2n, encode([CAROL]))

  // Assert
  expect(stale).toBe(false)
  expect(timeline.records()).toEqual([
    { startedAtMs: 1_000, speaker: 'alice', text: 'こんにちは' },
    { startedAtMs: 2_000, speaker: 'bob', text: '今日は良い天気ですね' },
    { startedAtMs: 3_000, speaker: 'carol', text: 'よろしくお願いします' }
  ])
})
