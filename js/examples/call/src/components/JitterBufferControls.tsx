import type { AudioJitterBufferMode } from '../../../../utils/media/audioJitterBuffer'
import {
  createVideoPacingPresetConfig,
  DEFAULT_VIDEO_PACING_CONFIG,
  DEFAULT_VIDEO_JITTER_CONFIG,
  DEFAULT_AUDIO_JITTER_CONFIG,
  type VideoJitterConfig,
  type AudioJitterConfig,
  type VideoPacingConfig,
  type VideoPacingPreset,
  type VideoDecoderHardwareAcceleration
} from '../types/jitterBuffer'

const AUDIO_MODES: AudioJitterBufferMode[] = ['ordered', 'latest']
const VIDEO_PACING_PRESETS: VideoPacingPreset[] = ['disabled', 'call', 'onvif']
const VIDEO_DECODER_HWA_OPTIONS: VideoDecoderHardwareAcceleration[] = ['prefer-hardware', 'prefer-software']

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
  const handlePacingPresetChange = (preset: VideoPacingPreset) => {
    const next = createVideoPacingPresetConfig(preset)
    onChange({ pacing: next })
  }
  const handleTargetLatencyChange = (targetLatencyMs: number) => {
    if (!Number.isFinite(targetLatencyMs)) return
    const next = {
      ...(config.pacing ?? DEFAULT_VIDEO_PACING_CONFIG),
      targetLatencyMs: Math.max(0, targetLatencyMs)
    } satisfies VideoPacingConfig
    onChange({ pacing: next })
  }
  const handleDecoderHardwareAccelerationChange = (decoderHardwareAcceleration: VideoDecoderHardwareAcceleration) => {
    onChange({ decoderHardwareAcceleration })
  }
  return (
    <div className="space-y-2 rounded-lg border border-white/10 bg-white/5 p-3">
      <div className="text-xs font-semibold uppercase tracking-wide text-blue-100">Video Jitter Buffer</div>
      <label className="grid gap-2 text-sm">
        <span className="text-blue-50">Pacing preset</span>
        <select
          value={config.pacing?.preset ?? DEFAULT_VIDEO_PACING_CONFIG.preset}
          onChange={(e) => handlePacingPresetChange(e.target.value as VideoPacingPreset)}
          className="w-full rounded-md border border-white/10 bg-white/10 px-2 py-1 text-sm text-white outline-none focus:border-blue-300 focus:ring-1 focus:ring-blue-300"
        >
          {VIDEO_PACING_PRESETS.map((preset) => (
            <option key={preset} value={preset} className="bg-slate-900 text-white">
              {preset}
            </option>
          ))}
        </select>
      </label>
      <div className="text-[11px] text-blue-200">Pacing pipeline: buffer-pacing-decode (fixed)</div>
      <label className="flex items-center justify-between gap-3 text-sm">
        <span className="text-blue-50">Target latency (ms)</span>
        <input
          type="number"
          min={0}
          value={config.pacing?.targetLatencyMs ?? 0}
          onChange={(e) => handleTargetLatencyChange(Number(e.target.value))}
          className="w-24 rounded-md border border-white/10 bg-white/10 px-2 py-1 text-right text-sm text-white outline-none focus:border-blue-300 focus:ring-1 focus:ring-blue-300"
        />
      </label>
      <label className="grid gap-2 text-sm">
        <span className="text-blue-50">Decoder HW accel</span>
        <select
          value={config.decoderHardwareAcceleration ?? DEFAULT_VIDEO_JITTER_CONFIG.decoderHardwareAcceleration}
          onChange={(e) => handleDecoderHardwareAccelerationChange(e.target.value as VideoDecoderHardwareAcceleration)}
          className="w-full rounded-md border border-white/10 bg-white/10 px-2 py-1 text-sm text-white outline-none focus:border-blue-300 focus:ring-1 focus:ring-blue-300"
        >
          {VIDEO_DECODER_HWA_OPTIONS.map((value) => (
            <option key={value} value={value} className="bg-slate-900 text-white">
              {value}
            </option>
          ))}
        </select>
      </label>
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
