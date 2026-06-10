export type CameraId = 'cam01' | 'cam02' | 'cam03' | 'cam04'
export type MonitorMode = 'live' | 'review'
export type ConnState = 'connected' | 'closed'
export type VideoSourceType = 'synthetic' | 'camera' | 'file'

export interface VideoSource {
  type: VideoSourceType
  fileUrl?: string
}

export const ALL_CAMERA_IDS: CameraId[] = ['cam01', 'cam02', 'cam03', 'cam04']

export const RELAY_PRESETS = [
  { label: 'Local', value: 'https://127.0.0.1:4433' },
  { label: 'Cloud', value: 'https://moqt.research.skyway.io:4433' }
] as const
