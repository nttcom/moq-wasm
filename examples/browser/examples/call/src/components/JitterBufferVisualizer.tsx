import { ReactNode, useEffect, useState } from 'react'
import { Mic, Video } from 'lucide-react'
import type { JitterBufferSnapshot } from '../types/media'

const VISIBLE_BLOCKS = 30
const PUSH_HIGHLIGHT_MS = 220

type BufferRowProps = {
  label: string
  icon: ReactNode
  snapshot?: JitterBufferSnapshot
  filledClass: string
}

type VideoDiagnostics = {
  targetLatencyMs?: number
  networkLatencyMs?: number
  e2eLatencyMs?: number
  receiveToDecodeMs?: number | null
  receiveToRenderMs?: number | null
  pacingPreset?: string
  pacingPipeline?: string
  pacingEffectiveIntervalMs?: number
  pacingBufferedFrames?: number
  pacingTargetFrames?: number
  decodingGroupId?: string
  decodingObjectId?: string
  decoderCodec?: string
  decoderWidth?: number
  decoderHeight?: number
  decoderAvcFormat?: 'annexb' | 'avc'
  decoderDescriptionBytes?: number
  decoderHardwareAcceleration?: HardwareAcceleration
  decoderOptimizeForLatency?: boolean
}

export function JitterBufferVisualizer({
  videoBuffer,
  audioBuffer,
  videoDiagnostics
}: {
  videoBuffer?: JitterBufferSnapshot
  audioBuffer?: JitterBufferSnapshot
  videoDiagnostics?: VideoDiagnostics
}) {
  if (!videoBuffer && !audioBuffer && !videoDiagnostics) {
    return null
  }

  return (
    <div className="mt-2 space-y-1.5 rounded-md border border-white/10 bg-black/30 p-2">
      <BufferRow
        label="Video"
        icon={<Video className="h-3.5 w-3.5 text-cyan-300" />}
        snapshot={videoBuffer}
        filledClass="bg-cyan-400/85"
      />
      <BufferRow
        label="Audio"
        icon={<Mic className="h-3.5 w-3.5 text-emerald-300" />}
        snapshot={audioBuffer}
        filledClass="bg-emerald-400/85"
      />
      {videoDiagnostics ? <VideoDiagnosticsPanel diagnostics={videoDiagnostics} /> : null}
    </div>
  )
}

function BufferRow({ label, icon, snapshot, filledClass }: BufferRowProps) {
  const [activeEvent, setActiveEvent] = useState<'push' | 'pop' | null>(null)

  useEffect(() => {
    if (!snapshot) {
      setActiveEvent(null)
      return
    }
    setActiveEvent(snapshot.lastEvent)
    const timeoutId = window.setTimeout(() => {
      setActiveEvent((current) => (current === snapshot.lastEvent ? null : current))
    }, PUSH_HIGHLIGHT_MS)
    return () => window.clearTimeout(timeoutId)
  }, [snapshot?.lastEvent, snapshot?.sequence])

  const visibleCapacity = resolveVisibleCapacity(snapshot)
  const filledBlocks = calculateFilledBlocks(snapshot, visibleCapacity)
  const firstFilledIndex = visibleCapacity - filledBlocks

  return (
    <div className="flex items-center gap-2">
      <div className="inline-flex h-4 w-4 items-center justify-center">{icon}</div>
      <div
        className="grid flex-1 grid-flow-col auto-cols-fr gap-[2px]"
        role="img"
        aria-label={`${label} jitter buffer visualization`}
      >
        {Array.from({ length: visibleCapacity }, (_, index) => (
          <span
            key={`${label}-frame-${index}`}
            className={`h-2.5 rounded-[2px] border border-white/15 ${index >= firstFilledIndex ? filledClass : 'bg-white/5'} ${
              activeEvent === 'push' && filledBlocks > 0 && index === firstFilledIndex
                ? 'ring-1 ring-emerald-300/70'
                : activeEvent === 'pop' && index === firstFilledIndex - 1
                  ? 'ring-1 ring-rose-300/70'
                  : ''
            }`}
          />
        ))}
      </div>
    </div>
  )
}

function resolveVisibleCapacity(snapshot?: JitterBufferSnapshot): number {
  if (!snapshot) {
    return VISIBLE_BLOCKS
  }
  return VISIBLE_BLOCKS
}

function calculateFilledBlocks(snapshot: JitterBufferSnapshot | undefined, visibleCapacity: number): number {
  if (!snapshot) {
    return 0
  }
  const bufferedFrames = Math.max(0, Math.floor(snapshot.bufferedFrames))
  return Math.min(visibleCapacity, bufferedFrames)
}

