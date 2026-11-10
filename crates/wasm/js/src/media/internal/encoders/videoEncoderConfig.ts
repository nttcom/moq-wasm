export type VideoEncoderSettings = Omit<VideoEncoderConfig, 'width' | 'height'> &
  Partial<Pick<VideoEncoderConfig, 'width' | 'height'>>

export type RenditionEncoderConfig = Partial<VideoEncoderConfig> & { scaleResolutionDownBy?: number }

const DEFAULT_FRAMERATE = 30

const DEFAULT_CONFIG: VideoEncoderSettings = {
  codec: 'avc1.640028', // H.264 High Profile Level 4.0
  bitrate: 1_000_000,
  framerate: DEFAULT_FRAMERATE,
  hardwareAcceleration: 'prefer-hardware',
  // The hardware encoder of an M1 Mac supports only L1T1.
  scalabilityMode: 'L1T1',
  latencyMode: 'realtime'
}

/// `config` over `DEFAULT_CONFIG`, skipping fields set to `undefined`. H.264 is
/// always Annex B, the form the broadcast reads, and the frame rate is clamped
/// to 1–120.
export function videoEncoderConfig(config: Partial<VideoEncoderConfig> = {}): VideoEncoderSettings {
  const set = Object.fromEntries(Object.entries(config).filter(([, value]) => value !== undefined))
  const merged: VideoEncoderSettings = { ...DEFAULT_CONFIG, ...set }
  return {
    ...merged,
    avc: merged.codec.startsWith('avc') ? { format: 'annexb' } : undefined,
    framerate:
      typeof merged.framerate === 'number' && Number.isFinite(merged.framerate) && merged.framerate > 0
        ? Math.min(120, Math.max(1, Math.round(merged.framerate)))
        : DEFAULT_CONFIG.framerate
  }
}

export const DEFAULT_KEYFRAME_INTERVAL_SECONDS = 30
