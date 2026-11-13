import { expect, test } from '@playwright/test'
import { checkRenditions, renditionEncoderConfigs } from './rendition'

test('a rendition scaled down by 1 or more is accepted', () => {
  // Act / Assert
  expect(() =>
    checkRenditions([
      { name: 'low', label: 'Low', bitrate: 150_000, scaleResolutionDownBy: 4 },
      { name: '360p', label: '360p', bitrate: 600_000, width: 640, height: 360 }
    ])
  ).not.toThrow()
})

test('a rendition scaled by less than 1 or by a non-finite value is rejected', () => {
  // Act / Assert
  for (const scaleResolutionDownBy of [0.5, 0, Number.NaN, Number.POSITIVE_INFINITY]) {
    expect(() => checkRenditions([{ name: 'low', label: 'Low', bitrate: 150_000, scaleResolutionDownBy }])).toThrow(
      RangeError
    )
  }
})

test('a rendition takes its own codec, or the video codec without one', () => {
  // Arrange
  const renditions = [
    { name: '360p', label: '360p', width: 640, height: 360, bitrate: 400_000 },
    { name: '360p_avc', label: '360p', width: 640, height: 360, bitrate: 600_000, codec: 'h264' }
  ]

  // Act
  const configs = renditionEncoderConfigs(renditions, { codec: 'h265', bitrate: 1_500_000 })

  // Assert
  expect(configs.map(({ codec, bitrate }) => ({ codec, bitrate }))).toEqual([
    { codec: 'h265', bitrate: 400_000 },
    { codec: 'h264', bitrate: 600_000 }
  ])
})
