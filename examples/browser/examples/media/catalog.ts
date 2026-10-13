import type { MsfTrack } from '../../lib/msf/catalog'
import { buildMsfCatalogJson } from '../../lib/msf/catalog'
import { getMediaVideoEncodingOverrides } from './common'

export type MediaVideoProfile = {
  trackName: string
  label: string
  codec: string
  width: number
  height: number
}

export const MEDIA_DEFAULT_VIDEO_CODEC = 'avc1.640028' // H.264 High Profile Level 4.0

const MEDIA_VIDEO_PROFILE_LAYOUTS: Omit<MediaVideoProfile, 'codec'>[] = [
  { trackName: 'video_1080p', label: '1080p', width: 1920, height: 1080 },
  { trackName: 'video_720p', label: '720p', width: 1280, height: 720 },
  { trackName: 'video_480p', label: '480p', width: 854, height: 480 }
]

export type MediaAudioProfile = {
  trackName: string
  label: string
  codec: string
  sampleRate: number
  channels: number
  bitrate: number
}

export const MEDIA_AUDIO_PROFILES: MediaAudioProfile[] = [
  {
    trackName: 'audio_128kbps',
    label: 'audio 128kbps',
    codec: 'opus',
    sampleRate: 48000,
    channels: 1,
    bitrate: 128_000
  },
  {
    trackName: 'audio_64kbps',
    label: 'audio 64kbps',
    codec: 'opus',
    sampleRate: 48000,
    channels: 1,
    bitrate: 64_000
  },
  {
    trackName: 'audio_32kbps',
    label: 'audio 32kbps',
    codec: 'opus',
    sampleRate: 48000,
    channels: 1,
    bitrate: 32_000
  }
]

export function getResolvedMediaVideoCodec(): string {
  return getMediaVideoEncodingOverrides().codec ?? MEDIA_DEFAULT_VIDEO_CODEC
}

export function getMediaVideoProfiles(): MediaVideoProfile[] {
  const codec = getResolvedMediaVideoCodec()
  return MEDIA_VIDEO_PROFILE_LAYOUTS.map((profile) => ({
    ...profile,
    codec
  }))
}

export function buildMediaCatalogJson(
  trackNamespace: string[],
  packaging: 'loc' | 'cmaf',
  videoProfiles: MediaVideoProfile[],
  audioProfiles: MediaAudioProfile[]
): string {
  const namespace = trackNamespace.length > 0 ? trackNamespace.join('/') : undefined
  const videoTracks: MsfTrack[] = videoProfiles.map((profile) => ({
    namespace,
    name: profile.trackName,
    packaging,
    role: 'video',
    isLive: true,
    label: profile.label,
    codec: profile.codec,
    width: profile.width,
    height: profile.height
  }))
  const audioTracks: MsfTrack[] = audioProfiles.map((profile) => ({
    namespace,
    name: profile.trackName,
    packaging,
    role: 'audio',
    isLive: true,
    label: profile.label,
    codec: profile.codec,
    bitrate: profile.bitrate,
    samplerate: profile.sampleRate,
    channelConfig: profile.channels === 1 ? 'mono' : `${profile.channels}ch`
  }))
  return buildMsfCatalogJson([...videoTracks, ...audioTracks])
}
