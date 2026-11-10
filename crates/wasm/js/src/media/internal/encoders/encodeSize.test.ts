import { expect, test } from '@playwright/test'
import { fitEncodeSize } from './encodeSize'

test('a 16:10 screen keeps its aspect ratio within a 16:9 request', () => {
  // Arrange
  const requested = { width: 1920, height: 1080 }
  const screen = { width: 3024, height: 1890 }

  // Act
  const size = fitEncodeSize(requested, screen)

  // Assert
  expect(size).toEqual({ width: 1728, height: 1080 })
})

test('a frame that already matches the request is encoded at the request', () => {
  // Act / Assert
  expect(fitEncodeSize({ width: 1280, height: 720 }, { width: 1280, height: 720 })).toEqual({
    width: 1280,
    height: 720
  })
})

test('a frame smaller than the request is encoded at its own size', () => {
  // Act / Assert
  expect(fitEncodeSize({ width: 1920, height: 1080 }, { width: 1280, height: 720 })).toEqual({
    width: 1280,
    height: 720
  })
})

test('an odd scaled size is rounded down to even', () => {
  // Arrange: a 1000x999 window scaled by 0.5 is 500x499.5
  const requested = { width: 500, height: 1000 }
  const window = { width: 1000, height: 999 }

  // Act
  const size = fitEncodeSize(requested, window)

  // Assert
  expect(size).toEqual({ width: 500, height: 498 })
})

test('a frame is encoded at its own size when no size is requested', () => {
  // Act / Assert
  expect(fitEncodeSize({}, { width: 2560, height: 1440 })).toEqual({ width: 2560, height: 1440 })
})

test('a requested height alone bounds the size', () => {
  // Act / Assert
  expect(fitEncodeSize({ height: 720 }, { width: 2560, height: 1440 })).toEqual({ width: 1280, height: 720 })
})

test('a scale divides the frame size, whatever its aspect ratio', () => {
  // Act / Assert
  expect(fitEncodeSize({ scaleResolutionDownBy: 4 }, { width: 1280, height: 720 })).toEqual({ width: 320, height: 180 })
  expect(fitEncodeSize({ scaleResolutionDownBy: 4 }, { width: 3024, height: 1890 })).toEqual({
    width: 756,
    height: 472
  })
})

test('a scale below 1 does not scale the frame up', () => {
  // Act / Assert
  expect(fitEncodeSize({ scaleResolutionDownBy: 0.5 }, { width: 640, height: 360 })).toEqual({
    width: 640,
    height: 360
  })
})
