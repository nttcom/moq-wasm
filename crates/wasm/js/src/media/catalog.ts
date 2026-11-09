import { sdkLog } from '../core/log'

export const CATALOG_TRACK_NAME = 'catalog'

/// The catalog `version` (draft-ietf-moq-msf-01 §5.1.1).
const MSF_VERSION = 'draft-01'

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
  altGroup?: number
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
  version?: string
  generatedAt?: number
  isComplete?: boolean
  tracks?: MsfTrack[]
  initDataList?: MsfInitData[]
}

export type CatalogTrackRole = 'video' | 'audio' | 'mediatimeline'

export type MediaCatalogTrack = {
  /// The namespace the track is subscribed under when it is not the catalog's
  /// (draft-ietf-moq-msf-01 §5.2.2), as a `/`-joined tuple.
  namespace?: string
  name: string
  label: string
  packaging?: string
  /// draft-ietf-moq-msf-01 §5.2.5: the type and structure of the records of an
  /// event timeline (§8).
  eventType?: string
  role?: string
  codec?: string
  altGroup?: number
  bitrate?: number
  samplerate?: number
  channelConfig?: string
  width?: number
  height?: number
  initData?: string
  depends?: string[]
  lang?: string
  /// draft-ietf-moq-msf-01 §5.2.7: false when no new objects will be added.
  isLive?: boolean
  /// draft-ietf-moq-msf-01 §5.2.35, in milliseconds; only on a track that is not live.
  trackDuration?: number
}

export function buildMsfCatalogJson(tracks: MsfTrack[], initDataList: MsfInitData[] = []): string {
  const catalog: MsfCatalog = {
    version: MSF_VERSION,
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
    sdkLog('warn', 'moq-wasm::media::catalog', 'initRef names no inline init data, so the track has none', {
      track_name: track.name,
      init_ref: initRef,
      init_data_type: entry?.type
    })
  }
  return data
}

/// One source of a broadcast that has several, such as a camera and a shared
/// screen, named by its video and audio tracks.
export type LiveSource = { video: string; audio?: string }

/// The audio is returned even when the video track is absent, such as a muted
/// camera's. The alternates of the named video and audio (draft-ietf-moq-msf-01
/// §5.2.12), such as renditions or translated audio, belong to the source too.
export function sourceTracks(tracks: MediaCatalogTrack[], source: LiveSource): MediaCatalogTrack[] {
  const named = [source.video, source.audio].flatMap((name) => tracks.find((track) => track.name === name) ?? [])
  const own = tracks.filter(
    (track) =>
      track.packaging !== 'cmaf' &&
      named.some(
        (base) =>
          base.name === track.name ||
          (base.altGroup !== undefined && base.altGroup === track.altGroup && base.role === track.role)
      )
  )
  const cmafNames = new Set(own.map((track) => `${track.name}_cmaf`))
  return [...own, ...tracks.filter((track) => track.packaging === 'cmaf' && cmafNames.has(track.name))]
}

/// The media timeline of the source `trackNames` belong to: each source has
/// its own, which depends on the source's tracks.
export function mediaTimelineOf(catalog: unknown, trackNames: string[]): MediaCatalogTrack | undefined {
  return extractCatalogTracks(catalog, 'mediatimeline').find(({ depends }) =>
    depends?.some((name) => trackNames.includes(name))
  )
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
    if (!matchesRole(role, trackRole, asString(track.packaging), name)) {
      return acc
    }
    acc.push({
      namespace: asString(track.namespace),
      name,
      label: asString(track.label) ?? name,
      packaging: asString(track.packaging),
      eventType: asString(track.eventType),
      role: trackRole,
      codec: asString(track.codec),
      altGroup: asNumber(track.altGroup),
      bitrate: asNumber(track.bitrate),
      samplerate: asNumber(track.samplerate),
      channelConfig: asString(track.channelConfig),
      width: asNumber(track.width),
      height: asNumber(track.height),
      initData: resolveInitData(catalog, track),
      depends: asStrings(track.depends),
      lang: asString(track.lang),
      isLive: typeof track.isLive === 'boolean' ? track.isLive : undefined,
      trackDuration: asNumber(track.trackDuration)
    })
    return acc
  }, [])
}

export function catalogTrackNamespace(track: MediaCatalogTrack, catalogNamespace: string[]): string[] {
  return track.namespace ? track.namespace.split('/') : catalogNamespace
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
