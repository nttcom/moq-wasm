import { getResolvedMediaVideoCodec } from './catalog'
import { getMediaVideoEncodingOverrides } from './common'

const DEFAULT_WIDTH = 1280
const DEFAULT_HEIGHT = 720
const DEFAULT_FRAMERATE = 30
const BITRATE = 1_000_000

export function buildVideoEncoderConfig(stream: MediaStream): VideoEncoderConfig {
  const settings = stream.getVideoTracks()[0]?.getSettings() ?? {}
  const { hardwareAcceleration } = getMediaVideoEncodingOverrides()
  return {
    codec: getResolvedMediaVideoCodec(),
    width: typeof settings.width === 'number' ? settings.width : DEFAULT_WIDTH,
    height: typeof settings.height === 'number' ? settings.height : DEFAULT_HEIGHT,
    bitrate: BITRATE,
    framerate: typeof settings.frameRate === 'number' ? settings.frameRate : DEFAULT_FRAMERATE,
    hardwareAcceleration
  }
}
