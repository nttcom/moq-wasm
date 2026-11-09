import { expect, test } from '@playwright/test'
import { TrackPublication } from './trackPublication'

test('concurrent callers share one PUBLISH', async () => {
  // Arrange
  const publication = new TrackPublication<string>()
  let publishes = 0
  const publish = async () => `writer ${++publishes}`

  // Act
  const writers = await Promise.all([publication.writer(publish), publication.writer(publish)])

  // Assert
  expect(writers).toEqual(['writer 1', 'writer 1'])
})

test('a failed PUBLISH is retried by the next caller', async () => {
  // Arrange
  const publication = new TrackPublication<string>()
  await expect(publication.writer(() => Promise.reject(new Error('rejected')))).rejects.toThrow('rejected')

  // Act
  const writer = await publication.writer(async () => 'writer')

  // Assert
  expect(writer).toBe('writer')
})

test('a failure of a reset PUBLISH keeps the newer one', async () => {
  // Arrange
  const publication = new TrackPublication<string>()
  let reject: (error: Error) => void = () => {}
  const stale = publication.writer(() => new Promise((_, rejectStale) => (reject = rejectStale)))
  publication.reset()
  const current = publication.writer(async () => 'current')

  // Act
  reject(new Error('stale session closed'))
  await expect(stale).rejects.toThrow('stale session closed')

  // Assert
  expect(await publication.writer(() => Promise.reject(new Error('published again')))).toBe('current')
  expect(await current).toBe('current')
})
