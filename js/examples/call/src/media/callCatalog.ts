import { parse_msf_catalog_json } from '../../../../pkg/moqt_client_wasm'
import {
  DEFAULT_AUDIO_STREAM_UPDATE_SETTINGS,
  DEFAULT_VIDEO_KEYFRAME_INTERVAL,
  type AudioStreamUpdateMode,
  type CallCatalogTrack,
  type CatalogTrackRole,
  type EditableCallCatalogTrack
} from '../types/catalog'

const CAMERA_PROFILES = [
  { id: 'base', label: 'Base', codec: 'avc1.42001F', maxEncodePixels: 1280 * 720 },
  { id: 'main', label: 'Main', codec: 'avc1.4D4028', maxEncodePixels: Number.POSITIVE_INFINITY },
  { id: 'high', label: 'High', codec: 'avc1.640028', maxEncodePixels: Number.POSITIVE_INFINITY }
] as const

const CAMERA_RESOLUTIONS = [
  { id: '1080p', label: '1080p', width: 1920, height: 1080, bitrate: 1_000_000 },
  { id: '720p', label: '720p', width: 1280, height: 720, bitrate: 500_000 },
  { id: '480p', label: '480p', width: 854, height: 480, bitrate: 200_000 }
] as const

const CAMERA_CATALOG_TRACKS: CallCatalogTrack[] = CAMERA_PROFILES.flatMap((profile) =>
  CAMERA_RESOLUTIONS.filter((resolution) => resolution.width * resolution.height <= profile.maxEncodePixels).map(
    (resolution) => ({
      name: `camera_${profile.id}_${resolution.id}`,
      label: `Camera ${profile.label} ${resolution.label}`,
      role: 'video' as const,
      codec: profile.codec,
      width: resolution.width,
      height: resolution.height,
      bitrate: resolution.bitrate,
      framerate: 30,
      hardwareAcceleration: 'prefer-software',
      keyframeInterval: DEFAULT_VIDEO_KEYFRAME_INTERVAL,
      isLive: true
    })
  )
)

const DEFAULT_CALL_CATALOG_TRACKS: CallCatalogTrack[] = []

const SCREENSHARE_CATALOG_TRACKS: CallCatalogTrack[] = [
  {
    name: 'screenshare_1080p',
    label: 'Screen 1080p',
    role: 'video',
    codec: 'av01.0.08M.08',
    width: 1920,
    height: 1080,
    bitrate: 2_000_000,
    framerate: 30,
    hardwareAcceleration: 'prefer-software',
    isLive: true
  },
  {
    name: 'screenshare_720p',
    label: 'Screen 720p',
    role: 'video',
    codec: 'av01.0.08M.08',
    width: 1280,
    height: 720,
    bitrate: 1_200_000,
    framerate: 30,
    hardwareAcceleration: 'prefer-software',
    isLive: true
  },
  {
    name: 'screenshare_480p',
    label: 'Screen 480p',
    role: 'video',
    codec: 'av01.0.08M.08',
    width: 854,
    height: 480,
    bitrate: 700_000,
    framerate: 30,
    hardwareAcceleration: 'prefer-software',
    isLive: true
  }
]

const AUDIO_CATALOG_TRACKS: CallCatalogTrack[] = [
  {
    name: 'audio_128kbps',
    label: 'Audio 128kbps',
    role: 'audio',
    codec: 'opus',
    bitrate: 128_000,
    samplerate: 48_000,
    channelConfig: 'mono',
    isLive: true
  },
  {
    name: 'audio_64kbps',
    label: 'Audio 64kbps',
    role: 'audio',
    codec: 'opus',
    bitrate: 64_000,
    samplerate: 48_000,
    channelConfig: 'mono',
    isLive: true
  },
  {
    name: 'audio_32kbps',
    label: 'Audio 32kbps',
    role: 'audio',
    codec: 'opus',
    bitrate: 32_000,
    samplerate: 48_000,
    channelConfig: 'mono',
    isLive: true
  }
]

