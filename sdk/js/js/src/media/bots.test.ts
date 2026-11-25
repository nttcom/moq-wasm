import { expect, test } from '@playwright/test'
import { botNamespace, botTypeOf } from './bots'

const ROOM = ['app', 'room']

test('a bot publishes under the bots of its room', () => {
  // Act / Assert
  expect(botNamespace(ROOM, 'transcriber')).toEqual(['app', 'room', 'bots', 'transcriber'])
})

test('botTypeOf names the type of a bot namespace of the room', () => {
  // Act / Assert
  expect(botTypeOf(ROOM, botNamespace(ROOM, 'transcriber'))).toBe('transcriber')
})

test('botTypeOf reads a member, a namespace deeper under bots and another room as no bot', () => {
  // Act / Assert
  expect(botTypeOf(ROOM, ['app', 'room', 'alice'])).toBeUndefined()
  expect(botTypeOf(ROOM, ['app', 'room', 'bots'])).toBeUndefined()
  expect(botTypeOf(ROOM, ['app', 'room', 'bots', 'transcriber', 'extra'])).toBeUndefined()
  expect(botTypeOf(ROOM, ['app', 'other', 'bots', 'transcriber'])).toBeUndefined()
  expect(botTypeOf(ROOM, ['app', 'room', 'alice', 'transcriber'])).toBeUndefined()
})
