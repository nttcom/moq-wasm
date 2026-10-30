import { expect, test } from '@playwright/test'
import { TIMESCALE_ID, TIMESTAMP_ID, VIDEO_CONFIG_ID, buildLocHeader, readLocHeader } from './loc'

test('the capture time and video config round-trip as LOC-04 properties', () => {
  // Arrange
  const meta = { captureTimestampMicros: 1_700_000_000_000_000, videoConfig: Uint8Array.of(1, 0x42) }

  // Act
  const header = buildLocHeader(meta)

  // Assert
  expect(header).toEqual([
    { id: TIMESTAMP_ID, value: { varint: meta.captureTimestampMicros } },
    { id: VIDEO_CONFIG_ID, value: { bytes: meta.videoConfig } }
  ])
  expect(readLocHeader(header)).toEqual(meta)
})

test('a Timestamp in media time is not read as the capture time', () => {
  // Arrange
  const header = [
    { id: TIMESCALE_ID, value: { varint: 48_000 } },
    { id: TIMESTAMP_ID, value: { varint: 96_000 } }
  ]

  // Act / Assert
  expect(readLocHeader(header).captureTimestampMicros).toBeUndefined()
})
