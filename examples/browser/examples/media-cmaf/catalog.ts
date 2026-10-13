import { MEDIA_DEFAULT_VIDEO_CODEC, type MediaAudioProfile, type MediaVideoProfile } from '../media/catalog'

export const MEDIA_VIDEO_PROFILES: MediaVideoProfile[] = [
  { trackName: 'video_720p', label: '720p', codec: MEDIA_DEFAULT_VIDEO_CODEC, width: 1280, height: 720 }
]

export const MEDIA_AUDIO_PROFILES: MediaAudioProfile[] = [
  { trackName: 'audio_aac', label: 'AAC 128kbps', codec: 'mp4a.40.2', sampleRate: 48000, channels: 2, bitrate: 128_000 }
]
