export type CatalogTrackRole = 'video' | 'audio'
export type CatalogSubscribeRole = 'video' | 'screenshare' | 'audio'

export interface CallCatalogTrack {
  name: string
  label: string
  role: CatalogTrackRole
  codec?: string
  bitrate?: number
  width?: number
  height?: number
  samplerate?: number
  channelConfig?: string
  isLive?: boolean
}

export interface EditableCallCatalogTrack extends CallCatalogTrack {
  id: string
}
