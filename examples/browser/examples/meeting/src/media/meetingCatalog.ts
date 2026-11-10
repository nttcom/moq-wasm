import type { Rendition } from 'moq-wasm/media'
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
