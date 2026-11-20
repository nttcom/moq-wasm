import { isHevcCodec } from '../hevc'

export function videoDecoderConfig(codec: string, description?: Uint8Array): VideoDecoderConfig {
  return {
    codec,
    description,
    // Chrome decodes H.265 only in hardware.
    hardwareAcceleration: isHevcCodec(codec) ? 'no-preference' : 'prefer-software',
    optimizeForLatency: true
  }
}
