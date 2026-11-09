import { expect, test } from '@playwright/test'
import { sameCodecIgnoringLevel } from './codecLevel'

test('renditions that differ only in their level are one codec', () => {
  // Act / Assert
  expect(sameCodecIgnoringLevel('avc1.42D00B', 'avc1.42C00A')).toBe(true)
  expect(sameCodecIgnoringLevel('vp09.00.31.08', 'vp09.00.21.08')).toBe(true)
  expect(sameCodecIgnoringLevel('av01.0.08M.08', 'av01.0.04M.08')).toBe(true)
})

test('another profile, bit depth or codec is not the same codec', () => {
  // Act / Assert
  expect(sameCodecIgnoringLevel('avc1.42C01F', 'avc1.64001F')).toBe(false)
  expect(sameCodecIgnoringLevel('vp09.00.31.08', 'vp09.00.31.10')).toBe(false)
  expect(sameCodecIgnoringLevel('av01.0.08M.08', 'av01.0.08H.08')).toBe(false)
  expect(sameCodecIgnoringLevel('avc1.42C01F', 'vp8')).toBe(false)
  expect(sameCodecIgnoringLevel('avc1.42C01F', undefined)).toBe(false)
})
