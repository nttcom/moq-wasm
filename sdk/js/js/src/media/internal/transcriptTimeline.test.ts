import { expect, test } from '@playwright/test'
import { type TranscriptRecord, TranscriptTimeline } from './transcriptTimeline'

const encode = (records: { t: number; speaker: string; text: string }[]) =>
  new TextEncoder().encode(JSON.stringify(records.map(({ t, speaker, text }) => ({ T: t, data: { speaker, text } }))))
const ALICE = { t: 1_000, speaker: 'alice', text: 'こんにちは' }
const BOB = { t: 2_000, speaker: 'bob', text: '今日は良い天気ですね' }
const CAROL = { t: 3_000, speaker: 'carol', text: 'よろしくお願いします' }
const DAVE = { t: 4_000, speaker: 'dave', text: 'はじめまして' }
const text = (value: string) => new TextEncoder().encode(value)

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

test('an object delivered twice changes nothing the second time', () => {
  // Arrange
  const timeline = new TranscriptTimeline()
  timeline.add(10n, 0n, encode([ALICE]), false)

  // Act
  const again = timeline.add(10n, 0n, encode([BOB]), true)

  // Assert
  expect(again).toBe(false)
  expect(timeline.records().map((record) => record.speaker)).toEqual(['alice'])
})

test('an object that is not JSON, a JSON value that is not an array and a malformed record are skipped', () => {
  // Arrange
  const timeline = new TranscriptTimeline()

  // Act
  timeline.add(10n, 0n, encode([ALICE]), false)
  timeline.add(10n, 1n, text('{not json'), true)
  timeline.add(10n, 2n, text('{"T":1}'), true)
  timeline.add(
    10n,
    3n,
    text('[null, 5, {"T":"x","data":{"speaker":"bob","text":"hi"}}, {"T":4000,"data":{"speaker":"bob"}}]'),
    true
  )
  timeline.add(10n, 4n, encode([CAROL]), true)

  // Assert
  expect(timeline.records().map((record) => record.speaker)).toEqual(['alice', 'carol'])
})

test('an opening object that is not a JSON array leaves the current group as it was', () => {
  // Arrange
  const timeline = new TranscriptTimeline()
  timeline.add(10n, 0n, encode([ALICE]), false)
  timeline.add(10n, 1n, encode([BOB]), false)

  // Act
  const notJson = timeline.add(11n, 0n, text('{not json'), false)
  const notArray = timeline.add(12n, 0n, text('{"T":1}'), false)

  // Assert
  expect([notJson, notArray]).toEqual([false, false])
  expect(timeline.records().map((record) => record.speaker)).toEqual(['alice', 'bob'])
})

test('a later object that is not a JSON array changes nothing', () => {
  // Arrange
  const timeline = new TranscriptTimeline()
  timeline.add(10n, 0n, encode([ALICE]), false)

  // Act
  const changed = timeline.add(10n, 1n, text('"hello"'), true)

  // Assert
  expect(changed).toBe(false)
  expect(timeline.records().map((record) => record.speaker)).toEqual(['alice'])
})

test('an empty opening object opens its group with no records', () => {
  // Arrange
  const timeline = new TranscriptTimeline()
  timeline.add(10n, 0n, encode([ALICE]), false)

  // Act
  const changed = timeline.add(11n, 0n, encode([]), true)

  // Assert
  expect(changed).toBe(true)
  expect(timeline.records()).toEqual([])
})

type Delivery = { groupId: bigint; objectId: bigint; records: (typeof ALICE)[] }

/// Every order of `items`, each delivery of an object counted once.
function* orders<T>(items: T[]): Generator<T[]> {
  if (items.length === 0) {
    yield []
    return
  }
  const seen = new Set<T>()
  for (const [index, item] of items.entries()) {
    if (seen.has(item)) {
      continue
    }
    seen.add(item)
    const rest = [...items.slice(0, index), ...items.slice(index + 1)]
    for (const order of orders(rest)) {
      yield [item, ...order]
    }
  }
}

/// The delivered objects of the newest group whose object 0 was delivered, in
/// object-id order, each with the `live` of its first delivery.
function model(delivered: Map<Delivery, boolean>): TranscriptRecord[] {
  const opened = [...delivered.keys()].filter(({ objectId }) => objectId === 0n).map(({ groupId }) => groupId)
  if (opened.length === 0) {
    return []
  }
  const current = opened.reduce((newest, groupId) => (groupId > newest ? groupId : newest))
  return [...delivered.entries()]
    .filter(([{ groupId }]) => groupId === current)
    .sort(([a], [b]) => Number(a.objectId - b.objectId))
    .flatMap(([{ records }, live]) => records.map(({ t, speaker, text }) => ({ startedAtMs: t, speaker, text, live })))
}

test('in every delivery order, duplicates included, the records are those of the model', () => {
  // Arrange: each object is delivered twice, by the FETCH (not live) first and the SUBSCRIBE (live) next
  const objects: Delivery[] = [
    { groupId: 1n, objectId: 0n, records: [ALICE] },
    { groupId: 1n, objectId: 1n, records: [BOB] },
    { groupId: 2n, objectId: 0n, records: [ALICE, BOB, CAROL] },
    { groupId: 2n, objectId: 1n, records: [DAVE] }
  ]

  const mismatches: string[] = []

  // Act
  for (const order of orders([...objects, ...objects])) {
    const timeline = new TranscriptTimeline()
    const delivered = new Map<Delivery, boolean>()
    for (const [step, object] of order.entries()) {
      const before = JSON.stringify(model(delivered))
      const live = delivered.has(object)
      if (!live) {
        delivered.set(object, live)
      }
      const changed = timeline.add(object.groupId, object.objectId, encode(object.records), live)
      const expected = JSON.stringify(model(delivered))
      if (JSON.stringify(timeline.records()) !== expected || changed !== (before !== expected)) {
        mismatches.push(
          order
            .slice(0, step + 1)
            .map(({ groupId, objectId }) => `(${groupId},${objectId})`)
            .join(' ')
        )
        break
      }
    }
  }

  // Assert: the delivery orders after which the records or the reported change differ from the model
  expect(mismatches.slice(0, 5)).toEqual([])
})
