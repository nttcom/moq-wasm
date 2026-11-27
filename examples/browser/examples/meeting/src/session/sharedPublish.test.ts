import { expect, test } from '@playwright/test'
import { SharedPublish } from './sharedPublish'

test('concurrent callers share one PUBLISH', async () => {
  // Arrange
  const shared = new SharedPublish<string>()
  let publishes = 0
  const publish = async () => `writer ${++publishes}`

  // Act
  const writers = await Promise.all([shared.writer(publish), shared.writer(publish)])

  // Assert
  expect(writers).toEqual(['writer 1', 'writer 1'])
})

test('a failed PUBLISH is retried by the next caller', async () => {
  // Arrange
  const shared = new SharedPublish<string>()
  await expect(shared.writer(() => Promise.reject(new Error('rejected')))).rejects.toThrow('rejected')

  // Act
  const writer = await shared.writer(async () => 'writer')

  // Assert
  expect(writer).toBe('writer')
})

test('a failure of a reset PUBLISH keeps the newer one', async () => {
  // Arrange
  const shared = new SharedPublish<string>()
  let reject: (error: Error) => void = () => {}
  const stale = shared.writer(() => new Promise((_, rejectStale) => (reject = rejectStale)))
  shared.reset()
  const current = shared.writer(async () => 'current')

  // Act
  reject(new Error('stale session closed'))
  await expect(stale).rejects.toThrow('stale session closed')

  // Assert
  expect(await shared.writer(() => Promise.reject(new Error('published again')))).toBe('current')
  expect(await current).toBe('current')
})
