import type { VideoJitterBufferMode } from '../../../../utils/media/videoJitterBuffer'
import type { AudioJitterBufferMode } from '../../../../utils/media/audioJitterBuffer'
import {
  DEFAULT_VIDEO_JITTER_CONFIG,
  DEFAULT_AUDIO_JITTER_CONFIG,
  type VideoJitterConfig,
  type AudioJitterConfig
} from '../types/jitterBuffer'

const VIDEO_MODES: VideoJitterBufferMode[] = ['buffered', 'normal', 'correctly', 'fast']
const AUDIO_MODES: AudioJitterBufferMode[] = ['ordered', 'latest']

interface VideoJitterBufferControlsProps {
  value?: VideoJitterConfig
  onChange: (value: Partial<VideoJitterConfig>) => void
}

interface AudioJitterBufferControlsProps {
  value?: AudioJitterConfig
  onChange: (value: Partial<AudioJitterConfig>) => void
}

export function VideoJitterBufferControls({ value, onChange }: VideoJitterBufferControlsProps) {
  const config = value ?? DEFAULT_VIDEO_JITTER_CONFIG

  const handleModeChange = (mode: VideoJitterBufferMode) => onChange({ mode })
  const handleMinDelayChange = (minDelayMs: number) => {
    if (!Number.isFinite(minDelayMs)) return
    onChange({ minDelayMs: Math.max(0, minDelayMs) })
  }
  const handleBufferedFramesChange = (frames: number) => {
    if (!Number.isFinite(frames)) return
    onChange({ bufferedAheadFrames: Math.max(1, Math.floor(frames)) })
  }

  const showMinDelay = config.mode === 'normal' || config.mode === 'correctly'
  const showBufferedAhead = config.mode === 'buffered'

  return (
    <div className="space-y-2 rounded-lg border border-white/10 bg-white/5 p-3">
      <div className="text-xs font-semibold uppercase tracking-wide text-blue-100">Video Jitter Buffer</div>
      <label className="flex items-center justify-between gap-3 text-sm">
        <span className="text-blue-50">Mode</span>
        <select
          value={config.mode}
          onChange={(e) => handleModeChange(e.target.value as VideoJitterBufferMode)}
          className="w-36 rounded-md border border-white/10 bg-white/10 px-2 py-1 text-sm text-white outline-none focus:border-blue-300 focus:ring-1 focus:ring-blue-300"
        >
          {VIDEO_MODES.map((mode) => (
            <option key={mode} value={mode} className="bg-slate-900 text-white">
              {mode}
            </option>
          ))}
        </select>
      </label>
      {showMinDelay && (
        <label className="flex items-center justify-between gap-3 text-sm">
          <span className="text-blue-50">Min delay (ms)</span>
          <input
            type="number"
            min={0}
            value={config.minDelayMs}
            onChange={(e) => handleMinDelayChange(Number(e.target.value))}
            className="w-24 rounded-md border border-white/10 bg-white/10 px-2 py-1 text-right text-sm text-white outline-none focus:border-blue-300 focus:ring-1 focus:ring-blue-300"
          />
        </label>
      )}
      {showBufferedAhead && (
        <label className="flex items-center justify-between gap-3 text-sm">
          <span className="text-blue-50">Buffered ahead</span>
          <input
            type="number"
            min={1}
            value={config.bufferedAheadFrames}
            onChange={(e) => handleBufferedFramesChange(Number(e.target.value))}
            className="w-24 rounded-md border border-white/10 bg-white/10 px-2 py-1 text-right text-sm text-white outline-none focus:border-blue-300 focus:ring-1 focus:ring-blue-300"
          />
        </label>
      )}
    </div>
  )
}

export function AudioJitterBufferControls({ value, onChange }: AudioJitterBufferControlsProps) {
  const config = value ?? DEFAULT_AUDIO_JITTER_CONFIG

  const handleModeChange = (mode: AudioJitterBufferMode) => onChange({ mode })

  return (
    <div className="space-y-2 rounded-lg border border-white/10 bg-white/5 p-3">
      <div className="text-xs font-semibold uppercase tracking-wide text-blue-100">Audio Jitter Buffer</div>
      <label className="flex items-center justify-between gap-3 text-sm">
        <span className="text-blue-50">Mode</span>
        <select
          value={config.mode}
          onChange={(e) => handleModeChange(e.target.value as AudioJitterBufferMode)}
          className="w-36 rounded-md border border-white/10 bg-white/10 px-2 py-1 text-sm text-white outline-none focus:border-blue-300 focus:ring-1 focus:ring-blue-300"
        >
          {AUDIO_MODES.map((mode) => (
            <option key={mode} value={mode} className="bg-slate-900 text-white">
              {mode}
            </option>
          ))}
        </select>
      </label>
    </div>
  )
}
