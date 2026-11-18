import { expect, test } from '@playwright/test'
import { transcriptTurns } from './transcript'

function payload(value: unknown): Uint8Array {
  return new TextEncoder().encode(JSON.stringify(value))
}

test('each record of an object is a turn of its role', () => {
  // Arrange
  const object = payload([
    { T: 1, data: { role: 'user', text: 'こんにちは' } },
    { T: 2, data: { role: 'assistant', text: 'こんにちは。何を話しましょうか' } }
  ])

  // Act
  const turns = transcriptTurns(object)

  // Assert
  expect(turns).toEqual([
    { role: 'user', text: 'こんにちは' },
    { role: 'assistant', text: 'こんにちは。何を話しましょうか' }
  ])
})

test('a record of an answer keeps its turn, so the next record of the answer replaces its text', () => {
  // Arrange
  const object = payload([{ T: 2, data: { role: 'assistant', text: 'こんにちは。', turn: 1_700_000_000_000 } }])

  // Act
  const turns = transcriptTurns(object)

  // Assert
  expect(turns).toEqual([{ role: 'assistant', text: 'こんにちは。', turn: 1_700_000_000_000 }])
})

test('the empty record list that opens the timeline and malformed records give no turn', () => {
  // Arrange
  const objects = [
    payload([]),
    payload([{ T: 1, data: { role: 'system', text: 'x' } }, { T: 2, data: { role: 'user', text: '' } }, null]),
    payload({ data: { role: 'user', text: 'not a list' } }),
    new TextEncoder().encode('{')
  ]

  // Act
  const turns = objects.flatMap(transcriptTurns)

  // Assert
  expect(turns).toEqual([])
})
