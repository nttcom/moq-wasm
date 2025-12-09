import type { VideoJitterBufferMode } from '../../../../utils/media/videoJitterBuffer'
import { DEFAULT_VIDEO_JITTER_CONFIG, type VideoJitterConfig } from '../types/jitterBuffer'

const MODES: VideoJitterBufferMode[] = ['buffered', 'normal', 'correctly', 'fast']

interface JitterBufferControlsProps {
  value?: VideoJitterConfig
  onChange: (value: Partial<VideoJitterConfig>) => void
}

export function JitterBufferControls({ value, onChange }: JitterBufferControlsProps) {
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
      <div className="text-xs font-semibold uppercase tracking-wide text-blue-100">Jitter Buffer</div>
      <label className="flex items-center justify-between gap-3 text-sm">
        <span className="text-blue-50">Mode</span>
        <select
          value={config.mode}
          onChange={(e) => handleModeChange(e.target.value as VideoJitterBufferMode)}
          className="w-36 rounded-md border border-white/10 bg-white/10 px-2 py-1 text-sm text-white outline-none focus:border-blue-300 focus:ring-1 focus:ring-blue-300"
        >
          {MODES.map((mode) => (
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
