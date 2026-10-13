export const CATALOG_TRACK_NAME = 'catalog'

export type MsfTrack = {
  namespace?: string
  name: string
  packaging: string
  eventType?: string
  role?: string
  isLive: boolean
  label?: string
  codec?: string
  mimeType?: string
  depends?: string[]
  bitrate?: number
  width?: number
  height?: number
  framerate?: number
  samplerate?: number
  channelConfig?: string
  initRef?: string
}

/// An entry of the catalog `initDataList` (draft-ietf-moq-msf-01 §5.1.7).
export type MsfInitData = {
  id: string
  type: 'inline'
  data: string
}

type MsfCatalog = {
  version?: number
  generatedAt?: number
  isComplete?: boolean
  tracks?: MsfTrack[]
  initDataList?: MsfInitData[]
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

export function buildMsfCatalogJson(tracks: MsfTrack[], initDataList: MsfInitData[] = []): string {
  const catalog: MsfCatalog = {
    version: 1,
    generatedAt: Date.now(),
    tracks,
    // Keep after `tracks`: JSON.stringify writes keys in insertion order, and
    // draft-ietf-moq-msf-01 §5.1.7 requires initDataList to follow tracks.
    initDataList: initDataList.length > 0 ? initDataList : undefined
  }
  return JSON.stringify(catalog)
}

/// The Base64 init data of the `initDataList` entry a track's `initRef` names
/// (draft-ietf-moq-msf-01 §5.2.13). A missing or non-`inline` entry returns
/// `undefined` and logs a warning instead of rejecting the catalog.
export function resolveInitData(catalog: unknown, track: Record<string, unknown>): string | undefined {
  const initRef = asString(track.initRef)
  if (!initRef) {
    return undefined
  }
  const initDataList = isObject(catalog) && Array.isArray(catalog.initDataList) ? catalog.initDataList : []
  const entry = initDataList.find((item): item is Record<string, unknown> => isObject(item) && item.id === initRef)
  const data = entry?.type === 'inline' ? asString(entry.data) : undefined
  if (!data) {
    console.warn('[msf] initRef does not resolve to inline init data, so the track has none', {
      track: track.name,
      initRef,
      type: entry?.type
    })
  }
  return data
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
      initData: resolveInitData(catalog, track),
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
