import { cn } from '../utils/cn'
import { CameraId, MonitorMode, ConnState } from '../types/monitoring'

interface Props {
  cameraId: CameraId
  mode: MonitorMode
  connState: ConnState
  currentGroupId: bigint | null
  latestGroupId: bigint | null
  onCanvasReady?: (canvas: HTMLCanvasElement | null) => void
  onSubscribe?: () => void
  isSubscribed?: boolean
  isSubscribing?: boolean
  onClick: () => void
}

export function StageCamera({
  cameraId,
  mode,
  connState,
  currentGroupId,
  latestGroupId,
  onCanvasReady,
  onSubscribe,
  isSubscribed,
  isSubscribing,
  onClick
}: Props) {
  const isReview = mode === 'review'
  const isUnhealthy = connState !== 'connected'

  const delaySeconds = currentGroupId != null && latestGroupId != null ? Number(latestGroupId - currentGroupId) : null
  const timestamp =
    isReview && delaySeconds != null
      ? `−${String(Math.floor(delaySeconds / 60)).padStart(2, '0')}:${String(delaySeconds % 60).padStart(2, '0')}`
      : 'NOW'

  return (
    <div
      className={cn(
        'relative flex flex-col justify-end overflow-hidden rounded-lg cursor-pointer select-none',
        'aspect-video w-full',
        isReview ? 'ring-2 ring-amber-500' : 'ring-1 ring-zinc-700'
      )}
      onClick={onClick}
    >
      {/* video feed */}
      <div
        className="absolute inset-0 bg-zinc-900"
        style={{
          backgroundImage: 'repeating-linear-gradient(45deg, rgba(255,255,255,.03) 0 9px, transparent 9px 18px)'
        }}
      >
        <canvas ref={onCanvasReady} className="absolute inset-0 w-full h-full object-cover" />
        {!isSubscribed && (
          <div className="absolute inset-0 flex items-center justify-center pointer-events-none">
            <span className="font-mono text-sm text-zinc-500">
              {connState === 'connected' ? '「視聴」で映像を受信' : '接続 → 視聴 で映像を受信できます'}
            </span>
          </div>
        )}
      </div>

      {/* connection banner */}
      {isUnhealthy && isSubscribed && (
        <div className="absolute inset-0 z-10 flex items-center justify-center bg-black/60 font-mono text-white">
          <span className="text-base">⚠ 接続が切れました</span>
        </div>
      )}

      {/* LIVE / REVIEW badge */}
      <div className="absolute top-3 left-3 z-10">
        {isReview ? (
          <span className="flex items-center gap-1.5 rounded-md border border-amber-500 bg-amber-500/10 px-2.5 py-1 font-mono text-xs font-bold text-amber-400">
            <span className="h-1.5 w-1.5 rounded-full bg-amber-400 animate-pulse" />
            REVIEW
          </span>
        ) : (
          <span className="flex items-center gap-1.5 rounded-md border border-green-500 bg-green-500/10 px-2.5 py-1 font-mono text-xs font-bold text-green-400">
            <span className="h-1.5 w-1.5 rounded-full bg-green-400 animate-pulse" />
            LIVE
          </span>
        )}
      </div>

      {/* click hint */}
      {!isReview && isSubscribed && (
        <div className="absolute bottom-11 right-3 z-10 font-mono text-xs text-zinc-500 bg-zinc-900/70 rounded px-2 py-0.5">
          クリックで過去を見る
        </div>
      )}

      {/* meta row */}
      <div className="relative z-10 flex items-center justify-between px-3 py-2.5 bg-gradient-to-t from-zinc-900/90 to-transparent">
        <span className="text-sm text-white">{cameraId.toUpperCase()}</span>
        <div className="flex items-center gap-2">
          {onSubscribe && !isSubscribed ? (
            <button
              onClick={(e) => {
                e.stopPropagation()
                onSubscribe()
              }}
              disabled={isSubscribing}
              className="rounded border border-green-500 bg-green-500/20 px-2.5 py-1 font-mono text-xs font-bold text-green-400 hover:bg-green-500/30 disabled:opacity-50"
            >
              {isSubscribing ? '接続中…' : '視聴'}
            </button>
          ) : (
            <span className="font-mono text-xs text-zinc-400">{timestamp}</span>
          )}
        </div>
      </div>
    </div>
  )
}