function VideoDiagnosticsPanel({ diagnostics }: { diagnostics: VideoDiagnostics }) {
  const [activeTab, setActiveTab] = useState<'performance' | 'settings'>('performance')
  const decodeToRenderMs =
    typeof diagnostics.receiveToRenderMs === 'number' &&
    Number.isFinite(diagnostics.receiveToRenderMs) &&
    typeof diagnostics.receiveToDecodeMs === 'number' &&
    Number.isFinite(diagnostics.receiveToDecodeMs)
      ? Math.max(0, diagnostics.receiveToRenderMs - diagnostics.receiveToDecodeMs)
      : undefined

  const performanceItems = [
    {
      label: 'Target latency',
      value: formatMs(diagnostics.targetLatencyMs)
    },
    {
      label: 'E2E Latency',
      value: formatMs(diagnostics.e2eLatencyMs)
    },
    {
      label: 'NW latency',
      value: formatMs(diagnostics.networkLatencyMs)
    },
    {
      label: 'Recv→Decode',
      value: formatMs(diagnostics.receiveToDecodeMs)
    },
    {
      label: 'Decode→Render',
      value: formatMs(decodeToRenderMs)
    },
    {
      label: 'Pacing interval',
      value: formatMs(diagnostics.pacingEffectiveIntervalMs)
    },
    {
      label: 'Pacing buffer',
      value:
        typeof diagnostics.pacingBufferedFrames === 'number' || typeof diagnostics.pacingTargetFrames === 'number'
          ? `${formatNum(diagnostics.pacingBufferedFrames)} / ${formatNum(diagnostics.pacingTargetFrames)} frames`
          : '-'
    },
    {
      label: 'Pacing mode',
      value:
        diagnostics.pacingPreset || diagnostics.pacingPipeline
          ? `${diagnostics.pacingPreset ?? '-'} / ${diagnostics.pacingPipeline ?? '-'}`
          : '-'
    },
    {
      label: 'Decoding group',
      value: diagnostics.decodingGroupId ?? '-'
    },
    {
      label: 'Decoding object',
      value: diagnostics.decodingObjectId ?? '-'
    }
  ]

  const settingsItems = [
    {
      label: 'Codec',
      value: diagnostics.decoderCodec ?? '-'
    },
    {
      label: 'Resolution',
      value:
        typeof diagnostics.decoderWidth === 'number' && typeof diagnostics.decoderHeight === 'number'
          ? `${Math.round(diagnostics.decoderWidth)}x${Math.round(diagnostics.decoderHeight)}`
          : '-'
    },
    {
      label: 'H264 format',
      value: diagnostics.decoderAvcFormat ?? '-'
    },
    {
      label: 'Description bytes',
      value: formatNum(diagnostics.decoderDescriptionBytes)
    },
    {
      label: 'HW Accel',
      value: diagnostics.decoderHardwareAcceleration ?? '-'
    },
    {
      label: 'Optimize latency',
      value:
        typeof diagnostics.decoderOptimizeForLatency === 'boolean'
          ? diagnostics.decoderOptimizeForLatency
            ? 'true'
            : 'false'
          : '-'
    }
  ]

  return (
    <div className="space-y-2 pt-1 text-[10px] text-blue-100/90">
      <div className="inline-flex rounded-md border border-white/10 bg-white/[0.03] p-0.5">
        <button
          type="button"
          className={`rounded px-2 py-1 text-[10px] font-medium transition ${
            activeTab === 'performance' ? 'bg-blue-500/80 text-white' : 'text-blue-200 hover:bg-white/10'
          }`}
          onClick={() => setActiveTab('performance')}
        >
          Performance
        </button>
        <button
          type="button"
          className={`rounded px-2 py-1 text-[10px] font-medium transition ${
            activeTab === 'settings' ? 'bg-blue-500/80 text-white' : 'text-blue-200 hover:bg-white/10'
          }`}
          onClick={() => setActiveTab('settings')}
        >
          Settings
        </button>
      </div>
      <div className="grid grid-cols-1 gap-1 sm:grid-cols-2">
        {(activeTab === 'performance' ? performanceItems : settingsItems).map((item) => (
          <div key={item.label} className="flex items-center justify-between gap-2 rounded bg-white/[0.03] px-2 py-1">
            <span className="text-blue-200/90">{item.label}</span>
            <span className="font-mono text-white/95">{item.value}</span>
          </div>
        ))}
      </div>
    </div>
  )
}

function formatMs(value: number | null | undefined): string {
  if (typeof value !== 'number' || !Number.isFinite(value)) {
    return '-'
  }
  return `${Math.round(value)} ms`
}

function formatNum(value: number | null | undefined): string {
  if (typeof value !== 'number' || !Number.isFinite(value)) {
    return '-'
  }
  return `${Math.round(value)}`
}
