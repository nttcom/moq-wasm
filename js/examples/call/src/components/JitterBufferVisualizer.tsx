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

export function JitterBufferVisualizer({
  videoBuffer,
  audioBuffer
}: {
  videoBuffer?: JitterBufferSnapshot
  audioBuffer?: JitterBufferSnapshot
}) {
  if (!videoBuffer && !audioBuffer) {
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
