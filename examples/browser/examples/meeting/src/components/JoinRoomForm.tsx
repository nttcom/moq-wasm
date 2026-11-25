import { useState, FormEvent, ChangeEvent } from 'react'
import { Loader2, LogIn, User, Users } from 'lucide-react'
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

const INPUT_CLASS_NAME = 'h-11 pl-10 text-[15px]'
const INPUT_ICON_CLASS_NAME = 'pointer-events-none absolute left-3 top-1/2 h-4 w-4 -translate-y-1/2 text-faint'

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
    <main className="flex min-h-[calc(100dvh-var(--shell-height))] items-center justify-center px-4 py-10 sm:px-6">
      <form
        onSubmit={handleSubmit}
        className="flex w-full max-w-md flex-col gap-5 rounded-xl border border-line bg-surface p-8 shadow-lg"
      >
        <div className="space-y-2">
          <Label htmlFor="roomName">ルーム名</Label>
          <div className="relative">
            <Users className={INPUT_ICON_CLASS_NAME} aria-hidden="true" />
            <Input
              type="text"
              id="roomName"
              data-testid="join-room-name-input"
              value={roomName}
              onChange={(e: ChangeEvent<HTMLInputElement>) => setRoomName(e.target.value)}
              placeholder="weekly-sync"
              autoComplete="off"
              required
              className={INPUT_CLASS_NAME}
            />
          </div>
        </div>
        <div className="space-y-2">
          <Label htmlFor="userName">表示名</Label>
          <div className="relative">
            <User className={INPUT_ICON_CLASS_NAME} aria-hidden="true" />
            <Input
              type="text"
              id="userName"
              data-testid="join-user-name-input"
              value={userName}
              onChange={(e: ChangeEvent<HTMLInputElement>) => setUserName(e.target.value)}
              placeholder="alice"
              autoComplete="nickname"
              required
              className={INPUT_CLASS_NAME}
            />
          </div>
        </div>
        <RelayPicker value={relayUrl} onChange={setRelayUrl} className="text-fg" />
        {joinError && (
          <p
            className="rounded-md border border-danger/40 bg-danger-soft px-3 py-2 text-sm text-danger"
            data-testid="join-error"
            role="alert"
          >
            {joinError.message}
          </p>
        )}
        <Button
          type="submit"
          size="lg"
          data-testid="join-submit-button"
          className="mt-1 w-full"
          disabled={!roomName.trim() || !userName.trim() || connecting}
        >
          {connecting ? <Loader2 className="h-4 w-4 animate-spin" /> : <LogIn className="h-4 w-4" />}
          {connecting ? '接続中…' : 'ルームに参加'}
        </Button>
      </form>
    </main>
  )
}
