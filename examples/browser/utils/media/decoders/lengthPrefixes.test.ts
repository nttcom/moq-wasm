import { expect, test } from '@playwright/test'
import { withLengthPrefixes } from './lengthPrefixes'

// AVCDecoderConfigurationRecord headers; the low 2 bits of byte 4 are
// lengthSizeMinusOne (0xff → 3, 0xfd → 1).
const AVCC_LENGTH_4 = Uint8Array.of(1, 0x64, 0, 0x28, 0xff, 0xe0)
const AVCC_LENGTH_2 = Uint8Array.of(1, 0x64, 0, 0x28, 0xfd, 0xe0)

test('an Annex-B payload is rewritten with 4-byte length prefixes', () => {
  // Arrange
  const annexb = Uint8Array.of(0, 0, 0, 1, 0x67, 0x42, 0, 0, 1, 0x68, 0xce, 0, 0, 0, 1, 0x65, 0x88, 0x84)

  // Act
  const payload = withLengthPrefixes(annexb, AVCC_LENGTH_4)

  // Assert
  expect([...payload]).toEqual([0, 0, 0, 2, 0x67, 0x42, 0, 0, 0, 2, 0x68, 0xce, 0, 0, 0, 3, 0x65, 0x88, 0x84])
})

test('the length size follows the description', () => {
  // Act
  const payload = withLengthPrefixes(Uint8Array.of(0, 0, 0, 1, 0x41, 0x9a), AVCC_LENGTH_2)

  // Assert
  expect([...payload]).toEqual([0, 2, 0x41, 0x9a])
})

test('a length-prefixed payload whose length begins like a 3-byte start code is kept', () => {
  // Arrange: the 4-byte length 0x00000100 starts with 00 00 01
  const payload = new Uint8Array(4 + 256)
  payload.set([0, 0, 1, 0, 0x41])

  // Act / Assert
  expect(withLengthPrefixes(payload, AVCC_LENGTH_4)).toBe(payload)
})
