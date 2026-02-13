export const MEDIA_CATALOG_TRACK_NAME = 'catalog'
export const MEDIA_AUDIO_TRACK_NAME = 'audio'

export type MediaVideoProfile = {
  trackName: string
  label: string
  codec: string
  width: number
  height: number
}

export const MEDIA_DEFAULT_VIDEO_CODEC = 'avc1.640032'

export const MEDIA_VIDEO_PROFILES: MediaVideoProfile[] = [
  { trackName: 'video_1080p', label: '1080p', codec: MEDIA_DEFAULT_VIDEO_CODEC, width: 1920, height: 1080 },
  { trackName: 'video_720p', label: '720p', codec: MEDIA_DEFAULT_VIDEO_CODEC, width: 1280, height: 720 },
  { trackName: 'video_480p', label: '480p', codec: MEDIA_DEFAULT_VIDEO_CODEC, width: 854, height: 480 }
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

type MsfTrack = {
  namespace?: string
  name: string
  packaging: string
  role?: string
  isLive: boolean
  label?: string
  codec?: string
  bitrate?: number
  width?: number
  height?: number
  samplerate?: number
  channelConfig?: string
}

type MsfCatalog = {
  version?: number
  generatedAt?: number
  isComplete?: boolean
  tracks?: MsfTrack[]
}

export type CatalogTrackRole = 'video' | 'audio'

export type MediaCatalogTrack = {
  name: string
  label: string
  role?: string
  codec?: string
  bitrate?: number
  samplerate?: number
  channelConfig?: string
  width?: number
  height?: number
}

export function buildMediaCatalogJson(trackNamespace: string[]): string {
  const namespace = trackNamespace.length > 0 ? trackNamespace.join('/') : undefined
  const videoTracks: MsfTrack[] = MEDIA_VIDEO_PROFILES.map((profile) => ({
    namespace,
    name: profile.trackName,
    packaging: 'loc',
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
    packaging: 'loc',
    role: 'audio',
    isLive: true,
    label: profile.label,
    codec: profile.codec,
    bitrate: profile.bitrate,
    samplerate: profile.sampleRate,
    channelConfig: profile.channels === 1 ? 'mono' : `${profile.channels}ch`
  }))
  const tracks: MsfTrack[] = [...videoTracks, ...audioTracks]
  const catalog: MsfCatalog = {
    version: 1,
    generatedAt: Date.now(),
    isComplete: true,
    tracks
  }
  return JSON.stringify(catalog)
}

export function extractCatalogTracks(catalog: unknown, role?: CatalogTrackRole): MediaCatalogTrack[] {
  const tracks = Array.isArray((catalog as { tracks?: unknown[] } | undefined)?.tracks)
    ? ((catalog as { tracks: unknown[] }).tracks ?? [])
    : []
  return tracks.reduce<MediaCatalogTrack[]>((acc, track) => {
    if (!isObject(track)) {
      return acc
    }
    const name = asString(track.name)
    if (!name) {
      return acc
    }
    const trackRole = asString(track.role)
    if (!matchesRole(role, trackRole, name)) {
      return acc
    }
    acc.push({
      name,
      label: asString(track.label) ?? name,
      role: trackRole,
      codec: asString(track.codec),
      bitrate: asNumber(track.bitrate),
      samplerate: asNumber(track.samplerate),
      channelConfig: asString(track.channelConfig),
      width: asNumber(track.width),
      height: asNumber(track.height)
    })
    return acc
  }, [])
}

export function extractCatalogVideoTracks(catalog: unknown): MediaCatalogTrack[] {
  return extractCatalogTracks(catalog, 'video')
}

export function extractCatalogAudioTracks(catalog: unknown): MediaCatalogTrack[] {
  return extractCatalogTracks(catalog, 'audio')
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null
}

function asString(value: unknown): string | undefined {
  return typeof value === 'string' && value.trim().length > 0 ? value : undefined
}

function asNumber(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined
}

function matchesRole(
  expectedRole: CatalogTrackRole | undefined,
  trackRole: string | undefined,
  trackName: string
): boolean {
  if (!expectedRole) {
    return true
  }
  if (expectedRole === 'video') {
    return !trackRole || trackRole === 'video'
  }
  return trackRole === 'audio' || trackName === MEDIA_AUDIO_TRACK_NAME || hasAudioProfileTrackName(trackName)
}

function hasAudioProfileTrackName(trackName: string): boolean {
  return MEDIA_AUDIO_PROFILES.some((profile) => profile.trackName === trackName)
}
