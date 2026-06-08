import { CameraId } from '../types/monitoring'
import { ThumbCamera } from './ThumbCamera'

interface Props {
  cameraIds: CameraId[]
  onSelect: (id: CameraId) => void
  onCanvasReady?: (camId: CameraId) => (canvas: HTMLCanvasElement | null) => void
  onSubscribe?: (camId: CameraId) => void
  subscribedCameras?: Set<CameraId>
  subscribingCameras?: Set<CameraId>
}

export function ThumbStrip({ cameraIds, onSelect, onCanvasReady, onSubscribe, subscribedCameras, subscribingCameras }: Props) {
  if (cameraIds.length === 0) return null

  return (
    <div className="flex flex-col gap-2.5 w-[200px]">
      <p className="font-mono text-xs text-center text-zinc-500">
        他カメラ（クリックで主役に）
      </p>
      {cameraIds.map((id) => (
        <ThumbCamera
          key={id}
          cameraId={id}
          onCanvasReady={onCanvasReady?.(id)}
          onSubscribe={onSubscribe ? () => onSubscribe(id) : undefined}
          isSubscribed={subscribedCameras?.has(id)}
          isSubscribing={subscribingCameras?.has(id)}
          onClick={() => onSelect(id)}
        />
      ))}
    </div>
  )
}
