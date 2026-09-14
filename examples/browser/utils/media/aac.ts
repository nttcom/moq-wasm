import { bytesToBase64 } from './loc'

const SAMPLING_FREQUENCIES = [96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350]
const AAC_LC_OBJECT_TYPE = 2

/// ISO/IEC 14496-3 AudioSpecificConfig for AAC-LC: 5 bits object type, 4 bits
/// sampling frequency index, 4 bits channel configuration, padded to 2 bytes.
export function audioSpecificConfigBase64(sampleRate: number, channels: number): string | undefined {
  const frequencyIndex = SAMPLING_FREQUENCIES.indexOf(sampleRate)
  if (frequencyIndex < 0) {
    return undefined
  }
  const bits = (AAC_LC_OBJECT_TYPE << 11) | (frequencyIndex << 7) | (channels << 3)
  return bytesToBase64(new Uint8Array([bits >> 8, bits & 0xff]))
}
