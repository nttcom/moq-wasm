import type { VideoEncodingOptions } from 'moq-wasm/media'
import { getResolvedMediaVideoCodec } from './catalog'
import { getMediaVideoHardwareAcceleration } from './common'

const DEFAULT_WIDTH = 1280
const DEFAULT_HEIGHT = 720
const DEFAULT_FRAMERATE = 30
const BITRATE = 1_000_000

export function buildVideoEncoding(stream: MediaStream): VideoEncodingOptions {
  const settings = stream.getVideoTracks()[0]?.getSettings() ?? {}
  return {
    encoding: {
      codec: getResolvedMediaVideoCodec(),
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
