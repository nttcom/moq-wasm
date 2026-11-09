import { CircleStop, ExternalLink } from 'lucide-react'
import { RECORDING_CHOICES, type RoomRecordingControls, pendingRecordingLabel } from '../hooks/useRoomRecording'

export function RecordPanel({ recording }: { recording: RoomRecordingControls | null }) {
  if (!recording) {
    return <p className="mt-4 text-sm text-blue-100">Recording is not available in this room</p>
  }
  return (
    <div className="mt-4 flex min-h-0 flex-1 flex-col gap-4">
      <RecordControls recording={recording} />
      <div className="text-xs font-semibold uppercase tracking-wide text-slate-400">Recordings</div>
      <div className="min-h-0 flex-1 space-y-2 overflow-y-auto pr-1">
        {recording.replays.length === 0 ? (
          <p className="text-sm text-blue-100">No finished recording yet.</p>
        ) : (
          recording.replays.toReversed().map((replay) => (
            <a
              key={replay.id}
              href={replay.url}
              target="_blank"
              rel="noopener noreferrer"
              data-testid="record-replay-link"
              className="flex items-center justify-between gap-2 rounded-lg border border-white/10 bg-gray-900/70 px-4 py-3 text-sm text-white transition hover:bg-white/10"
            >
              <span className="truncate">
                {replay.startedAtMs === undefined ? replay.id : new Date(replay.startedAtMs).toLocaleString()}
              </span>
              <ExternalLink className="h-4 w-4 shrink-0 text-blue-200" />
            </a>
          ))
        )}
      </div>
    </div>
  )
}

function RecordControls({ recording }: { recording: RoomRecordingControls }) {
  const active = recording.active
  const pending = pendingRecordingLabel(active)
  return (
    <div className="space-y-2">
      {active ? (
        <button
          type="button"
          data-testid="record-panel-stop"
          disabled={pending !== null || recording.busy}
          onClick={() => void recording.stop()}
          className="flex w-full items-center gap-2 rounded-lg bg-red-600 px-4 py-2 text-left text-sm font-semibold text-white transition hover:bg-red-700 disabled:cursor-not-allowed disabled:opacity-60"
        >
          <CircleStop className="h-4 w-4" />
          {pending ?? 'Stop recording'}
        </button>
      ) : (
        <>
          <div className="text-xs font-semibold uppercase tracking-wide text-slate-400">Start recording</div>
          {RECORDING_CHOICES.map(({ id, layout, composite, label, format }) => (
            <button
              key={id}
              type="button"
              data-testid={`record-panel-${id}`}
              disabled={recording.busy}
              onClick={() => void recording.start(layout, composite, format)}
              className="w-full rounded-lg bg-white/10 px-4 py-2 text-left text-sm font-semibold text-white transition hover:bg-white/20 disabled:cursor-not-allowed disabled:opacity-60"
            >
              {label}
            </button>
          ))}
        </>
      )}
      {recording.error && <p className="text-xs text-red-300">{recording.error}</p>}
    </div>
  )
}
