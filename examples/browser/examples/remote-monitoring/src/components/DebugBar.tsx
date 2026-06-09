import { cn } from '../utils/cn'
import { CameraId, ConnState } from '../types/monitoring'

interface Props {
  connState: ConnState
  relayUrl: string
  subscribedCameras: CameraId[]
  fetchingCamera: CameraId | null
  fetchWindow: { startGroup: bigint; endGroup: bigint } | null
}

const connLabel: Record<ConnState, string> = {
  connected: 'CONNECTED',
  closed: 'CLOSED'
}

const connDotClass: Record<ConnState, string> = {
  connected: 'bg-green-500',
  closed: 'bg-red-500'
}

const connTextClass: Record<ConnState, string> = {
  connected: 'text-green-400',
  closed: 'text-red-400'
}

export function DebugBar({ connState, relayUrl, subscribedCameras, fetchingCamera, fetchWindow }: Props) {
  const isUnhealthy = connState !== 'connected'

  return (
    <div className="mt-4 rounded-xl bg-zinc-900 px-4 py-3 font-mono text-sm text-zinc-200 space-y-1.5">
      {/* connection row */}
      <div className="flex items-center gap-2">
        <span className={cn('h-2.5 w-2.5 rounded-full', connDotClass[connState])} />
        <span className={cn('font-semibold', connTextClass[connState])}>{connLabel[connState]}</span>
        <span className="text-zinc-500">— {relayUrl}</span>
      </div>

      {/* subscribe row */}
      <div className="flex items-center gap-2 flex-wrap">
        <span className="text-blue-400 w-24 shrink-0">SUBSCRIBE</span>
        {subscribedCameras.length > 0 ? (
          subscribedCameras.map((id) => (
            <span
              key={id}
              className={cn(
                'rounded-full border px-2 py-0.5 text-xs',
                isUnhealthy
                  ? 'border-zinc-700 text-zinc-600 line-through opacity-50'
                  : 'border-green-800 text-green-400'
              )}
            >
              {id}
            </span>
          ))
        ) : (
          <span className="text-zinc-600">—</span>
        )}
      </div>

      {/* fetch row */}
      <div className="flex items-center gap-2 flex-wrap">
        <span className="text-amber-400 w-24 shrink-0">FETCH</span>
        {fetchingCamera && fetchWindow ? (
          <span
            className={cn(
              'rounded-full border px-2 py-0.5 text-xs',
              isUnhealthy ? 'border-zinc-700 text-zinc-600 line-through opacity-50' : 'border-amber-800 text-amber-400'
            )}
          >
            {fetchingCamera} [ group {fetchWindow.startGroup.toString()} → {fetchWindow.endGroup.toString()} ]
          </span>
        ) : (
          <span className="text-zinc-600">—</span>
        )}
      </div>
    </div>
  )
}
