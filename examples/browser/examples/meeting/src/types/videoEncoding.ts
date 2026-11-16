export type VideoResolutionOption = {
  id: string
  label: string
  width: number
  height: number
}

export type VideoHardwareAccelerationOption = {
  id: string
  label: string
  value: HardwareAcceleration
}

export type VideoEncodingSettings = {
  codec: string
  width: number
  height: number
  bitrate: number
  framerate: number
  hardwareAcceleration: HardwareAcceleration
}

export const VIDEO_RESOLUTION_OPTIONS: VideoResolutionOption[] = [
  { id: '144p', label: '256 x 144 (144p)', width: 256, height: 144 },
  { id: '240p', label: '426 x 240 (240p)', width: 426, height: 240 },
  { id: '360p', label: '640 x 360 (360p)', width: 640, height: 360 },
  { id: '480p', label: '854 x 480 (480p)', width: 854, height: 480 },
  { id: '720p', label: '1280 x 720 (720p)', width: 1280, height: 720 },
  { id: '1080p', label: '1920 x 1080 (1080p)', width: 1920, height: 1080 },
  { id: '1440p', label: '2560 x 1440 (1440p)', width: 2560, height: 1440 },
  { id: '2160p', label: '3840 x 2160 (2160p)', width: 3840, height: 2160 },
  { id: '4320p', label: '7680 x 4320 (4320p)', width: 7680, height: 4320 },
  // Portrait presets
  { id: '720p-portrait', label: '720 x 1280 (Portrait 720p)', width: 720, height: 1280 },
  { id: '1080p-portrait', label: '1080 x 1920 (Portrait 1080p)', width: 1080, height: 1920 },
  { id: '1440p-portrait', label: '1440 x 2560 (Portrait 1440p)', width: 1440, height: 2560 }
]

export const VIDEO_HARDWARE_ACCELERATION_OPTIONS: VideoHardwareAccelerationOption[] = [
  { id: 'prefer-hardware', label: 'Prefer hardware', value: 'prefer-hardware' },
  { id: 'prefer-software', label: 'Prefer software', value: 'prefer-software' }
]

export const DEFAULT_VIDEO_ENCODING_SETTINGS: VideoEncodingSettings = {
  codec: 'avc1.4D4028',
  width: VIDEO_RESOLUTION_OPTIONS.find((r) => r.id === '1080p')?.width ?? VIDEO_RESOLUTION_OPTIONS[0].width,
  height: VIDEO_RESOLUTION_OPTIONS.find((r) => r.id === '1080p')?.height ?? VIDEO_RESOLUTION_OPTIONS[0].height,
  bitrate: 1_000_000,
  framerate: 30,
  hardwareAcceleration: 'prefer-software'
}

export const DEFAULT_SCREEN_SHARE_ENCODING_SETTINGS: VideoEncodingSettings = {
  codec: 'av01.0.08M.08',
  width: 1920,
  height: 1080,
  bitrate: 2_000_000,
  framerate: 30,
  hardwareAcceleration: 'prefer-software'
}
