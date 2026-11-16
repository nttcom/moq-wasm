import {
  AUDIO_LEVEL_ID,
  LEVEL_MASK,
  type LocHeader,
  type LocMetadata,
  TIMESTAMP_ID,
  VIDEO_CONFIG_ID,
  VOICE_ACTIVITY_BIT
} from '../loc'

export function buildLocHeader(meta: LocMetadata): LocHeader {
  const extensions: LocHeader = []
  if (typeof meta.timestampMicros === 'number') {
    extensions.push({ id: TIMESTAMP_ID, value: { varint: meta.timestampMicros } })
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
