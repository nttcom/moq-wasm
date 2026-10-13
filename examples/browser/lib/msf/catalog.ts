export const CATALOG_TRACK_NAME = 'catalog'

export type MsfTrack = {
  namespace?: string
  name: string
  packaging: string
  role?: string
  isLive: boolean
  label?: string
  codec?: string
  mimeType?: string
  depends?: string[]
  bitrate?: number
  width?: number
  height?: number
  samplerate?: number
  channelConfig?: string
  initData?: string
}

type MsfCatalog = {
  version?: number
  generatedAt?: number
  isComplete?: boolean
  tracks?: MsfTrack[]
}

export type CatalogTrackRole = 'video' | 'audio' | 'mediatimeline'

export type MediaCatalogTrack = {
  name: string
  label: string
  packaging?: string
  role?: string
  codec?: string
  bitrate?: number
  samplerate?: number
  channelConfig?: string
  width?: number
  height?: number
  initData?: string
  depends?: string[]
}

export function buildMsfCatalogJson(tracks: MsfTrack[]): string {
  const catalog: MsfCatalog = {
    version: 1,
    generatedAt: Date.now(),
    isComplete: true,
    tracks
  }
  return JSON.stringify(catalog)
}

function extractCatalogTracks(catalog: unknown, role?: CatalogTrackRole): MediaCatalogTrack[] {
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
    if (!matchesRole(role, trackRole, asString(track.packaging), name)) {
      return acc
    }
    acc.push({
      name,
      label: asString(track.label) ?? name,
      packaging: asString(track.packaging),
      role: trackRole,
      codec: asString(track.codec),
      bitrate: asNumber(track.bitrate),
      samplerate: asNumber(track.samplerate),
      channelConfig: asString(track.channelConfig),
      width: asNumber(track.width),
      height: asNumber(track.height),
      initData: asString(track.initData),
      depends: asStrings(track.depends)
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

export function extractCatalogMediaTimelineTracks(catalog: unknown): MediaCatalogTrack[] {
  return extractCatalogTracks(catalog, 'mediatimeline')
}

export function extractCatalogCmafTracks(catalog: unknown): MediaCatalogTrack[] {
  return extractCatalogTracks(catalog).filter((track) => track.packaging === 'cmaf')
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null
}

function asString(value: unknown): string | undefined {
  return typeof value === 'string' && value.trim().length > 0 ? value : undefined
}

function asStrings(value: unknown): string[] | undefined {
  return Array.isArray(value) ? value.filter((item): item is string => typeof item === 'string') : undefined
}

function asNumber(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined
}

function matchesRole(
  expectedRole: CatalogTrackRole | undefined,
  trackRole: string | undefined,
  trackPackaging: string | undefined,
  trackName: string
): boolean {
  if (!expectedRole) {
    return true
  }
  /// draft-ietf-moq-msf-01 §7.2 requires the packaging; the role of §5.2.6 is
  /// optional, so either one identifies a media timeline track.
  if (expectedRole === 'mediatimeline') {
    return trackPackaging === 'mediatimeline' || trackRole === 'mediatimeline'
  }
  if (trackPackaging === 'mediatimeline' || trackRole === 'mediatimeline') {
    return false
  }
  if (expectedRole === 'video') {
    return !trackRole || trackRole === 'video'
  }
  return trackRole === 'audio' || trackName === 'audio'
}
