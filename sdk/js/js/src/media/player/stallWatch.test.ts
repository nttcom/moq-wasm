import { expect, test } from '@playwright/test'
import { stallTimeoutMs } from './stallWatch'

test('a track at 4 fps or more, or without a catalog frame rate, stalls after 500 ms', () => {
  // Act / Assert
  expect(stallTimeoutMs(30)).toBe(500)
  expect(stallTimeoutMs(4)).toBe(500)
  expect(stallTimeoutMs(undefined)).toBe(500)
})

test('a 2 fps track stalls only after two frame intervals', () => {
  // Act / Assert
  expect(stallTimeoutMs(2)).toBe(1_000)
})
