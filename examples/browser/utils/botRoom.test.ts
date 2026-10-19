import { expect, test } from '@playwright/test'
import { roomIdFromSearch } from './botRoom'

test('a room ID is taken from the query only when it is one namespace element', () => {
  // Act / Assert
  expect(roomIdFromSearch('?room=team-1')).toBe('team-1')
  expect(roomIdFromSearch('?room=Team')).toBeUndefined()
  expect(roomIdFromSearch('?room=a/b')).toBeUndefined()
  expect(roomIdFromSearch(`?room=${'a'.repeat(33)}`)).toBeUndefined()
  expect(roomIdFromSearch('')).toBeUndefined()
})
