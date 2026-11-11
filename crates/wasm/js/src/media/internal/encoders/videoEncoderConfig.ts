import { isHevcCodec } from '../hevc'

/// `hevc` is the member of the WebCodecs HEVC registration, which TypeScript's
/// DOM types do not carry.
export type VideoEncoderSettings = Omit<VideoEncoderConfig, 'width' | 'height'> &
  Partial<Pick<VideoEncoderConfig, 'width' | 'height'>> & { hevc?: { format: 'annexb' } }

export type RenditionEncoderConfig = Partial<VideoEncoderConfig> & { scaleResolutionDownBy?: number }

const DEFAULT_FRAMERATE = 30

export type VideoCodecName = 'h264' | 'h265' | 'vp8' | 'vp9' | 'av1'

const CODEC_STRINGS: Record<VideoCodecName, string> = {
  h264: 'avc1.640028', // High Profile, level 4.0 and raised to fit the encoded size
  h265: 'hvc1.1.6.L93.B0', // Main Profile; the encoder writes the level the encoded size needs
  vp8: 'vp8',
  vp9: 'vp09.00.10.08', // Profile 0, 8-bit
  av1: 'av01.0.08M.08' // Main Profile, level 4.0, 8-bit
}

/// The WebCodecs codec string of `codec`: a name, or a codec string as given.
export function codecString(codec: string): string {
  return Object.hasOwn(CODEC_STRINGS, codec) ? CODEC_STRINGS[codec as VideoCodecName] : codec
}

const DEFAULT_CONFIG: VideoEncoderSettings = {
  codec: CODEC_STRINGS.h264,
  bitrate: 1_000_000,
  framerate: DEFAULT_FRAMERATE,
  // Chrome tries only a hardware encoder for 'prefer-hardware', so a machine
  // without one cannot encode at all.
  hardwareAcceleration: 'no-preference',
  // The hardware encoder of an M1 Mac supports only L1T1.
  scalabilityMode: 'L1T1',
  latencyMode: 'realtime'
}

/// `config` over `DEFAULT_CONFIG`, skipping fields set to `undefined`. H.264
/// and H.265 are always Annex B, the form the broadcast reads, and the frame
/// rate is clamped to 1–120.
export function videoEncoderConfig(config: Partial<VideoEncoderConfig> = {}): VideoEncoderSettings {
  const set = Object.fromEntries(Object.entries(config).filter(([, value]) => value !== undefined))
  const merged: VideoEncoderSettings = { ...DEFAULT_CONFIG, ...set }
  const codec = codecString(merged.codec)
  return {
    ...merged,
    codec,
    avc: codec.startsWith('avc') ? { format: 'annexb' } : undefined,
    hevc: isHevcCodec(codec) ? { format: 'annexb' } : undefined,
    framerate:
      typeof merged.framerate === 'number' && Number.isFinite(merged.framerate) && merged.framerate > 0
        ? Math.min(120, Math.max(1, Math.round(merged.framerate)))
        : DEFAULT_CONFIG.framerate
  }
}

export const DEFAULT_KEYFRAME_INTERVAL_SECONDS = 30

/// The frame rate a track set to `framerate` is capped at, as the encoder
/// worker clamps it; undefined when it is not set, as the worker then encodes
/// every frame.
export function framerateCap(framerate: number | undefined): number | undefined {
  return framerate === undefined ? undefined : videoEncoderConfig({ framerate }).framerate
}
