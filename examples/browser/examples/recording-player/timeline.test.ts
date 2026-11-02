import { expect, test } from '@playwright/test'
import { eventRecords, isKeyframe, keyframeDetector, mergeEvents, chunkIndexAt, type IndexedChunk } from './timeline'

test('a long recording selects the seek chunk without retaining preceding payloads', () => {
  // Arrange
  const chunks: IndexedChunk[] = Array.from({ length: 1000 }, (_, index) => ({
    first: { group: String(index), object: '0' },
    last: { group: String(index), object: '99' },
    capture_start_us: String(index * 10_000_000),
    capture_end_us: String((index + 1) * 10_000_000 - 1),
    data_size: String(4 * 1024 * 1024)
  }))
  // Act / Assert
  expect(chunkIndexAt(chunks, 9_995_000_000)).toBe(999)
  expect(chunkIndexAt(chunks, -1)).toBe(0)
  expect(chunkIndexAt(chunks, 50_000_000)).toBe(5)
})

test('transcript snapshots deduplicate and late corrections retain utterance time', () => {
  // Arrange
  const encode = (value: unknown) => new TextEncoder().encode(JSON.stringify(value))
  const old = eventRecords(
    encode([{ t: 100, data: { speaker: 'alice', text: 'old' } }]),
    'anon/r/bots/transcriber',
    'transcript'
  )
  const corrected = eventRecords(
    encode([{ t: 100, data: { speaker: 'alice', text: 'corrected' } }]),
    'anon/r/bots/transcriber',
    'transcript'
  )
  // Act
  const events = mergeEvents([...old, ...corrected, ...corrected])
  // Assert
  expect(events).toHaveLength(1)
  expect(events[0]).toMatchObject({ at: 100_000, text: 'corrected', speaker: 'alice' })
  expect(() => eventRecords(encode([{ data: { text: 'no clock' } }]), 'alice', 'chat')).toThrow()
})

test('access points come from encoded bytes rather than object numbering', () => {
  // Arrange
  const h264Key = new Uint8Array([0, 0, 0, 1, 0x65, 0x88])
  const h264Delta = new Uint8Array([0, 0, 1, 0x41, 0x88])
  // Act / Assert
  expect(isKeyframe('avc1.42C01E', h264Key)).toBe(true)
  expect(isKeyframe('avc1.42C01E', h264Delta)).toBe(false)
  expect(isKeyframe('vp8', new Uint8Array([0, 0, 0, 0x9d, 1, 0x2a]))).toBe(true)
  expect(isKeyframe('vp8', new Uint8Array([1, 0, 0, 0x9d, 1, 0x2a]))).toBe(false)
  const av1 = keyframeDetector('av01.0.05M.08')
  expect(av1(new Uint8Array([0x0a, 1, 0, 0x32, 1, 0]))).toBe(true)
  expect(av1(new Uint8Array([0x32, 1, 0x20]))).toBe(false)
  expect(() => av1(new Uint8Array([0x32, 4, 0]))).toThrow('Truncated')
})
