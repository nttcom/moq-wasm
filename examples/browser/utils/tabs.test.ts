import { expect, test } from '@playwright/test'
import { nextTabIndex } from './tabs'

test('the arrow keys wrap around the tabs, and Home and End go to the first and last', () => {
  // Act / Assert
  expect(nextTabIndex('ArrowRight', 0, 3)).toBe(1)
  expect(nextTabIndex('ArrowRight', 2, 3)).toBe(0)
  expect(nextTabIndex('ArrowLeft', 0, 3)).toBe(2)
  expect(nextTabIndex('Home', 2, 3)).toBe(0)
  expect(nextTabIndex('End', 0, 3)).toBe(2)
})

test('a key tabs do not take moves to no tab', () => {
  // Act / Assert
  expect(nextTabIndex('ArrowDown', 0, 3)).toBeUndefined()
  expect(nextTabIndex('Enter', 0, 3)).toBeUndefined()
})
