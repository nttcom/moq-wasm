import { expect, test } from '@playwright/test'
import { audioLevelOfSamples, louderAudioLevel } from './audioLevel'
import { buildLocHeader, readLocHeader } from './loc'

test('a full-scale square wave is level 0 with voice activity', () => {
  // Arrange
  const samples = Float32Array.from({ length: 480 }, (_, index) => (index % 2 === 0 ? 1 : -1))

  // Act / Assert
  expect(audioLevelOfSamples(samples)).toEqual({ level: 0, voiceActivity: true })
})

test('a signal at -60 dBov is quiet without voice activity', () => {
  // Arrange
  const samples = new Float32Array(480).fill(0.001)

  // Act / Assert
  expect(audioLevelOfSamples(samples)).toEqual({ level: 60, voiceActivity: false })
})

test('silence is the quietest level', () => {
  // Act / Assert
  expect(audioLevelOfSamples(new Float32Array(480))).toEqual({ level: 127, voiceActivity: false })
})

test('the LOC header carries the level and the V bit in one varint', () => {
  // Arrange
  const audioLevel = { level: 33, voiceActivity: true }

  // Act
  const header = buildLocHeader({ captureTimestampMicros: 1, audioLevel })

  // Assert
  expect(header).toContainEqual({ id: 6, value: { varint: 0x80 | 33 } })
  expect(readLocHeader(header).audioLevel).toEqual(audioLevel)
})

test('the louder of two levels is the lower one', () => {
  // Arrange
  const quiet = { level: 70, voiceActivity: false }
  const loud = { level: 30, voiceActivity: true }

  // Act / Assert
  expect(louderAudioLevel(quiet, loud)).toBe(loud)
  expect(louderAudioLevel(loud, quiet)).toBe(loud)
  expect(louderAudioLevel(undefined, quiet)).toBe(quiet)
})
