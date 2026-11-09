import { expect, test } from '@playwright/test'
import { AUDIO_LEVEL_ID, TIMESCALE_ID, TIMESTAMP_ID, VIDEO_CONFIG_ID, buildLocHeader, readLocHeader } from './loc'

test('the capture time and video config round-trip as LOC-04 properties', () => {
  // Arrange
  const meta = { timestampMicros: 1_700_000_000_000_000, videoConfig: Uint8Array.of(1, 0x42) }

  // Act
  const header = buildLocHeader(meta)

  // Assert
  expect(header).toEqual([
    { id: TIMESTAMP_ID, value: { varint: meta.timestampMicros } },
    { id: VIDEO_CONFIG_ID, value: { bytes: meta.videoConfig } }
  ])
  expect(readLocHeader(header)).toEqual(meta)
})

test('a Timestamp with a Timescale is read as media time in microseconds', () => {
  // Arrange
  const header = [
    { id: TIMESCALE_ID, value: { varint: 48_000 } },
    { id: TIMESTAMP_ID, value: { varint: 96_000 } }
  ]

  // Act
  const meta = readLocHeader(header)

  // Assert
  expect(meta).toEqual({ timestampMicros: 2_000_000, mediaTime: true })
})

test('a Timestamp with a Timescale of 0 is not read', () => {
  // Arrange
  const header = [
    { id: TIMESCALE_ID, value: { varint: 0 } },
    { id: TIMESTAMP_ID, value: { varint: 96_000 } }
  ]

  // Act / Assert
  expect(readLocHeader(header).timestampMicros).toBeUndefined()
})

test('an Audio Level without the V bit is read as a level without voice activity', () => {
  // Arrange
  const header = [{ id: AUDIO_LEVEL_ID, value: { varint: 90 } }]

  // Act / Assert
  expect(readLocHeader(header).audioLevel).toEqual({ level: 90, voiceActivity: false })
})
