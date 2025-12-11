import type { VideoJitterBufferMode } from '../../../../utils/media/videoJitterBuffer'
import type { AudioJitterBufferMode } from '../../../../utils/media/audioJitterBuffer'

export type VideoJitterConfig = {
  mode: VideoJitterBufferMode
  minDelayMs: number
  bufferedAheadFrames: number
}

export const DEFAULT_VIDEO_JITTER_CONFIG: VideoJitterConfig = {
  mode: 'fast',
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

export type AudioJitterConfig = {
  mode: AudioJitterBufferMode
}

export const DEFAULT_AUDIO_JITTER_CONFIG: AudioJitterConfig = {
  mode: 'latest'
}

export function normalizeAudioJitterConfig(config: Partial<AudioJitterConfig>): AudioJitterConfig {
  const mode = config.mode ?? DEFAULT_AUDIO_JITTER_CONFIG.mode
  return { mode }
}
