import { CameraId } from '../types/monitoring'

interface Props {
  cameraId: CameraId
  onCanvasReady?: (canvas: HTMLCanvasElement | null) => void
  onSubscribe?: () => void
  isSubscribed?: boolean
  isSubscribing?: boolean
  onClick: () => void
}

export function ThumbCamera({ cameraId, onCanvasReady, onSubscribe, isSubscribed, isSubscribing, onClick }: Props) {
  return (
    <div
      className="relative flex flex-col justify-end overflow-hidden rounded-md cursor-pointer ring-1 ring-zinc-700 hover:ring-blue-400 transition-shadow aspect-video w-full"
      onClick={onClick}
    >
      {/* video feed */}
      <div
        className="absolute inset-0 bg-zinc-800"
        style={{
          backgroundImage: 'repeating-linear-gradient(45deg, rgba(255,255,255,.03) 0 6px, transparent 6px 12px)'
        }}
      >
        <canvas ref={onCanvasReady} className="absolute inset-0 w-full h-full object-cover" />
        <div className="absolute inset-0 flex items-center justify-center pointer-events-none">
          <span className="font-mono text-xs text-zinc-600">{cameraId.toUpperCase()}</span>
        </div>
      </div>

      {/* meta row */}
      <div className="relative z-10 flex items-center justify-between px-2 py-1.5 bg-zinc-900/80">
        <span className="text-xs text-zinc-300">{cameraId.toUpperCase()}</span>
        {onSubscribe && !isSubscribed ? (
          <button
            onClick={(e) => {
              e.stopPropagation()
              onSubscribe()
            }}
            disabled={isSubscribing}
            className="rounded border border-green-500 bg-green-500/20 px-2 py-0.5 font-mono text-xs font-bold text-green-400 hover:bg-green-500/30 disabled:opacity-50"
          >
            {isSubscribing ? '…' : '視聴'}
          </button>
        ) : (
          <span className={`h-1.5 w-1.5 rounded-full ${isSubscribed ? 'bg-green-400' : 'bg-zinc-600'}`} />
        )}
      </div>
    </div>
  )
}
