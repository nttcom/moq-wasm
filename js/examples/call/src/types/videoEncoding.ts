export type VideoCodecOption = {
  id: string
  label: string
  codec: string
}

export type VideoResolutionOption = {
  id: string
  label: string
  width: number
  height: number
}

export type VideoBitrateOption = {
  id: string
  label: string
  bitrate: number
}

export type VideoEncodingSettings = {
  codec: string
  width: number
  height: number
  bitrate: number
}

export const VIDEO_CODEC_OPTIONS: VideoCodecOption[] = [
  { id: 'h264-high40', label: 'H.264 High@4.0 (avc1.640028)', codec: 'avc1.640028' },
  { id: 'h264-high50', label: 'H.264 High@5.0 (avc1.640032)', codec: 'avc1.640032' },
  { id: 'h264-main40', label: 'H.264 Main@4.0 (avc1.4D4028)', codec: 'avc1.4D4028' },
  { id: 'h264-baseline31', label: 'H.264 Baseline@3.1 (avc1.42001F)', codec: 'avc1.42001F' },
  { id: 'av1-main-8bit-l3', label: 'AV1 Main 8bit (av01.0.08M.08)', codec: 'av01.0.08M.08' },
  { id: 'av1-high-8bit-l4', label: 'AV1 High 8bit (av01.1.08M.08)', codec: 'av01.1.08M.08' },
  { id: 'vp8', label: 'VP8 (vp8)', codec: 'vp8' },
  { id: 'vp9', label: 'VP9 (vp09.00.10.08)', codec: 'vp09.00.10.08' }
]

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

export const VIDEO_BITRATE_OPTIONS: VideoBitrateOption[] = [
  { id: '200kbps', label: '200 kbps', bitrate: 200_000 },
  { id: '500kbps', label: '500 kbps', bitrate: 500_000 },
  { id: '1mbps', label: '1 Mbps', bitrate: 1_000_000 },
  { id: '2mbps', label: '2 Mbps', bitrate: 2_000_000 },
  { id: '3mbps', label: '3 Mbps', bitrate: 3_000_000 },
  { id: '5mbps', label: '5 Mbps', bitrate: 5_000_000 }
]

export const DEFAULT_VIDEO_ENCODING_SETTINGS: VideoEncodingSettings = {
  codec: VIDEO_CODEC_OPTIONS.find((c) => c.id.startsWith('h264'))?.codec ?? VIDEO_CODEC_OPTIONS[0].codec,
  width: VIDEO_RESOLUTION_OPTIONS.find((r) => r.id === '1080p')?.width ?? VIDEO_RESOLUTION_OPTIONS[0].width,
  height: VIDEO_RESOLUTION_OPTIONS.find((r) => r.id === '1080p')?.height ?? VIDEO_RESOLUTION_OPTIONS[0].height,
  bitrate: VIDEO_BITRATE_OPTIONS.find((b) => b.id === '1mbps')?.bitrate ?? VIDEO_BITRATE_OPTIONS[0].bitrate
}
