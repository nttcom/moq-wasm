export type HeaderMember = { id: string; name: string; speaking: boolean; states: Record<string, string> }

interface RoomHeaderProps {
  roomName: string
  members: HeaderMember[]
}

/// Every remote member appears here, on the stage or not.
export function RoomHeader({ roomName, members }: RoomHeaderProps) {
  return (
    <header className="flex items-center gap-3 px-4 py-2 text-sm">
      <h1 className="font-semibold" data-testid="room-name">
        {roomName}
      </h1>
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
