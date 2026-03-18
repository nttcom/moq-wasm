interface RoomHeaderProps {
  roomName: string
  onLeave: () => void
}

export function RoomHeader({ roomName, onLeave }: RoomHeaderProps) {
  return (
    <header className="flex flex-col gap-4 border-b border-white/10 pb-8 md:flex-row md:items-center md:justify-between">
      <div>
        <p className="text-sm uppercase tracking-wide text-blue-300">Current Room</p>
        <h1 className="text-4xl font-bold">{roomName}</h1>
      </div>
      <button
        onClick={onLeave}
        className="px-6 py-3 rounded-lg bg-red-500 hover:bg-red-600 transition shadow-lg self-start md:self-auto"
      >
        Leave Room
      </button>
    </header>
  )
}
