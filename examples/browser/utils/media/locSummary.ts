import { AUDIO_LEVEL_ID, CAPTURE_TIMESTAMP_ID, VIDEO_CONFIG_ID, VIDEO_FRAME_MARKING_ID, type LocHeader } from './loc'

export type LocHeaderSummary = {
  present: boolean
  extensionCount: number
  hasCaptureTimestamp: boolean
  hasVideoConfig: boolean
  hasVideoFrameMarking: boolean
  hasAudioLevel: boolean
}

export function summarizeLocHeader(locHeader: unknown): LocHeaderSummary {
  const extensions: LocHeader = Array.isArray(locHeader) ? locHeader : []
  const has = (id: number) => extensions.some((ext) => ext.id === id)
  return {
    present: Boolean(locHeader),
    extensionCount: extensions.length,
    hasCaptureTimestamp: has(CAPTURE_TIMESTAMP_ID),
    hasVideoConfig: has(VIDEO_CONFIG_ID),
    hasVideoFrameMarking: has(VIDEO_FRAME_MARKING_ID),
    hasAudioLevel: has(AUDIO_LEVEL_ID)
  }
}
