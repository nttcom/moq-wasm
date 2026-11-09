import { expect, test } from '@playwright/test'
import { videoEncoderConfig } from './videoEncoderConfig'

test('a field the page sets replaces the default, such as a quality latency mode for screen sharing', () => {
  // Act
  const config = videoEncoderConfig({ codec: 'vp09.00.10.08', width: 1920, height: 1080, latencyMode: 'quality' })

  // Assert
  expect(config).toMatchObject({ codec: 'vp09.00.10.08', latencyMode: 'quality', bitrate: 1_000_000, framerate: 30 })
})

test('a field the page leaves undefined keeps the default', () => {
  // Act / Assert
  expect(videoEncoderConfig({ codec: 'avc1.640028', width: 640, height: 360, bitrate: undefined }).bitrate).toBe(
    1_000_000
  )
})

test('H.264 is encoded as Annex B whatever the page asks', () => {
  // Act / Assert
  expect(videoEncoderConfig({ codec: 'avc1.42e01f', width: 640, height: 360, avc: { format: 'avc' } }).avc).toEqual({
    format: 'annexb'
  })
})

test('a frame rate outside 1 to 120 is clamped', () => {
  // Act / Assert
  expect(videoEncoderConfig({ codec: 'avc1.640028', width: 640, height: 360, framerate: 240 }).framerate).toBe(120)
})
