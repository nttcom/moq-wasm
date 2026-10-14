export type LocValue = { varint: number } | { bytes: Uint8Array }

export type LocExtension = { id: number; value: LocValue }

export type LocHeader = LocExtension[]

export const CAPTURE_TIMESTAMP_ID = 2
export const VIDEO_CONFIG_ID = 13
export const AUDIO_LEVEL_ID = 6

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
    extensions.push({ id: CAPTURE_TIMESTAMP_ID, value: { varint: meta.captureTimestampMicros } })
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

export function readLocHeader(header?: LocHeader): LocMetadata {
  const meta: LocMetadata = {}
  for (const ext of header ?? []) {
    if (ext.id === VIDEO_CONFIG_ID && 'bytes' in ext.value) {
      meta.videoConfig = ext.value.bytes
    } else if (ext.id === CAPTURE_TIMESTAMP_ID && 'varint' in ext.value) {
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
