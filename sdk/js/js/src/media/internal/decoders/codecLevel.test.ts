import { expect, test } from '@playwright/test'
import { differsOnlyInLevel } from './codecLevel'

test('codec strings that differ only in their level are the same codec', () => {
  // Act / Assert
  expect(differsOnlyInLevel('avc1.64001E', 'avc1.64000B')).toBe(true)
  expect(differsOnlyInLevel('vp09.00.10.08', 'vp09.00.31.08')).toBe(true)
  expect(differsOnlyInLevel('av01.0.04M.08', 'av01.0.08M.08')).toBe(true)
  expect(differsOnlyInLevel('hvc1.1.6.L93.B0', 'hvc1.1.6.L120.B0')).toBe(true)
})

test('codec strings of another codec or profile differ in more than their level', () => {
  // Act / Assert
  expect(differsOnlyInLevel('avc1.64001E', 'avc1.4D401E')).toBe(false)
  expect(differsOnlyInLevel('avc1.64001E', 'av01.0.04M.08')).toBe(false)
  expect(differsOnlyInLevel('vp09.00.10.08', 'vp09.02.10.10')).toBe(false)
  expect(differsOnlyInLevel('av01.0.04M.08', 'av01.1.04M.08')).toBe(false)
  expect(differsOnlyInLevel('hvc1.1.6.L93.B0', 'hvc1.2.4.L93.B0')).toBe(false)
  expect(differsOnlyInLevel('hvc1.1.6.L93.B0', 'hvc1.1.6.H93.B0')).toBe(false)
  expect(differsOnlyInLevel('vp8', 'vp8.0')).toBe(false)
})
