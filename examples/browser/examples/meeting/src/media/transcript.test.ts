import { expect, test } from '@playwright/test'
import { freshCaptions, parseTranscriberStatus, transcriptRecordKey } from './transcript'

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
