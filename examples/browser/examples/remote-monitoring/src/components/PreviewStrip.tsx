import { CameraId } from '../types/monitoring'
import { PreviewCamera } from './PreviewCamera'

interface Props {
  cameraIds: CameraId[]
  hostOf: (camId: CameraId) => HTMLElement
  onSelect: (id: CameraId) => void
}

export function PreviewStrip({ cameraIds, hostOf, onSelect }: Props) {
  return (
    <div className="flex flex-col gap-2.5 w-[200px]">
      <p className="font-mono text-xs text-center text-zinc-500">他のカメラ（クリックでステージへ）</p>
      {cameraIds.map((id) => (
        <PreviewCamera key={id} cameraId={id} host={hostOf(id)} onClick={() => onSelect(id)} />
      ))}
      {cameraIds.length === 0 && <p className="font-mono text-xs text-center text-zinc-600">なし</p>}
    </div>
  )
}
