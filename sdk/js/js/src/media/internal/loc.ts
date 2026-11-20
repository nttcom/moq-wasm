type LocValue = { varint: number } | { bytes: Uint8Array }

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
  /// The Timestamp property in µs: wall-clock time since the Unix epoch
  /// (draft-ietf-moq-loc-04 §2.3.1.1), or media time when `mediaTime` is set (§2.3.1.2).
  timestampMicros?: number
  /// Set only by `readLocHeader`: a Timescale property came with the Timestamp.
  mediaTime?: boolean
  videoConfig?: Uint8Array
  audioLevel?: AudioLevel
}

const MICROS_PER_SECOND = 1_000_000n
export const VOICE_ACTIVITY_BIT = 0x80
export const LEVEL_MASK = 0x7f

/// A Timestamp with a Timescale property of 0 is not read.
export function readLocHeader(header?: LocHeader): LocMetadata {
  const meta: LocMetadata = {}
  const timescale = (header ?? []).find((ext) => ext.id === TIMESCALE_ID && 'varint' in ext.value)
  const ticksPerSecond = timescale && 'varint' in timescale.value ? timescale.value.varint : undefined
  for (const ext of header ?? []) {
    if (ext.id === VIDEO_CONFIG_ID && 'bytes' in ext.value) {
      meta.videoConfig = ext.value.bytes
    } else if (ext.id === TIMESTAMP_ID && 'varint' in ext.value && ticksPerSecond === undefined) {
      meta.timestampMicros = ext.value.varint
    } else if (ext.id === TIMESTAMP_ID && 'varint' in ext.value && ticksPerSecond) {
      meta.timestampMicros = ticksToMicros(ext.value.varint, ticksPerSecond)
      meta.mediaTime = true
    } else if (ext.id === AUDIO_LEVEL_ID && 'varint' in ext.value) {
      meta.audioLevel = {
        level: ext.value.varint & LEVEL_MASK,
        voiceActivity: (ext.value.varint & VOICE_ACTIVITY_BIT) !== 0
      }
    }
  }
  return meta
}

/// Rounds down to a whole microsecond, as the Rust LOC reader does: WebCodecs
/// timestamps are integers, so a fraction would not match the timestamp a
/// decoded frame comes back with.
function ticksToMicros(ticks: number, ticksPerSecond: number): number {
  return Number((BigInt(ticks) * MICROS_PER_SECOND) / BigInt(ticksPerSecond))
}
