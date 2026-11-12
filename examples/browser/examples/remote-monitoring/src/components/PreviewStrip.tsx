import { useEffect, useState } from 'react'
import { CameraId } from '../types/monitoring'
import { PreviewCamera } from './PreviewCamera'

const PAGE_SIZE = 4

interface Props {
  cameraIds: CameraId[]
  hostOf: (camId: CameraId) => HTMLElement
  onSelect: (id: CameraId) => void
}

export function PreviewStrip({ cameraIds, hostOf, onSelect }: Props) {
  const [page, setPage] = useState(0)
  const pageCount = Math.max(1, Math.ceil(cameraIds.length / PAGE_SIZE))

  // A camera leaving can empty the last page; stay on the last one that has cameras.
  useEffect(() => {
    if (page > pageCount - 1) {
      setPage(pageCount - 1)
    }
  }, [page, pageCount])

  const visible = cameraIds.slice(page * PAGE_SIZE, (page + 1) * PAGE_SIZE)

  return (
    <div className="flex flex-col gap-2.5 w-[200px]">
      <p className="font-mono text-xs text-center text-zinc-500">他のカメラ（クリックでステージへ）</p>
      {visible.map((id) => (
        <PreviewCamera key={id} cameraId={id} host={hostOf(id)} onClick={() => onSelect(id)} />
      ))}
      {cameraIds.length === 0 && <p className="font-mono text-xs text-center text-zinc-600">なし</p>}
      {pageCount > 1 && (
        <div className="flex items-center justify-between font-mono text-xs text-zinc-400">
          <button
            onClick={() => setPage((p) => Math.max(0, p - 1))}
            disabled={page === 0}
            className="disabled:opacity-30"
          >
            ◀
          </button>
          <span>
            {page + 1} / {pageCount}
          </span>
          <button
            onClick={() => setPage((p) => Math.min(pageCount - 1, p + 1))}
            disabled={page === pageCount - 1}
            className="disabled:opacity-30"
          >
            ▶
          </button>
        </div>
      )}
    </div>
  )
}
