import { expect, test } from '@playwright/test'
import { ReviewGroupStore } from './reviewGroupStore'

const MEBIBYTE = 1024 * 1024

/// A store of results that are their own size in bytes, and a `fetch` that
/// counts its calls and resolves with `bytes`.
function countingStore() {
  const store = new ReviewGroupStore<number>((bytes) => bytes)
  let fetches = 0
  const fetch = (bytes: number) => async () => {
    fetches += 1
    return bytes
  }
  return { store, fetch, fetches: () => fetches }
}

test('a group is fetched once and later requests for it share the result', async () => {
  // Arrange
  const { store, fetch, fetches } = countingStore()

  // Act
  const results = await Promise.all([store.fetched('video', 7n, fetch(10)), store.fetched('video', 7n, fetch(10))])
  const later = await store.fetched('video', 7n, fetch(10))

  // Assert
  expect([...results, later]).toEqual([10, 10, 10])
  expect(fetches()).toBe(1)
})

test('a result without bytes is not kept, so the group is fetched again', async () => {
  // Arrange
  const { store, fetch, fetches } = countingStore()
  await store.fetched('video', 7n, fetch(0))

  // Act
  const result = await store.fetched('video', 7n, fetch(10))

  // Assert
  expect(result).toBe(10)
  expect(fetches()).toBe(2)
})

test('another track empties the store and ends the fetches under way', async () => {
  // Arrange: a fetch for `video` that reports whether it is still wanted
  const { store, fetch, fetches } = countingStore()
  let wanted: () => boolean = () => true
  void store.fetched('video', 7n, async (isCurrent) => {
    wanted = isCurrent
    return 10
  })

  // Act
  await store.fetched('video-low', 7n, fetch(10))
  await store.fetched('video', 7n, fetch(10))

  // Assert
  expect(wanted()).toBe(false)
  expect(fetches()).toBe(2)
})

test('the oldest groups leave once the held payloads pass the limit', async () => {
  // Arrange: two groups of 40 MiB against a limit of 64 MiB
  const { store, fetch, fetches } = countingStore()
  await store.fetched('video', 1n, fetch(40 * MEBIBYTE))
  await store.fetched('video', 2n, fetch(40 * MEBIBYTE))

  // Act
  await store.fetched('video', 2n, fetch(40 * MEBIBYTE))
  await store.fetched('video', 1n, fetch(40 * MEBIBYTE))

  // Assert: group 2 was still held and group 1 was fetched again
  expect(fetches()).toBe(3)
})
