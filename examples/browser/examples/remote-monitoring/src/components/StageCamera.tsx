import { History } from 'lucide-react'
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
        'relative flex flex-col justify-end overflow-hidden rounded-xl cursor-pointer select-none shadow-md',
        'aspect-video w-full',
        isReview ? 'ring-2 ring-amber-400' : 'ring-1 ring-line'
      )}
      onClick={onClick}
    >
      {/* video feed */}
      <div
        className="absolute inset-0 bg-neutral-950"
        style={{
          backgroundImage: 'repeating-linear-gradient(45deg, rgba(255,255,255,.03) 0 9px, transparent 9px 18px)'
        }}
      >
        {host && <PlayerHost host={host} />}
        {!cameraId && (
          <div className="absolute inset-0 flex items-center justify-center pointer-events-none">
            <span className="text-sm text-neutral-400">
              {connState === 'connected' ? 'カメラを待っています' : '接続するとカメラが現れます'}
            </span>
          </div>
        )}
      </div>

      {/* banners */}
      {isUnhealthy && cameraId && (
        <div className="absolute inset-0 z-10 flex items-center justify-center bg-black/60 text-white backdrop-blur-[2px]">
          <span className="text-base font-medium">⚠ 接続が切れました</span>
        </div>
      )}
      {lost && !isUnhealthy && (
        <div className="absolute inset-0 z-10 flex items-center justify-center bg-black/60 text-white backdrop-blur-[2px]">
          <span className="text-base font-medium">カメラが切断しました。他のカメラを選んでください</span>
        </div>
      )}

      {/* LIVE / REVIEW badge */}
      {cameraId && !lost && (
        <div className="absolute top-3 left-3 z-10">
          {isReview ? (
            <span className="flex items-center gap-1.5 rounded-md border border-amber-400/60 bg-black/55 px-2.5 py-1 font-mono text-xs font-bold text-amber-300 backdrop-blur-sm">
              <span className="h-1.5 w-1.5 rounded-full bg-amber-300 animate-pulse" />
              REVIEW
            </span>
          ) : (
            <span className="flex items-center gap-1.5 rounded-md border border-emerald-400/60 bg-black/55 px-2.5 py-1 font-mono text-xs font-bold text-emerald-300 backdrop-blur-sm">
              <span className="h-1.5 w-1.5 rounded-full bg-emerald-300 animate-pulse" />
              LIVE
            </span>
          )}
        </div>
      )}

      {/* click hint */}
      {!isReview && cameraId && !lost && (
        <div
          className="absolute bottom-11 right-3 z-10 flex items-center gap-1 rounded bg-black/55 px-2 py-0.5 text-xs text-white/80 backdrop-blur-sm"
          title="クリックか ↓ で過去を見る"
        >
          <History className="h-3.5 w-3.5" />↓
        </div>
      )}

      {/* meta row */}
      <div className="relative z-10 flex items-center justify-between px-3 py-2.5 bg-gradient-to-t from-black/80 to-transparent">
        <span className="text-sm font-medium text-white">{cameraId ?? ''}</span>
        <span className="font-mono text-xs text-white/70">{cameraId ? timestamp : ''}</span>
      </div>
    </div>
  )
}
