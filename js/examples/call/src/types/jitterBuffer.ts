import type { VideoJitterBufferMode } from '../../../../utils/media/videoJitterBuffer'

export type VideoJitterConfig = {
  mode: VideoJitterBufferMode
  minDelayMs: number
  bufferedAheadFrames: number
}

export const DEFAULT_VIDEO_JITTER_CONFIG: VideoJitterConfig = {
  mode: 'buffered',
  minDelayMs: 250,
  bufferedAheadFrames: 5
}

export function normalizeVideoJitterConfig(config: Partial<VideoJitterConfig>): VideoJitterConfig {
  const mode = config.mode ?? DEFAULT_VIDEO_JITTER_CONFIG.mode
  const minDelayMs = Math.max(
    0,
    Number.isFinite(config.minDelayMs) ? Number(config.minDelayMs) : DEFAULT_VIDEO_JITTER_CONFIG.minDelayMs
  )
  const bufferedAheadFrames = Math.max(
    1,
    Number.isFinite(config.bufferedAheadFrames)
      ? Math.floor(Number(config.bufferedAheadFrames))
      : DEFAULT_VIDEO_JITTER_CONFIG.bufferedAheadFrames
  )
  return { mode, minDelayMs, bufferedAheadFrames }
}
