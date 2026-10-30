export type LocValue = { varint: number } | { bytes: Uint8Array }

export type LocExtension = { id: number; value: LocValue }

export type LocHeader = LocExtension[]

// draft-ietf-moq-loc-04 §6.1 property ids.
export const TIMESCALE_ID = 0x08
export const AUDIO_LEVEL_ID = 0x0c
export const VIDEO_CONFIG_ID = 0x0d
export const TIMESTAMP_ID = 0x10

/// RFC 6464 §3: `level` is the audio level in -dBov, 0 the loudest and 127 the
/// quietest, and `voiceActivity` the V bit.
export type AudioLevel = { level: number; voiceActivity: boolean }

export type LocMetadata = {
  captureTimestampMicros?: number
  videoConfig?: Uint8Array
  audioLevel?: AudioLevel
}

const VOICE_ACTIVITY_BIT = 0x80
const LEVEL_MASK = 0x7f

export function buildLocHeader(meta: LocMetadata): LocHeader {
  const extensions: LocHeader = []
  if (typeof meta.captureTimestampMicros === 'number') {
    extensions.push({ id: TIMESTAMP_ID, value: { varint: meta.captureTimestampMicros } })
  }
  if (meta.videoConfig) {
    extensions.push({ id: VIDEO_CONFIG_ID, value: { bytes: meta.videoConfig } })
  }
  if (meta.audioLevel) {
    const { level, voiceActivity } = meta.audioLevel
    extensions.push({
      id: AUDIO_LEVEL_ID,
      value: { varint: (voiceActivity ? VOICE_ACTIVITY_BIT : 0) | (level & LEVEL_MASK) }
    })
  }
  return extensions
}

/// A Timestamp sent with a Timescale property is media time with an
/// application-defined epoch (draft-ietf-moq-loc-04 §2.3.1.2), not the Unix µs
/// capture time, so it is not read.
export function readLocHeader(header?: LocHeader): LocMetadata {
  const meta: LocMetadata = {}
  const mediaTime = (header ?? []).some((ext) => ext.id === TIMESCALE_ID && 'varint' in ext.value)
  for (const ext of header ?? []) {
    if (ext.id === VIDEO_CONFIG_ID && 'bytes' in ext.value) {
      meta.videoConfig = ext.value.bytes
    } else if (ext.id === TIMESTAMP_ID && 'varint' in ext.value && !mediaTime) {
      meta.captureTimestampMicros = ext.value.varint
    } else if (ext.id === AUDIO_LEVEL_ID && 'varint' in ext.value) {
      meta.audioLevel = {
        level: ext.value.varint & LEVEL_MASK,
        voiceActivity: (ext.value.varint & VOICE_ACTIVITY_BIT) !== 0
      }
    }
  }
  return meta
}
