import { useState, FormEvent, ChangeEvent } from 'react'
import { LOAD_BALANCED_RELAY_PRESET } from '../../../../utils/relayPresets'
import { Button } from '../../../../lib/ui/button'
import { Input } from '../../../../lib/ui/input'
import { Label } from '../../../../lib/ui/label'
import { RelayPicker } from '../../../../lib/ui/relay-picker'

interface JoinRoomFormProps {
  onJoin: (roomName: string, userName: string, relayUrl: string) => void
  joinError: Error | null
  connecting: boolean
}

const INPUT_CLASS_NAME =
  'h-12 border-white/10 bg-slate-800 text-base text-white placeholder:text-slate-500 ring-offset-slate-900 focus-visible:ring-blue-500'

export function JoinRoomForm({ onJoin, joinError, connecting }: JoinRoomFormProps) {
  const [roomName, setRoomName] = useState('')
  const [userName, setUserName] = useState('')
  const [relayUrl, setRelayUrl] = useState<string>(LOAD_BALANCED_RELAY_PRESET.value)

  const handleSubmit = (e: FormEvent) => {
    e.preventDefault()
    if (roomName.trim() && userName.trim()) {
      onJoin(roomName.trim(), userName.trim(), relayUrl)
    }
  }

  return (
    <div className="flex min-h-screen items-center justify-center bg-slate-950 p-6 text-white">
      <div className="w-full max-w-xl space-y-8 rounded-2xl bg-slate-900 p-8 ring-1 ring-white/10">
        <div className="space-y-2">
          <h1 className="text-3xl font-semibold">Join Meeting Room</h1>
          <p className="text-slate-400">Enter your details to join the meeting</p>
        </div>
        <form onSubmit={handleSubmit} className="space-y-6">
          <div className="space-y-2">
            <Label htmlFor="roomName" className="text-slate-300">
              Room Name
            </Label>
            <Input
              type="text"
              id="roomName"
              data-testid="join-room-name-input"
              value={roomName}
              onChange={(e: ChangeEvent<HTMLInputElement>) => setRoomName(e.target.value)}
              placeholder="Enter room name"
              required
              className={INPUT_CLASS_NAME}
            />
          </div>
          <div className="space-y-2">
            <Label htmlFor="userName" className="text-slate-300">
              User Name
            </Label>
            <Input
              type="text"
              id="userName"
              data-testid="join-user-name-input"
              value={userName}
              onChange={(e: ChangeEvent<HTMLInputElement>) => setUserName(e.target.value)}
              placeholder="Enter your name"
              required
              className={INPUT_CLASS_NAME}
            />
          </div>
          <RelayPicker value={relayUrl} onChange={setRelayUrl} className="text-slate-300" />
          {joinError && (
            <p className="text-sm text-red-400" data-testid="join-error">
              {joinError.message}
            </p>
          )}
          <Button
            type="submit"
            size="lg"
            data-testid="join-submit-button"
            className="h-12 w-full rounded-full bg-blue-600 text-base font-semibold text-white hover:bg-blue-700"
            disabled={!roomName.trim() || !userName.trim() || connecting}
          >
            {connecting ? 'Connecting...' : 'Join Room'}
          </Button>
        </form>
      </div>
    </div>
  )
}
