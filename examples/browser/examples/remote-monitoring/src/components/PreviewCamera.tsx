import { CameraId } from '../types/monitoring'
import { PlayerHost } from './PlayerHost'

interface Props {
  cameraId: CameraId
  host: HTMLElement
  onClick: () => void
}

export function PreviewCamera({ cameraId, host, onClick }: Props) {
  return (
    <div
      className="relative flex flex-col justify-end overflow-hidden rounded-md cursor-pointer ring-1 ring-zinc-700 hover:ring-blue-400 transition-shadow aspect-video w-full"
      onClick={onClick}
    >
      <div className="absolute inset-0 bg-zinc-800">
        <PlayerHost host={host} />
      </div>
      <div className="relative z-10 flex items-center justify-between px-2 py-1.5 bg-zinc-900/80">
        <span className="text-xs text-zinc-300">{cameraId}</span>
        <span className="h-1.5 w-1.5 rounded-full bg-green-400" />
      </div>
    </div>
  )
}
