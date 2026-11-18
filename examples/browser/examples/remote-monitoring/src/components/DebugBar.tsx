import { Cctv, History } from 'lucide-react'
import { cn } from '../../../../lib/ui/classNames'
import { CameraId, ConnState } from '../types/monitoring'

interface Props {
  connState: ConnState
  relayUrl: string
  cameras: CameraId[]
  reviewingCamera: CameraId | null
  reviewStatus: string | null
}

const connLabel: Record<ConnState, string> = {
  connected: 'CONNECTED',
  closed: 'CLOSED'
}

const connDotClass: Record<ConnState, string> = {
  connected: 'bg-success',
  closed: 'bg-danger'
}

const connTextClass: Record<ConnState, string> = {
  connected: 'text-success',
  closed: 'text-danger'
}

export function DebugBar({ connState, relayUrl, cameras, reviewingCamera, reviewStatus }: Props) {
  const isUnhealthy = connState !== 'connected'

  return (
    <div className="mt-auto space-y-2 rounded-xl border border-line bg-surface px-4 py-3 font-mono text-[13px] text-fg shadow-sm">
      {/* connection row */}
      <div className="flex items-center gap-2">
        <span className={cn('h-2.5 w-2.5 rounded-full', connDotClass[connState])} />
        <span className={cn('font-semibold', connTextClass[connState])}>{connLabel[connState]}</span>
        <span className="text-faint">— {relayUrl}</span>
      </div>

      {/* cameras row */}
      <div className="flex items-center gap-2 flex-wrap">
        <span className="flex shrink-0 items-center text-faint" title="配信中のカメラ">
          <Cctv className="h-4 w-4" aria-label="カメラ" />
        </span>
        {cameras.length > 0 ? (
          cameras.map((id) => (
            <span
              key={id}
              className={cn(
                'rounded-full border px-2 py-0.5 text-xs',
                isUnhealthy ? 'border-line-strong text-faint line-through opacity-50' : 'border-success/40 text-success'
              )}
            >
              {id}
            </span>
          ))
        ) : (
          <span className="text-faint">—</span>
        )}
      </div>

      {/* review row */}
      <div className="flex items-center gap-2 flex-wrap">
        <span className="flex shrink-0 items-center text-faint" title="REVIEW 中のカメラ">
          <History className="h-4 w-4" aria-label="REVIEW" />
        </span>
        {reviewingCamera && reviewStatus ? (
          <span
            className={cn(
              'rounded-full border px-2 py-0.5 text-xs',
              isUnhealthy ? 'border-line-strong text-faint line-through opacity-50' : 'border-warning/40 text-warning'
            )}
          >
            {reviewingCamera} [ {reviewStatus} ]
          </span>
        ) : (
          <span className="text-faint">—</span>
        )}
      </div>
    </div>
  )
}
