import { expect, test } from '@playwright/test'
import { type LogRecord, init } from './init'

test('a second call returns the promise of the first', async () => {
  // Arrange: the unit tests run outside a browser, where the wasm module fails to load
  const first = init({ onLog: () => {} })

  // Act
  const second = init({ onLog: () => {} })

  // Assert
  expect(second).toBe(first)
  await expect(first).rejects.toThrow()
})

test('a wasm module that fails to load is logged to onLog', async () => {
  // Arrange: the unit tests run outside a browser, where the wasm module fails to load
  const logged: LogRecord[] = []

  // Act
  await init({ onLog: (record) => logged.push(record) }).catch(() => {})

  // Assert: the sink is installed before the load, so the failure reaches it
  expect(logged).toHaveLength(1)
  expect(logged[0]).toMatchObject({
    level: 'error',
    target: 'moq-wasm::core::init',
    message: 'wasm module failed to load'
  })
  expect(logged[0].fields.error).toEqual(expect.any(String))
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