const CHAT_TRACK_NAME = 'chat'
const CHAT_TRACK_LABEL = 'Chat'
const CHAT_EVENT_TYPE = 'com.skyway.chat.v1'

type MsfTrack = {
  namespace?: string
  name: string
  packaging: string
  eventType?: string
  role?: string
  isLive?: boolean
  label?: string
  codec?: string
  mimeType?: string
  depends?: string[]
  bitrate?: number
  width?: number
  height?: number
  framerate?: number
  hardwareAcceleration?: HardwareAcceleration
  samplerate?: number
  channelConfig?: string
}

type MsfCatalog = {
  version?: number
  generatedAt?: number
  isComplete?: boolean
  tracks?: MsfTrack[]
}

export function getDefaultCallCatalogTracks(): CallCatalogTrack[] {
  return DEFAULT_CALL_CATALOG_TRACKS.map((track) => ({ ...track }))
}

export function getCameraCatalogTracks(): CallCatalogTrack[] {
  return CAMERA_CATALOG_TRACKS.map((track) => ({ ...track }))
}

export function getScreenShareCatalogTracks(): CallCatalogTrack[] {
  return SCREENSHARE_CATALOG_TRACKS.map((track) => ({ ...track }))
}

export function getAudioCatalogTracks(): CallCatalogTrack[] {
  return AUDIO_CATALOG_TRACKS.map((track) => ({ ...track }))
}

export function buildCallCatalogJson(trackNamespace: string[], tracks: CallCatalogTrack[]): string {
  const namespace = trackNamespace.length > 0 ? trackNamespace.join('/') : undefined
  const catalogTracks: MsfTrack[] = tracks.map((track) => ({
    namespace,
    name: track.name,
    packaging: 'loc',
    role: track.role,
    isLive: track.isLive ?? true,
    label: track.label || track.name,
    codec: track.codec,
    bitrate: track.bitrate,
    width: track.width,
    height: track.height,
    framerate: track.framerate,
    hardwareAcceleration: track.hardwareAcceleration,
    samplerate: track.samplerate,
    channelConfig: track.channelConfig
  }))
  const existingNames = new Set(catalogTracks.map((track) => track.name))
  if (!existingNames.has(CHAT_TRACK_NAME)) {
    const depends = tracks.map((track) => track.name)
    catalogTracks.push({
      namespace,
      name: CHAT_TRACK_NAME,
      packaging: 'eventtimeline',
      role: 'chat',
      isLive: true,
      label: CHAT_TRACK_LABEL,
      mimeType: 'application/json',
      eventType: CHAT_EVENT_TYPE,
      depends
    })
  }
  const catalog: MsfCatalog = {
    version: 1,
    generatedAt: Date.now(),
    isComplete: true,
    tracks: catalogTracks
  }
  return JSON.stringify(catalog)
}

export function parseCallCatalogTracks(payload: string): CallCatalogTrack[] {
  const parsed = parse_msf_catalog_json(payload)
  return extractCallCatalogTracks(parsed)
}

export function extractCallCatalogTracks(catalog: unknown): CallCatalogTrack[] {
  const tracks = Array.isArray((catalog as { tracks?: unknown[] } | undefined)?.tracks)
    ? (catalog as { tracks: unknown[] }).tracks ?? []
    : []
  return tracks.reduce<CallCatalogTrack[]>((acc, rawTrack) => {
    if (!isObject(rawTrack)) {
      return acc
    }
    const name = asString(rawTrack.name)
    if (!name) {
      return acc
    }
    const role = resolveRole(rawTrack, name)
    if (!role) {
      return acc
    }
    acc.push({
      name,
      label: asString(rawTrack.label) ?? name,
      role,
      codec: asString(rawTrack.codec),
      bitrate: asNumber(rawTrack.bitrate),
      width: asNumber(rawTrack.width),
      height: asNumber(rawTrack.height),
      framerate: asNumber(rawTrack.framerate),
      hardwareAcceleration: asHardwareAcceleration(rawTrack.hardwareAcceleration),
      samplerate: asNumber(rawTrack.samplerate),
      channelConfig: asString(rawTrack.channelConfig),
      isLive: asBoolean(rawTrack.isLive)
    })
    return acc
  }, [])
}

