import { expect, test } from '@playwright/test'
import { captionAt, eventRecords, mergeEvents } from './timeline'

const encode = (value: unknown) => new TextEncoder().encode(JSON.stringify(value))

test('a chat record is the participant whose broadcast carries it and a transcript record names its speaker', () => {
  // Arrange
  const chat = encode([{ m: 100, data: { text: 'hello' } }])
  const transcript = encode([{ m: 200, data: { speaker: 'bob', text: 'hi' } }])

  // Act
  const events = [...eventRecords(chat, 'chat', 'alice'), ...eventRecords(transcript, 'transcript', 'alice')]

  // Assert
  expect(events).toEqual([
    { at: 100_000, kind: 'chat', speaker: 'alice', text: 'hello' },
    { at: 200_000, kind: 'transcript', speaker: 'bob', text: 'hi' }
  ])
  expect(() => eventRecords(encode([{ data: { text: 'no clock' } }]), 'chat', 'alice')).toThrow()
})

test('repeated records merge and a later transcript correction keeps the utterance time', () => {
  // Arrange
  const old = eventRecords(encode([{ m: 100, data: { speaker: 'alice', text: 'old' } }]), 'transcript', 'alice')
  const corrected = eventRecords(
    encode([{ m: 100, data: { speaker: 'alice', text: 'corrected' } }]),
    'transcript',
    'alice'
  )
  const chat = eventRecords(encode([{ m: 50, data: { text: 'first' } }]), 'chat', 'alice')

  // Act
  const events = mergeEvents([...old, ...corrected, ...corrected, ...chat, ...chat])

  // Assert
  expect(events.map(({ kind, at, text }) => [kind, at, text])).toEqual([
    ['chat', 50_000, 'first'],
    ['transcript', 100_000, 'corrected']
  ])
})

test('a caption shows its speaker utterance for four seconds from its time', () => {
  // Arrange
  const events = mergeEvents([
    ...eventRecords(encode([{ m: 1_000, data: { speaker: 'alice', text: 'hi' } }]), 'transcript', 'alice'),
    ...eventRecords(encode([{ m: 2_000, data: { speaker: 'bob', text: 'hello' } }]), 'transcript', 'bob')
  ])

  // Act / Assert
  expect(captionAt(events, 'alice', 999_000)).toBeUndefined()
  expect(captionAt(events, 'alice', 4_999_000)?.text).toBe('hi')
  expect(captionAt(events, 'alice', 5_000_000)).toBeUndefined()
  expect(captionAt(events, 'bob', 2_000_000)?.text).toBe('hello')
})
