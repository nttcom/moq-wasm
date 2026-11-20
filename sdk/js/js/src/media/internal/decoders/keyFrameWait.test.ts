import { expect, test } from '@playwright/test'
import { KeyFrameWait } from './keyFrameWait'

test('deltas dropped while joining do not ask for a new group', () => {
  // Arrange
  const wait = new KeyFrameWait()

  // Act
  const asked = wait.deltaDropped()

  // Assert
  expect(wait.waiting).toBe(true)
  expect(asked).toBe(false)
})

test('a cut group asks for a new group once, at its first dropped delta', () => {
  // Arrange
  const wait = new KeyFrameWait()
  wait.keyFrameDecoded()
  wait.cut()

  // Act
  const asked = [wait.deltaDropped(), wait.deltaDropped(), wait.deltaDropped()]

  // Assert
  expect(wait.waiting).toBe(true)
  expect(asked).toEqual([true, false, false])
})

test('a keyframe ends the wait, and the next cut asks again', () => {
  // Arrange
  const wait = new KeyFrameWait()
  wait.keyFrameDecoded()
  wait.cut()
  wait.deltaDropped()

  // Act
  wait.keyFrameDecoded()
  const waitingAfterKeyFrame = wait.waiting
  wait.cut()
  const asked = wait.deltaDropped()

  // Assert
  expect(waitingAfterKeyFrame).toBe(false)
  expect(asked).toBe(true)
})
