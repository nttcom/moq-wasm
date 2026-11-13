/// The last element of a camera's namespace, `[root, location, camera]`, shown as its name.
export type CameraId = string
export type MonitorMode = 'live' | 'review'
export type ConnState = 'connected' | 'closed'
export type VideoSourceType = 'synthetic' | 'camera' | 'file'

export interface VideoSource {
  type: VideoSourceType
  fileUrl?: string
}

/// One namespace element that is also safe in a URL query.
export const NAME_PATTERN = /^[a-z0-9-]{1,32}$/
