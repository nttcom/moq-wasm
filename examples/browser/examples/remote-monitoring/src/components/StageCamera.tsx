import { cn } from '../../../../lib/ui/classNames'
import { CameraId, MonitorMode, ConnState } from '../types/monitoring'
import { PlayerHost } from './PlayerHost'
import { formatBehindLive } from './SeekBar'

interface Props {
  /// Undefined until the location's first camera is announced.
  cameraId: CameraId | undefined
  host: HTMLElement | undefined
  mode: MonitorMode
  connState: ConnState
  /// The camera's publisher withdrew it; it stays on the stage until another camera is chosen.
  lost: boolean
  behindSeconds: number | null
  onClick: () => void
}

export function StageCamera({ cameraId, host, mode, connState, lost, behindSeconds, onClick }: Props) {
  const isReview = mode === 'review'
  const isUnhealthy = connState !== 'connected'

  const timestamp = isReview && behindSeconds != null ? formatBehindLive(behindSeconds) : 'NOW'

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
        {host && <PlayerHost host={host} />}
        {!cameraId && (
          <div className="absolute inset-0 flex items-center justify-center pointer-events-none">
            <span className="font-mono text-sm text-zinc-500">
              {connState === 'connected' ? 'カメラを待っています' : '接続するとカメラが現れます'}
            </span>
          </div>
        )}
      </div>

      {/* banners */}
      {isUnhealthy && cameraId && (
        <div className="absolute inset-0 z-10 flex items-center justify-center bg-black/60 font-mono text-white">
          <span className="text-base">⚠ 接続が切れました</span>
        </div>
      )}
      {lost && !isUnhealthy && (
        <div className="absolute inset-0 z-10 flex items-center justify-center bg-black/60 font-mono text-white">
          <span className="text-base">カメラが切断しました。他のカメラを選んでください</span>
        </div>
      )}

      {/* LIVE / REVIEW badge */}
      {cameraId && !lost && (
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
      )}

      {/* click hint */}
      {!isReview && cameraId && !lost && (
        <div className="absolute bottom-11 right-3 z-10 font-mono text-xs text-zinc-500 bg-zinc-900/70 rounded px-2 py-0.5">
          クリックで過去を見る
        </div>
      )}

      {/* meta row */}
      <div className="relative z-10 flex items-center justify-between px-3 py-2.5 bg-gradient-to-t from-zinc-900/90 to-transparent">
        <span className="text-sm text-white">{cameraId ?? ''}</span>
        <span className="font-mono text-xs text-zinc-400">{cameraId ? timestamp : ''}</span>
      </div>
    </div>
  )
}
