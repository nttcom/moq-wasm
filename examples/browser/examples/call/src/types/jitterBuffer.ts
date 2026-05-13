import type { AudioJitterBufferMode } from '../../../../utils/media/audioJitterBuffer'

export type VideoPacingConfig = {
  preset: VideoPacingPreset
  pipeline: VideoPacingPipeline
  fallbackIntervalMs: number
  lateThresholdMs: number
  maxWaitMs: number
  reportIntervalMs: number
  targetLatencyMs: number
}

export type VideoPacingPreset = 'disabled' | 'onvif' | 'call'
export type VideoPacingPipeline = 'buffer-pacing-decode'
export type VideoDecoderHardwareAcceleration = 'prefer-hardware' | 'prefer-software'

export function createVideoPacingPresetConfig(preset: VideoPacingPreset): VideoPacingConfig {
  if (preset === 'disabled') {
    return {
      ...createVideoPacingPresetConfig('onvif'),
      preset: 'disabled',
      pipeline: 'buffer-pacing-decode',
      targetLatencyMs: 0
    }
  }
  if (preset === 'onvif') {
    return {
      preset: 'onvif',
      pipeline: 'buffer-pacing-decode',
      fallbackIntervalMs: 33,
      lateThresholdMs: 120,
      maxWaitMs: 2000,
      reportIntervalMs: 500,
      targetLatencyMs: 250
    }
  }
  return {
    ...createVideoPacingPresetConfig('onvif'),
    preset: 'call',
    pipeline: 'buffer-pacing-decode',
    targetLatencyMs: 250,
    maxWaitMs: 1000,
    lateThresholdMs: 500
  }
}

export type VideoJitterConfig = {
  pacing: VideoPacingConfig
  decoderHardwareAcceleration: VideoDecoderHardwareAcceleration
}

export const DEFAULT_VIDEO_PACING_CONFIG: VideoPacingConfig = createVideoPacingPresetConfig('call')

export const DEFAULT_VIDEO_JITTER_CONFIG: VideoJitterConfig = {
  pacing: { ...DEFAULT_VIDEO_PACING_CONFIG },
  decoderHardwareAcceleration: 'prefer-software'
}

function clampNumber(value: unknown, fallback: number, min: number, max = Number.POSITIVE_INFINITY): number {
  if (typeof value !== 'number' || !Number.isFinite(value)) {
    return fallback
  }
  return Math.min(max, Math.max(min, value))
}

function normalizeVideoPacingConfig(config?: Partial<VideoPacingConfig>): VideoPacingConfig {
  const requestedPreset =
    config?.preset === 'disabled' || config?.preset === 'onvif' || config?.preset === 'call' ? config.preset : undefined
  const fallbackPreset =
    DEFAULT_VIDEO_PACING_CONFIG.preset === 'disabled' ||
    DEFAULT_VIDEO_PACING_CONFIG.preset === 'onvif' ||
    DEFAULT_VIDEO_PACING_CONFIG.preset === 'call'
      ? DEFAULT_VIDEO_PACING_CONFIG.preset
      : 'call'
  const preset = requestedPreset ?? fallbackPreset
  const merged: Partial<VideoPacingConfig> = {
    ...createVideoPacingPresetConfig(preset),
    ...(config ?? {}),
    preset
  }
  return {
    preset,
    pipeline: 'buffer-pacing-decode',
    fallbackIntervalMs: Math.round(
      clampNumber(merged.fallbackIntervalMs, DEFAULT_VIDEO_PACING_CONFIG.fallbackIntervalMs, 1, 1000)
    ),
    lateThresholdMs: clampNumber(merged.lateThresholdMs, DEFAULT_VIDEO_PACING_CONFIG.lateThresholdMs, 0, 10_000),
    maxWaitMs: clampNumber(merged.maxWaitMs, DEFAULT_VIDEO_PACING_CONFIG.maxWaitMs, 1, 10_000),
    reportIntervalMs: Math.round(
      clampNumber(merged.reportIntervalMs, DEFAULT_VIDEO_PACING_CONFIG.reportIntervalMs, 50, 10_000)
    ),
    targetLatencyMs: clampNumber(merged.targetLatencyMs, DEFAULT_VIDEO_PACING_CONFIG.targetLatencyMs, 0, 30_000)
  }
}

export function normalizeVideoJitterConfig(config: Partial<VideoJitterConfig>): VideoJitterConfig {
  const pacing = normalizeVideoPacingConfig(config.pacing)
  const decoderHardwareAcceleration =
    config.decoderHardwareAcceleration === 'prefer-software' || config.decoderHardwareAcceleration === 'prefer-hardware'
      ? config.decoderHardwareAcceleration
      : DEFAULT_VIDEO_JITTER_CONFIG.decoderHardwareAcceleration
  return { pacing, decoderHardwareAcceleration }
}

export type AudioJitterConfig = {
  mode: AudioJitterBufferMode
}

export const DEFAULT_AUDIO_JITTER_CONFIG: AudioJitterConfig = {
  mode: 'ordered'
}

export function normalizeAudioJitterConfig(config: Partial<AudioJitterConfig>): AudioJitterConfig {
  const mode = config.mode ?? DEFAULT_AUDIO_JITTER_CONFIG.mode
  return { mode }
}
