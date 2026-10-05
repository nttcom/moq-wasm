import { CameraId } from '../types/monitoring'
import { ThumbCamera } from './ThumbCamera'

interface Props {
  cameraIds: CameraId[]
  hostOf: (camId: CameraId) => HTMLElement
  onSelect: (id: CameraId) => void
  onSubscribe?: (camId: CameraId) => void
  subscribedCameras?: Set<CameraId>
  subscribingCameras?: Set<CameraId>
}

export function ThumbStrip({ cameraIds, hostOf, onSelect, onSubscribe, subscribedCameras, subscribingCameras }: Props) {
  if (cameraIds.length === 0) return null

  return (
    <div className="flex flex-col gap-2.5 w-[200px]">
      <p className="font-mono text-xs text-center text-zinc-500">他カメラ（クリックで主役に）</p>
      {cameraIds.map((id) => (
        <ThumbCamera
          key={id}
          cameraId={id}
          host={hostOf(id)}
          onSubscribe={onSubscribe ? () => onSubscribe(id) : undefined}
          isSubscribed={subscribedCameras?.has(id)}
          isSubscribing={subscribingCameras?.has(id)}
          onClick={() => onSelect(id)}
        />
      ))}
    </div>
  )
}
