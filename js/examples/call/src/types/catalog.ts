export type CatalogTrackRole = 'video' | 'audio' | 'chat'
export type CatalogSubscribeRole = 'video' | 'screenshare' | 'audio' | 'chat'
export type AudioStreamUpdateMode = 'single' | 'interval'

export interface CallCatalogTrack {
  name: string
  label: string
  role: CatalogTrackRole
  codec?: string
  bitrate?: number
  width?: number
  height?: number
  keyframeInterval?: number
  samplerate?: number
  channelConfig?: string
  audioStreamUpdateMode?: AudioStreamUpdateMode
  audioStreamUpdateIntervalSeconds?: number
  isLive?: boolean
}

export interface EditableCallCatalogTrack extends CallCatalogTrack {
  id: string
}

export interface AudioStreamUpdateSettings {
  mode: AudioStreamUpdateMode
  intervalSeconds: number
}

export const DEFAULT_AUDIO_STREAM_UPDATE_SETTINGS: AudioStreamUpdateSettings = {
  mode: 'interval',
  intervalSeconds: 1
}

export const DEFAULT_VIDEO_KEYFRAME_INTERVAL = 300
