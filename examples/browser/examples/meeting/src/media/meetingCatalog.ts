import { parse_msf_catalog_json } from 'moq-wasm/internal/wasm'
import type { Rendition } from 'moq-wasm/media/publish'
import { resolveInitData, type MsfTrack } from 'moq-wasm/media/catalog'
import {
  DEFAULT_VIDEO_KEYFRAME_INTERVAL,
  type MeetingCatalogTrack,
  type CatalogTrackRole,
  type CatalogSubscribeRole
} from '../types/catalog'
import { isScreenShareTrackName } from '../utils/catalogTrackName'
import { DEFAULT_VIDEO_ENCODING_SETTINGS, type VideoEncodingSettings } from '../types/videoEncoding'

type E2ECameraSettings = {
  width: number
  height: number
  framerate: number
  bitrate: number
  codec: string | null
  /// A second, lower rendition of the same codec for the track switch tests.
  lowHeight: number | null
}

function readPositiveIntegerParam(params: URLSearchParams, name: string): number | null {
  const raw = params.get(name)
  const parsed = raw ? Number(raw) : NaN
  return Number.isInteger(parsed) && parsed > 0 ? parsed : null
}

function readNonEmptyStringParam(params: URLSearchParams, name: string): string | null {
  const value = params.get(name)?.trim()
  return value ? value : null
}

function readWindowSearchParams(): URLSearchParams | null {
  if (typeof window === 'undefined') {
    return null
  }
  return new URLSearchParams(window.location.search)
}

// E2E hook: `?keyframeInterval=N` shortens the camera GOP so new subscribers get a
// renderable keyframe quickly. Without the param the normal default is used.
function resolveCameraKeyframeInterval(): number {
  const params = readWindowSearchParams()
  if (!params) {
    return DEFAULT_VIDEO_KEYFRAME_INTERVAL
  }
  return readPositiveIntegerParam(params, 'keyframeInterval') ?? DEFAULT_VIDEO_KEYFRAME_INTERVAL
}

function resolveE2ECameraSettings(): E2ECameraSettings | null {
  const params = readWindowSearchParams()
  if (!params) {
    return null
  }
  const width = readPositiveIntegerParam(params, 'e2eVideoWidth')
  const height = readPositiveIntegerParam(params, 'e2eVideoHeight')
  const framerate = readPositiveIntegerParam(params, 'e2eVideoFramerate')
  const bitrate = readPositiveIntegerParam(params, 'e2eVideoBitrate')
  const codec = readNonEmptyStringParam(params, 'e2eVideoCodec')
  const lowHeight = readPositiveIntegerParam(params, 'e2eVideoLowHeight')
  if (!width || !height || !framerate || !bitrate) {
    return null
  }
  return { width, height, framerate, bitrate, codec, lowHeight }
}

const CHAT_TRACK_NAME = 'chat'

export const CAMERA_KEYFRAME_INTERVAL = resolveCameraKeyframeInterval()
export const E2E_CAMERA_SETTINGS = resolveE2ECameraSettings()

export const INITIAL_CAMERA_ENCODING_SETTINGS: VideoEncodingSettings = E2E_CAMERA_SETTINGS
  ? {
      ...DEFAULT_VIDEO_ENCODING_SETTINGS,
      width: E2E_CAMERA_SETTINGS.width,
      height: E2E_CAMERA_SETTINGS.height,
      framerate: E2E_CAMERA_SETTINGS.framerate,
      bitrate: E2E_CAMERA_SETTINGS.bitrate,
      ...(E2E_CAMERA_SETTINGS.codec ? { codec: E2E_CAMERA_SETTINGS.codec } : {})
    }
  : DEFAULT_VIDEO_ENCODING_SETTINGS

/// The camera's lower renditions. An E2E run gets one rendition
/// `e2eVideoLowHeight` high at half the bitrate, and none without that parameter.
export const CAMERA_RENDITIONS: Rendition[] = E2E_CAMERA_SETTINGS
  ? E2E_CAMERA_SETTINGS.lowHeight
    ? [
        {
          name: 'low',
          label: 'Low',
          width:
            Math.round((E2E_CAMERA_SETTINGS.width * E2E_CAMERA_SETTINGS.lowHeight) / E2E_CAMERA_SETTINGS.height / 2) *
            2,
          height: E2E_CAMERA_SETTINGS.lowHeight,
          bitrate: E2E_CAMERA_SETTINGS.bitrate / 2
        }
      ]
    : []
  : [
      { name: '720p', label: '720p', width: 1280, height: 720, bitrate: 500_000 },
      { name: '480p', label: '480p', width: 854, height: 480, bitrate: 200_000 }
    ]

export const SCREEN_SHARE_RENDITIONS: Rendition[] = [
  { name: '720p', label: 'Screen 720p', width: 1280, height: 720, bitrate: 1_200_000 },
  { name: '480p', label: 'Screen 480p', width: 854, height: 480, bitrate: 700_000 }
]

export function parseMeetingCatalogTracks(payload: string): MeetingCatalogTrack[] {
  const catalog: { tracks?: MsfTrack[] } = parse_msf_catalog_json(payload)
  return (catalog.tracks ?? []).flatMap((track) => {
    const role = resolveRole(track)
    if (!role) {
      return []
    }
    return [
      {
        name: track.name,
        label: track.label ?? track.name,
        role,
        codec: track.codec,
        bitrate: track.bitrate,
        width: track.width,
        height: track.height,
        framerate: track.framerate,
        samplerate: track.samplerate,
        channelConfig: track.channelConfig,
        initData: resolveInitData(catalog, track),
        isLive: track.isLive
      }
    ]
  })
}

export function catalogTracksForRole<T extends MeetingCatalogTrack>(tracks: T[], role: CatalogSubscribeRole): T[] {
  if (role === 'audio' || role === 'chat') {
    return tracks.filter((track) => track.role === role)
  }
  return tracks.filter(
    (track) => track.role === 'video' && isScreenShareTrackName(track.name) === (role === 'screenshare')
  )
}

export function pickHighestBitrateTrack<T extends MeetingCatalogTrack>(tracks: T[]): T | undefined {
  return tracksByBitrate(tracks).at(-1) ?? tracks[0]
}

export function pickLowestBitrateTrack<T extends MeetingCatalogTrack>(tracks: T[]): T | undefined {
  return tracksByBitrate(tracks)[0] ?? tracks[0]
}

function tracksByBitrate<T extends MeetingCatalogTrack>(tracks: T[]): T[] {
  return tracks.filter((track) => typeof track.bitrate === 'number').sort((a, b) => (a.bitrate ?? 0) - (b.bitrate ?? 0))
}

function resolveRole({ role, name }: MsfTrack): CatalogTrackRole | null {
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
