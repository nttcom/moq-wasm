import { expect, test } from '@playwright/test'
import { init } from './init'

test('a second call returns the promise of the first', async () => {
  // Arrange: the unit tests run outside a browser, where the wasm module fails to load
  const first = init({ onLog: () => {} })

  // Act
  const second = init({ onLog: () => {} })

  // Assert
  expect(second).toBe(first)
  await expect(first).rejects.toThrow()
})

test('a call after the first one rejected starts again', async () => {
  // Arrange
  const first = init()
  await first.catch(() => {})

  // Act
  const second = init()

  // Assert
  expect(second).not.toBe(first)
  await expect(second).rejects.toThrow()
})
