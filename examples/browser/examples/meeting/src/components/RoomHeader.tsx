interface RoomHeaderProps {
  roomName: string
  participantCount: number
}

export function RoomHeader({ roomName, participantCount }: RoomHeaderProps) {
  return (
    <header className="flex items-center gap-3 px-4 py-2 text-sm">
      <h1 className="font-semibold" data-testid="room-name">
        {roomName}
      </h1>
      <span className="text-slate-400">{participantCount} participants</span>
    </header>
  )
}