export function toEditableCatalogTracks(tracks: CallCatalogTrack[]): EditableCallCatalogTrack[] {
  return tracks.map((track) => ({
    ...track,
    id: createCatalogTrackId()
  }))
}

export function toCatalogTracks(tracks: EditableCallCatalogTrack[]): CallCatalogTrack[] {
  return tracks.map((track) => sanitizeTrack(track)).filter((track): track is CallCatalogTrack => track !== null)
}

export function appendCatalogTracks(
  existingTracks: EditableCallCatalogTrack[],
  tracksToAppend: CallCatalogTrack[]
): EditableCallCatalogTrack[] {
  const existingNames = new Set(
    existingTracks
      .map((track) => track.name.trim())
      .filter((name): name is string => typeof name === 'string' && name.length > 0)
  )
  const appended: EditableCallCatalogTrack[] = [...existingTracks]
  let changed = false

  for (const track of tracksToAppend) {
    const normalizedName = track.name.trim()
    if (!normalizedName || existingNames.has(normalizedName)) {
      continue
    }
    changed = true
    existingNames.add(normalizedName)
    appended.push({
      ...track,
      name: normalizedName,
      label: track.label.trim() || normalizedName,
      id: createCatalogTrackId()
    })
  }

  return changed ? appended : existingTracks
}

export function removeCatalogTracksByNames(
  tracks: EditableCallCatalogTrack[],
  trackNames: string[]
): EditableCallCatalogTrack[] {
  const names = new Set(
    trackNames.map((name) => name.trim()).filter((name): name is string => typeof name === 'string' && name.length > 0)
  )
  if (!names.size) {
    return tracks
  }
  let changed = false
  const next = tracks.filter((track) => {
    const shouldRemove = names.has(track.name.trim())
    if (shouldRemove) {
      changed = true
    }
    return !shouldRemove
  })
  return changed ? next : tracks
}

export function getCameraCatalogTrackNames(): string[] {
  return CAMERA_CATALOG_TRACKS.map((track) => track.name)
}

export function getScreenShareCatalogTrackNames(): string[] {
  return SCREENSHARE_CATALOG_TRACKS.map((track) => track.name)
}

export function getAudioCatalogTrackNames(): string[] {
  return AUDIO_CATALOG_TRACKS.map((track) => track.name)
}

export function createEmptyEditableCatalogTrack(role: CatalogTrackRole): EditableCallCatalogTrack {
  if (role === 'chat') {
    const name = `chat_${Date.now()}`
    return {
      id: createCatalogTrackId(),
      name,
      label: 'chat',
      role: 'chat',
      codec: undefined,
      bitrate: undefined,
      width: undefined,
      height: undefined,
      framerate: undefined,
      hardwareAcceleration: undefined,
      keyframeInterval: undefined,
      samplerate: undefined,
      channelConfig: undefined,
      audioStreamUpdateMode: undefined,
      audioStreamUpdateIntervalSeconds: undefined,
      isLive: true
    }
  }
  return {
    id: createCatalogTrackId(),
    name: role === 'video' ? `video_${Date.now()}` : `audio_${Date.now()}`,
    label: role,
    role,
    codec: role === 'video' ? 'avc1.42E01E' : 'opus',
    bitrate: role === 'video' ? 800_000 : 64_000,
    width: role === 'video' ? 1280 : undefined,
    height: role === 'video' ? 720 : undefined,
    framerate: role === 'video' ? 30 : undefined,
    hardwareAcceleration: role === 'video' ? 'prefer-software' : undefined,
    keyframeInterval: role === 'video' ? DEFAULT_VIDEO_KEYFRAME_INTERVAL : undefined,
    samplerate: role === 'audio' ? 48_000 : undefined,
    channelConfig: role === 'audio' ? 'mono' : undefined,
    audioStreamUpdateMode: role === 'audio' ? DEFAULT_AUDIO_STREAM_UPDATE_SETTINGS.mode : undefined,
    audioStreamUpdateIntervalSeconds:
      role === 'audio' ? DEFAULT_AUDIO_STREAM_UPDATE_SETTINGS.intervalSeconds : undefined,
    isLive: true
  }
}

