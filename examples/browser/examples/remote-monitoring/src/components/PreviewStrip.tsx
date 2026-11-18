import { useEffect, useState } from 'react'
import { CameraId } from '../types/monitoring'
import { PreviewCamera } from './PreviewCamera'

const PAGE_SIZE = 4

interface Props {
  /// Every camera, the stage's included: a camera keeps its slot whichever is on the stage.
  cameraIds: CameraId[]
  stageId: CameraId | undefined
  hostOf: (camId: CameraId) => HTMLElement
  onSelect: (id: CameraId) => void
}

export function PreviewStrip({ cameraIds, stageId, hostOf, onSelect }: Props) {
  const [page, setPage] = useState(0)
  const pageCount = Math.max(1, Math.ceil(cameraIds.length / PAGE_SIZE))

  // A camera leaving can empty the last page; stay on the last one that has cameras.
  useEffect(() => {
    if (page > pageCount - 1) {
      setPage(pageCount - 1)
    }
  }, [page, pageCount])

  // The page follows the stage's camera; paging by hand lasts until the stage's camera or its page changes.
  const stageIndex = stageId === undefined ? -1 : cameraIds.indexOf(stageId)
  const stagePage = stageIndex < 0 ? undefined : Math.floor(stageIndex / PAGE_SIZE)
  useEffect(() => {
    if (stagePage !== undefined) {
      setPage(stagePage)
    }
  }, [stageId, stagePage])

  const visible = cameraIds.slice(page * PAGE_SIZE, (page + 1) * PAGE_SIZE)

  return (
    <div className="flex w-full flex-col gap-2.5 rounded-xl border border-line bg-surface p-3 shadow-sm">
      <div className="flex items-baseline justify-between gap-2">
        <p className="text-sm font-semibold text-fg">カメラ</p>
        <p className="text-[11px] text-faint">クリックでメインへ</p>
      </div>
      {/* Two thumbnails a row below lg, where the strip sits under the stage; one column beside it. */}
      <div className="grid grid-cols-2 gap-2.5 sm:grid-cols-4 lg:grid-cols-1">
        {visible.map((id) => (
          <PreviewCamera
            key={id}
            cameraId={id}
            host={hostOf(id)}
            onStage={id === stageId}
            onClick={() => onSelect(id)}
          />
        ))}
        {/* Empty slots keep the pager where it is on a last page that is not full. */}
        {pageCount > 1 &&
          Array.from({ length: PAGE_SIZE - visible.length }, (_, slot) => (
            <div key={slot} className="hidden aspect-video w-full lg:block" />
          ))}
      </div>
      {cameraIds.length === 0 && (
        <p className="rounded-md border border-dashed border-line-strong py-6 text-center text-xs text-faint">なし</p>
      )}
      {pageCount > 1 && (
        <div className="flex items-center justify-between text-xs text-muted">
          <button
            onClick={() => setPage((p) => Math.max(0, p - 1))}
            disabled={page === 0}
            className="inline-flex h-7 w-7 cursor-pointer items-center justify-center rounded-md border border-line hover:bg-hover disabled:cursor-not-allowed disabled:opacity-30"
          >
            ◀
          </button>
          <span>
            {page + 1} / {pageCount}
          </span>
          <button
            onClick={() => setPage((p) => Math.min(pageCount - 1, p + 1))}
            disabled={page === pageCount - 1}
            className="inline-flex h-7 w-7 cursor-pointer items-center justify-center rounded-md border border-line hover:bg-hover disabled:cursor-not-allowed disabled:opacity-30"
          >
            ▶
          </button>
        </div>
      )}
    </div>
  )
}
