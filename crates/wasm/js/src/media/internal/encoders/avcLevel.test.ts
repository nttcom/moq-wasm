import { expect, test } from '@playwright/test'
import { avcCodecForSize } from './avcLevel'

test('a 1920x1200 screen raises the default level 4.0 to 5.0', () => {
  // Act / Assert: 9000 macroblocks (120x75) exceed the 8704 of level 4.2
  expect(avcCodecForSize('avc1.640028', { width: 1920, height: 1200 }, 30)).toBe('avc1.640032')
})

test('a size the requested level holds keeps the codec', () => {
  // Act / Assert
  expect(avcCodecForSize('avc1.640028', { width: 1920, height: 1080 }, 30)).toBe('avc1.640028')
})

test('a high frame rate raises the level by the macroblocks a second', () => {
  // Act / Assert: 8160 macroblocks at 60 fps exceed the 245760 a second of level 4.1
  expect(avcCodecForSize('avc1.640028', { width: 1920, height: 1080 }, 60)).toBe('avc1.64002a')
})

test('a 4K screen raises the level to 5.1 and keeps the profile', () => {
  // Act / Assert
  expect(avcCodecForSize('avc1.42e01f', { width: 3840, height: 2160 }, 30)).toBe('avc1.42e033')
})

test('a level higher than the size needs is not lowered', () => {
  // Act / Assert
  expect(avcCodecForSize('avc1.640033', { width: 640, height: 360 }, 30)).toBe('avc1.640033')
})

test('a codec other than H.264 is left as it is', () => {
  // Act / Assert
  expect(avcCodecForSize('av01.0.08M.08', { width: 3840, height: 2160 }, 30)).toBe('av01.0.08M.08')
})
