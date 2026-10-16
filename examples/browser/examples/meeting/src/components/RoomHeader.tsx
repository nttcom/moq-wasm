export type HeaderMember = { id: string; name: string; speaking: boolean; states: Record<string, string> }

interface RoomHeaderProps {
  roomName: string
  members: HeaderMember[]
  recording: boolean
}

/// Every remote member appears here, on the stage or not.
export function RoomHeader({ roomName, members, recording }: RoomHeaderProps) {
  return (
    <header className="flex items-center gap-3 px-4 py-2 text-sm">
      <h1 className="font-semibold" data-testid="room-name">
        {roomName}
      </h1>
      {recording && (
        <span
          data-testid="recording-badge"
          className="flex items-center gap-1 rounded bg-red-600 px-2 py-0.5 text-xs font-semibold"
        >
          <span className="h-2 w-2 animate-pulse rounded-full bg-white" />
          REC
        </span>
      )}
      <span className="text-slate-400">{members.length + 1} participants</span>
      <div className="flex -space-x-1">
        {members.map((member) => (
          <span
            key={member.id}
            title={member.name}
            data-testid={`member-chip-${member.name}`}
            {...member.states}
            className={`flex h-6 w-6 items-center justify-center rounded-full bg-blue-600 text-[11px] font-semibold ring-2 ${
              member.speaking ? 'ring-emerald-400' : 'ring-slate-950'
            }`}
          >
            {member.name.slice(0, 1).toUpperCase()}
          </span>
        ))}
      </div>
    </header>
  )
}
