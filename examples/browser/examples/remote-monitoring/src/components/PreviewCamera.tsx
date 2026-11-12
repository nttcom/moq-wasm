import { cn } from '../../../../lib/ui/classNames'
import { CameraId } from '../types/monitoring'
import { PlayerHost } from './PlayerHost'

interface Props {
  cameraId: CameraId
  host: HTMLElement
  /// The camera's picture is on the stage, so this slot only marks where the camera is.
  onStage: boolean
  onClick: () => void
}

export function PreviewCamera({ cameraId, host, onStage, onClick }: Props) {
  return (
    <div
      className={cn(
        'relative flex flex-col justify-end overflow-hidden rounded-md transition-shadow aspect-video w-full',
        onStage ? 'ring-2 ring-blue-400' : 'cursor-pointer ring-1 ring-zinc-700 hover:ring-blue-400'
      )}
      onClick={onStage ? undefined : onClick}
    >
      <div className="absolute inset-0 bg-zinc-800">
        {onStage ? (
          <div className="absolute inset-0 flex items-center justify-center font-mono text-xs text-blue-300">
            ステージに表示中
          </div>
        ) : (
          <PlayerHost host={host} />
        )}
      </div>
      <div className="relative z-10 flex items-center justify-between px-2 py-1.5 bg-zinc-900/80">
        <span className="text-xs text-zinc-300">{cameraId}</span>
        <span className="h-1.5 w-1.5 rounded-full bg-green-400" />
      </div>
    </div>
  )
}
