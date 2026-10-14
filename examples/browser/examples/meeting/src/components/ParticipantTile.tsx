import { type ReactNode, useEffect, useRef } from 'react'
import { MoreHorizontal } from 'lucide-react'
import { MediaStreamVideo, PictureFrame } from './MediaStreamElements'

interface ParticipantTileProps {
  name: string
  testId?: string
  /// A remote picture owned by its live pipeline; `undefined` for the local
  /// member, whose capture `stream` is shown instead.
  picture?: HTMLElement | null
  stream?: MediaStream | null
  label?: string
  speaking: boolean
  footer?: ReactNode
  /// Called with the shown height in device pixels whenever it changes.
  onHeight?: (heightPx: number) => void
  onOpenDetails: () => void
  menuTestId?: string
}

export function ParticipantTile({
  name,
  testId,
  picture,
  stream,
  label,
  speaking,
  footer,
  onHeight,
  onOpenDetails,
  menuTestId
}: ParticipantTileProps) {
  const avatar = <Avatar name={name} />
  const rootRef = useRef<HTMLDivElement | null>(null)
  useEffect(() => {
    const root = rootRef.current
    if (!root || !onHeight) {
      return
    }
    const observer = new ResizeObserver(([entry]) => {
      onHeight(Math.round(entry.contentRect.height * window.devicePixelRatio))
    })
    observer.observe(root)
    return () => observer.disconnect()
  }, [onHeight])
  return (
    <div
      ref={rootRef}
      data-testid={testId}
      className={`group relative h-full w-full overflow-hidden rounded-xl bg-slate-800 ring-2 transition ${
        speaking ? 'ring-emerald-400' : 'ring-white/5'
      }`}
    >
      {picture !== undefined ? (
        <PictureFrame picture={picture} placeholder={avatar} fill />
      ) : stream ? (
        <MediaStreamVideo stream={stream} muted fill />
      ) : (
        <div className="flex h-full w-full items-center justify-center bg-slate-800">{avatar}</div>
      )}
      <div className="pointer-events-none absolute bottom-2 left-2 max-w-[70%] truncate rounded-md bg-black/60 px-2 py-0.5 text-xs font-medium text-white">
        {label ? `${name} · ${label}` : name}
      </div>
      <button
        type="button"
        aria-label={`Details of ${name}`}
        title="Details"
        data-testid={menuTestId}
        onClick={onOpenDetails}
        className="absolute right-2 top-2 rounded-full bg-black/60 p-1.5 text-white opacity-0 transition hover:bg-black/80 focus:opacity-100 group-hover:opacity-100"
      >
        <MoreHorizontal className="h-4 w-4" />
      </button>
      {footer && (
        <div className="pointer-events-none absolute bottom-2 right-2 max-w-[60%] truncate rounded bg-black/50 px-1.5 py-0.5 font-mono text-[10px] text-slate-200 opacity-0 transition group-hover:opacity-100">
          {footer}
        </div>
      )}
    </div>
  )
}

function Avatar({ name }: { name: string }) {
  return (
    <div className="flex h-16 w-16 items-center justify-center rounded-full bg-blue-600 text-2xl font-semibold text-white">
      {name.slice(0, 1).toUpperCase()}
    </div>
  )
}
