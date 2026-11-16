import type { VideoEncodingOptions } from 'moq-wasm/media'
import { getMediaVideoCodecOverride, getMediaVideoHardwareAcceleration } from './common'

const DEFAULT_CODEC = 'avc1.640028' // H.264 High Profile Level 4.0
const DEFAULT_WIDTH = 1280
const DEFAULT_HEIGHT = 720
const DEFAULT_FRAMERATE = 30
const BITRATE = 1_000_000

export function buildVideoEncoding(stream: MediaStream): VideoEncodingOptions {
  const settings = stream.getVideoTracks()[0]?.getSettings() ?? {}
  return {
    encoding: {
      codec: getMediaVideoCodecOverride() ?? DEFAULT_CODEC,
      hardwareAcceleration: getMediaVideoHardwareAcceleration()
    },
    main: {
      width: typeof settings.width === 'number' ? settings.width : DEFAULT_WIDTH,
      height: typeof settings.height === 'number' ? settings.height : DEFAULT_HEIGHT,
      bitrate: BITRATE,
      framerate: typeof settings.frameRate === 'number' ? settings.frameRate : DEFAULT_FRAMERATE
    }
  }
}
