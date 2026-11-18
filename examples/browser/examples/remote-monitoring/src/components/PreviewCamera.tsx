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
        onStage ? 'ring-2 ring-accent' : 'cursor-pointer ring-1 ring-line-strong hover:ring-accent'
      )}
      onClick={onStage ? undefined : onClick}
    >
      <div className="absolute inset-0 bg-neutral-950">
        {onStage ? (
          <div className="absolute inset-0 flex items-center justify-center text-xs font-medium text-teal-300">
            メインに表示中
          </div>
        ) : (
          <PlayerHost host={host} />
        )}
      </div>
      <div className="relative z-10 flex items-center justify-between bg-gradient-to-t from-black/80 to-transparent px-2 py-1.5">
        <span className="truncate text-xs text-white">{cameraId}</span>
        <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-emerald-400" />
      </div>
    </div>
  )
}
