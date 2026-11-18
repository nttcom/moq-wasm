import { Users, Video } from 'lucide-react'
import type { ActiveRecording } from '../recording/recorderBot'

export type HeaderMember = { id: string; name: string; speaking: boolean; states: Record<string, string> }

interface RoomHeaderProps {
  roomName: string
  members: HeaderMember[]
  /// The state of the room's active recording, if any.
  recording: ActiveRecording['state'] | undefined
  /// Whether the transcriber bot is in the room.
  transcribing: boolean
  /// Whether the transcriber's recognizer is still starting.
  transcriberStarting: boolean
}

/// Every remote member appears here, on the stage or not.
export function RoomHeader({ roomName, members, recording, transcribing, transcriberStarting }: RoomHeaderProps) {
  return (
    <div className="flex min-h-12 items-center gap-3 border-b border-line bg-surface px-4 py-2 text-sm">
      <span className="flex h-7 w-7 shrink-0 items-center justify-center rounded-md border border-accent-line bg-accent-soft text-accent">
        <Video className="h-4 w-4" />
      </span>
      <h1 className="truncate font-semibold text-fg" data-testid="room-name">
        {roomName}
      </h1>
      {recording === 'starting' && (
        <span
          data-testid="recording-badge"
          className="rounded-full border border-line-strong bg-sunken px-2 py-0.5 text-xs font-semibold text-muted"
        >
          Starting
        </span>
      )}
      {(recording === 'recording' || recording === 'stopping') && (
        <span
          data-testid="recording-badge"
          className="flex items-center gap-1.5 rounded-full bg-red-600 px-2 py-0.5 text-xs font-semibold text-white"
        >
          <span className="h-2 w-2 animate-pulse rounded-full bg-white" />
          REC
        </span>
      )}
      {transcribing && (
        <span
          data-testid="transcriber-badge"
          title="このルームの発話は文字起こしされています"
          className="rounded-full border border-line-strong bg-sunken px-2 py-0.5 text-xs font-semibold text-muted"
        >
          {transcriberStarting ? 'CC starting' : 'CC'}
        </span>
      )}
      <span className="ml-auto flex items-center gap-1.5 text-muted">
        <Users className="h-4 w-4" />
        {members.length + 1} participants
      </span>
      <div className="flex -space-x-1.5">
        {members.map((member) => (
          <span
            key={member.id}
            title={member.name}
            data-testid={`member-chip-${member.name}`}
            {...member.states}
            className={`flex h-7 w-7 items-center justify-center rounded-full bg-accent text-[11px] font-semibold text-on-accent ring-2 ${
              member.speaking ? 'ring-success' : 'ring-surface'
            }`}
          >
            {member.name.slice(0, 1).toUpperCase()}
          </span>
        ))}
      </div>
    </div>
  )
}
