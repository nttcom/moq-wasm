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
    <header className="flex items-center gap-3 border-b border-white/15 bg-zinc-900 px-4 py-2 text-sm">
      <h1 className="font-semibold" data-testid="room-name">
        {roomName}
      </h1>
      {recording === 'starting' && (
        <span
          data-testid="recording-badge"
          className="rounded bg-zinc-700 px-2 py-0.5 text-xs font-semibold text-zinc-200"
        >
          Starting
        </span>
      )}
      {(recording === 'recording' || recording === 'stopping') && (
        <span
          data-testid="recording-badge"
          className="flex items-center gap-1 rounded bg-red-600 px-2 py-0.5 text-xs font-semibold"
        >
          <span className="h-2 w-2 animate-pulse rounded-full bg-white" />
          REC
        </span>
      )}
      {transcribing && (
        <span
          data-testid="transcriber-badge"
          title="Speech in this room is transcribed"
          className="rounded bg-zinc-700 px-2 py-0.5 text-xs font-semibold text-zinc-200"
        >
          {transcriberStarting ? 'CC starting' : 'CC'}
        </span>
      )}
      <span className="text-zinc-400">{members.length + 1} participants</span>
      <div className="flex -space-x-1">
        {members.map((member) => (
          <span
            key={member.id}
            title={member.name}
            data-testid={`member-chip-${member.name}`}
            {...member.states}
            className={`flex h-6 w-6 items-center justify-center rounded-full bg-teal-600 text-[11px] font-semibold ring-2 ${
              member.speaking ? 'ring-emerald-400' : 'ring-zinc-950'
            }`}
          >
            {member.name.slice(0, 1).toUpperCase()}
          </span>
        ))}
      </div>
    </header>
  )
}
