export type AudioCodecOption = {
  id: string
  label: string
  codec: string
}

export type AudioBitrateOption = {
  id: string
  label: string
  bitrate: number
}

export type AudioChannelOption = {
  id: string
  label: string
  channels: number
}

export type AudioEncodingSettings = {
  codec: string
  bitrate: number
  channels: number
}

export const AUDIO_CODEC_OPTIONS: AudioCodecOption[] = [
  { id: 'opus', label: 'Opus', codec: 'opus' },
  { id: 'aac', label: 'AAC (mp4a.40.2)', codec: 'mp4a.40.2' }
]

export const AUDIO_BITRATE_OPTIONS: AudioBitrateOption[] = [
  { id: '32kbps', label: '32 kbps', bitrate: 32_000 },
  { id: '64kbps', label: '64 kbps', bitrate: 64_000 },
  { id: '96kbps', label: '96 kbps', bitrate: 96_000 },
  { id: '128kbps', label: '128 kbps', bitrate: 128_000 },
  { id: '192kbps', label: '192 kbps', bitrate: 192_000 },
  { id: '256kbps', label: '256 kbps', bitrate: 256_000 },
  { id: '320kbps', label: '320 kbps', bitrate: 320_000 }
]

export const AUDIO_CHANNEL_OPTIONS: AudioChannelOption[] = [
  { id: 'mono', label: 'Mono (1ch)', channels: 1 },
  { id: 'stereo', label: 'Stereo (2ch)', channels: 2 }
]

export const DEFAULT_AUDIO_ENCODING_SETTINGS: AudioEncodingSettings = {
  codec: AUDIO_CODEC_OPTIONS[0].codec,
  bitrate: AUDIO_BITRATE_OPTIONS.find((b) => b.id === '64kbps')?.bitrate ?? AUDIO_BITRATE_OPTIONS[1].bitrate,
  channels: 1
}
