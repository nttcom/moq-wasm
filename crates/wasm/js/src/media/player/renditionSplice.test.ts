import { expect, test } from '@playwright/test'
import { RenditionSplice } from './renditionSplice'

test('the incoming rendition starts after the group the outgoing one already started', () => {
  // Arrange
  const splice = new RenditionSplice(10n)

  // Act
  const verdict = splice.acceptIncoming(10n, 0n)

  // Assert
  expect(verdict).toEqual({ play: false, start: { groupId: 11n, aligned: true }, retireOutgoing: false })
  expect(splice.acceptOutgoing(10n)).toBe('play')
  expect(splice.acceptIncoming(11n, 0n)).toEqual({ play: true, retireOutgoing: false })
})

test('an incoming group the outgoing rendition has not reached takes over from its first object', () => {
  // Arrange
  const splice = new RenditionSplice(10n)

  // Act
  const verdict = splice.acceptIncoming(11n, 0n)

  // Assert: the outgoing rendition retires once it reaches the same group
  expect(verdict).toEqual({ play: true, start: { groupId: 11n, aligned: true }, retireOutgoing: false })
  expect(splice.acceptOutgoing(10n)).toBe('play')
  expect(splice.acceptOutgoing(11n)).toBe('retire')
})

test('the outgoing rendition keeps playing groups it starts before the incoming one delivers', () => {
  // Arrange
  const splice = new RenditionSplice(10n)
  splice.acceptOutgoing(11n)

  // Act
  const verdict = splice.acceptIncoming(11n, 0n)

  // Assert
  expect(verdict.start).toEqual({ groupId: 12n, aligned: true })
  expect(verdict.play).toBe(false)
})

test('the incoming rendition waits for the start of a group', () => {
  // Arrange
  const splice = new RenditionSplice(10n)

  // Act
  const verdict = splice.acceptIncoming(11n, 3n)

  // Assert
  expect(verdict).toEqual({ play: false, retireOutgoing: false })
  expect(splice.start).toBeUndefined()
})

test('the outgoing rendition retires once the incoming one plays past the start group', () => {
  // Arrange
  const splice = new RenditionSplice(10n)
  splice.acceptIncoming(11n, 0n)

  // Act
  const verdict = splice.acceptIncoming(12n, 0n)

  // Assert
  expect(verdict).toEqual({ play: true, retireOutgoing: true })
})

test('group ids that do not continue the outgoing rendition cut over at once', () => {
  // Arrange
  const splice = new RenditionSplice(1_000n)

  // Act
  const verdict = splice.acceptIncoming(5n, 0n)

  // Assert
  expect(verdict).toEqual({ play: true, start: { groupId: 5n, aligned: false }, retireOutgoing: true })
})

test('without an outgoing group the incoming rendition cuts over at its first group', () => {
  // Arrange
  const splice = new RenditionSplice(undefined)

  // Act
  const verdict = splice.acceptIncoming(7n, 0n)

  // Assert
  expect(verdict).toEqual({ play: true, start: { groupId: 7n, aligned: false }, retireOutgoing: true })
})
