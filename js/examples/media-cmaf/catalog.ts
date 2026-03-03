export {
  MEDIA_CATALOG_TRACK_NAME,
  type MediaVideoProfile,
  type MediaAudioProfile,
  type MediaCatalogTrack,
  extractCatalogTracks,
  extractCatalogVideoTracks,
  extractCatalogAudioTracks
} from '../media/catalog'

import { MEDIA_DEFAULT_VIDEO_CODEC, type MediaVideoProfile, type MediaAudioProfile } from '../media/catalog'

export const MEDIA_VIDEO_PROFILES: MediaVideoProfile[] = [
  { trackName: 'video_720p', label: '720p', codec: MEDIA_DEFAULT_VIDEO_CODEC, width: 1280, height: 720 }
]

export const MEDIA_AUDIO_PROFILES: MediaAudioProfile[] = [
  { trackName: 'audio_aac', label: 'AAC 128kbps', codec: 'mp4a.40.2', sampleRate: 48000, channels: 2, bitrate: 128_000 }
]

type MsfTrack = {
  namespace?: string
  name: string
  packaging: string
  role?: string
  isLive: boolean
  label?: string
  codec?: string
  width?: number
  height?: number
  bitrate?: number
  samplerate?: number
  channelConfig?: string
}

type MsfCatalog = {
  version?: number
  generatedAt?: number
  isComplete?: boolean
  tracks?: MsfTrack[]
}

export const buildCmafCatalogJson = (trackNamespace: string[]): string => {
  const namespace = trackNamespace.length > 0 ? trackNamespace.join('/') : undefined
  const videoTracks: MsfTrack[] = MEDIA_VIDEO_PROFILES.map((profile) => ({
    namespace,
    name: profile.trackName,
    packaging: 'cmaf',
    role: 'video',
    isLive: true,
    label: profile.label,
    codec: profile.codec,
    width: profile.width,
    height: profile.height
  }))
  const audioTracks: MsfTrack[] = MEDIA_AUDIO_PROFILES.map((profile) => ({
    namespace,
    name: profile.trackName,
    packaging: 'cmaf',
    role: 'audio',
    isLive: true,
    label: profile.label,
    codec: profile.codec,
    bitrate: profile.bitrate,
    samplerate: profile.sampleRate,
    channelConfig: profile.channels === 1 ? 'mono' : `${profile.channels}ch`
  }))
  const catalog: MsfCatalog = {
    version: 1,
    generatedAt: Date.now(),
    isComplete: true,
    tracks: [...videoTracks, ...audioTracks]
  }
  return JSON.stringify(catalog)
}
