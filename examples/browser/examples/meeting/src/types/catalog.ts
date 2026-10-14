export type CatalogTrackRole = 'video' | 'audio' | 'chat'
export type CatalogSubscribeRole = 'video' | 'screenshare' | 'audio' | 'chat'

/// The roles whose track can change while subscribed; see `LocalSession.switchTrack`.
export function isSwitchableRole(role: CatalogSubscribeRole): boolean {
  return role === 'video' || role === 'screenshare'
}
export const CATALOG_SUBSCRIBE_ROLES: CatalogSubscribeRole[] = ['video', 'screenshare', 'audio', 'chat']
export type CatalogSelections = Partial<Record<CatalogSubscribeRole, string>>
export type AudioStreamUpdateMode = 'single' | 'interval'

export interface MeetingCatalogTrack {
  name: string
  label: string
  role: CatalogTrackRole
  codec?: string
  bitrate?: number
  width?: number
  height?: number
  framerate?: number
  hardwareAcceleration?: HardwareAcceleration
  keyframeInterval?: number
  samplerate?: number
  channelConfig?: string
  initData?: string
  audioStreamUpdateMode?: AudioStreamUpdateMode
  audioStreamUpdateIntervalSeconds?: number
  isLive?: boolean
}

export interface EditableMeetingCatalogTrack extends MeetingCatalogTrack {
  id: string
}

export type TrackMediaConfig = {
  codec?: string
  samplerate?: number
  channelConfig?: string
  initData?: string
}

export interface AudioStreamUpdateSettings {
  mode: AudioStreamUpdateMode
  intervalSeconds: number
}

export const DEFAULT_AUDIO_STREAM_UPDATE_SETTINGS: AudioStreamUpdateSettings = {
  mode: 'interval',
  intervalSeconds: 1
}

/// In frames: 2 s at 30 fps. A viewer joining a rendition that the relay already
/// receives starts at its next keyframe, since the relay does not pass that
/// SUBSCRIBE on to the publisher.
export const DEFAULT_VIDEO_KEYFRAME_INTERVAL = 60
