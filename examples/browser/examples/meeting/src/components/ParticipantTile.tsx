import type { ReactNode } from 'react'
import { MoreHorizontal } from 'lucide-react'
import { MediaStreamVideo, PictureFrame } from './MediaStreamElements'

interface ParticipantTileProps {
  name: string
  testId?: string
  /// A remote source's player element and whether it has shown a frame;
  /// `undefined` for the local member, whose capture `stream` is shown instead.
  picture?: { element?: HTMLElement; shown: boolean }
  stream?: MediaStream | null
  label?: string
  speaking: boolean
  /// The member's newest transcribed utterance, while it is fresh.
  caption?: string
  footer?: ReactNode
  /// When set, the tile shows a details button that calls it.
  onOpenDetails?: () => void
  menuTestId?: string
}

export function ParticipantTile({
  name,
  testId,
  picture,
  stream,
  label,
  speaking,
  caption,
  footer,
  onOpenDetails,
  menuTestId
}: ParticipantTileProps) {
  const avatar = <Avatar name={name} />
  return (
    <div
      data-testid={testId}
      className={`group relative h-full w-full overflow-hidden rounded-xl bg-slate-800 ring-2 transition ${
        speaking ? 'ring-emerald-400' : 'ring-white/5'
      }`}
    >
      {picture !== undefined ? (
        <PictureFrame element={picture.element} shown={picture.shown} placeholder={avatar} />
      ) : stream ? (
        <MediaStreamVideo stream={stream} muted fill />
      ) : (
        <div className="flex h-full w-full items-center justify-center bg-slate-800">{avatar}</div>
      )}
      {caption && (
        <div
          data-testid="tile-caption"
          className="pointer-events-none absolute bottom-9 left-1/2 max-w-[90%] -translate-x-1/2 rounded-md bg-black/70 px-3 py-1 text-center text-sm text-white"
        >
          {caption}
        </div>
      )}
      <div className="pointer-events-none absolute bottom-2 left-2 max-w-[70%] truncate rounded-md bg-black/60 px-2 py-0.5 text-xs font-medium text-white">
        {label ? `${name} · ${label}` : name}
      </div>
      {onOpenDetails && (
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
      )}
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