export function createEmptyEditableScreenShareCatalogTrack(): EditableCallCatalogTrack {
  return {
    id: createCatalogTrackId(),
    name: `screenshare_${Date.now()}`,
    label: 'screenshare',
    role: 'video',
    codec: 'av01.0.08M.08',
    bitrate: 1_200_000,
    width: 1280,
    height: 720,
    framerate: 30,
    hardwareAcceleration: 'prefer-software',
    keyframeInterval: DEFAULT_VIDEO_KEYFRAME_INTERVAL,
    samplerate: undefined,
    channelConfig: undefined,
    audioStreamUpdateMode: undefined,
    audioStreamUpdateIntervalSeconds: undefined,
    isLive: true
  }
}

export function createCatalogTrackId(): string {
  return `catalog-track-${Date.now()}-${Math.random().toString(16).slice(2, 10)}`
}

function sanitizeTrack(track: EditableCallCatalogTrack): CallCatalogTrack | null {
  const name = track.name.trim()
  if (!name) {
    return null
  }
  const label = track.label.trim() || name
  return {
    name,
    label,
    role: track.role,
    codec: track.codec?.trim() || undefined,
    bitrate: toPositiveNumber(track.bitrate),
    width: track.role === 'video' ? toPositiveNumber(track.width) : undefined,
    height: track.role === 'video' ? toPositiveNumber(track.height) : undefined,
    framerate: track.role === 'video' ? toPositiveNumber(track.framerate) : undefined,
    hardwareAcceleration: track.role === 'video' ? asHardwareAcceleration(track.hardwareAcceleration) : undefined,
    keyframeInterval:
      track.role === 'video' ? toPositiveNumber(track.keyframeInterval) ?? DEFAULT_VIDEO_KEYFRAME_INTERVAL : undefined,
    samplerate: track.role === 'audio' ? toPositiveNumber(track.samplerate) : undefined,
    channelConfig: track.role === 'audio' ? track.channelConfig?.trim() || undefined : undefined,
    audioStreamUpdateMode:
      track.role === 'audio' ? normalizeAudioStreamUpdateMode(track.audioStreamUpdateMode) : undefined,
    audioStreamUpdateIntervalSeconds:
      track.role === 'audio'
        ? toPositiveNumber(track.audioStreamUpdateIntervalSeconds) ??
          DEFAULT_AUDIO_STREAM_UPDATE_SETTINGS.intervalSeconds
        : undefined,
    isLive: track.isLive ?? true
  }
}

function resolveRole(track: Record<string, unknown>, name: string): CatalogTrackRole | null {
  const role = asString(track.role)
  if (role === 'chat' || name === CHAT_TRACK_NAME) {
    return 'chat'
  }
  if (role === 'video' || role === 'audio') {
    return role
  }
  if (name.startsWith('video')) {
    return 'video'
  }
  if (name.startsWith('audio')) {
    return 'audio'
  }
  return null
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

function asBoolean(value: unknown): boolean | undefined {
  return typeof value === 'boolean' ? value : undefined
}

function asHardwareAcceleration(value: unknown): HardwareAcceleration | undefined {
  if (value === 'prefer-hardware' || value === 'prefer-software' || value === 'no-preference') {
    return value
  }
  return undefined
}

function toPositiveNumber(value: number | undefined): number | undefined {
  if (typeof value !== 'number' || !Number.isFinite(value) || value <= 0) {
    return undefined
  }
  return Math.floor(value)
}

function normalizeAudioStreamUpdateMode(value: AudioStreamUpdateMode | undefined): AudioStreamUpdateMode {
  return value === 'single' ? 'single' : DEFAULT_AUDIO_STREAM_UPDATE_SETTINGS.mode
}
