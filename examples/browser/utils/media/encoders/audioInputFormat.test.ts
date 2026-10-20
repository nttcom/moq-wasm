import { expect, test } from '@playwright/test'
import { encoderConfigForInput } from './audioInputFormat'

const config: AudioEncoderConfig = { codec: 'opus', sampleRate: 48_000, numberOfChannels: 1, bitrate: 128_000 }

test('input in the configured format keeps the config', () => {
  // Act
  const next = encoderConfigForInput(config, { sampleRate: 48_000, numberOfChannels: 1 })

  // Assert
  expect(next).toBe(config)
})

test('input in another format takes its sample rate and channel count', () => {
  // Act
  const next = encoderConfigForInput(config, { sampleRate: 16_000, numberOfChannels: 2 })

  // Assert
  expect(next).toEqual({ codec: 'opus', sampleRate: 16_000, numberOfChannels: 2, bitrate: 128_000 })
})
