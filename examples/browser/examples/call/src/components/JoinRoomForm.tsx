import { useMemo, useState, FormEvent, ChangeEvent } from 'react'
import { CLOUD_RELAY_PRESETS, LOAD_BALANCED_RELAY_PRESET, buildLocalRelayPresets } from '../../../../utils/relayPresets'
import { Button } from './ui/button'
import { Input } from './ui/input'
import { Label } from './ui/label'

interface JoinRoomFormProps {
  onJoin: (roomName: string, userName: string, relayUrl: string) => void
}

interface RelayOption {
  label: string
  value: string
  helper: string
  testId?: string
}

interface RelayOptionGroup {
  label: string
  options: RelayOption[]
}

export function JoinRoomForm({ onJoin }: JoinRoomFormProps) {
  const relayOptionGroups = useMemo(() => buildRelayOptionGroups(), [])
  const [roomName, setRoomName] = useState('')
  const [userName, setUserName] = useState('')
  const [relayUrl, setRelayUrl] = useState<string>(relayOptionGroups[0].options[0].value)

  const handleSubmit = (e: FormEvent) => {
    e.preventDefault()
    if (roomName.trim() && userName.trim()) {
      onJoin(roomName.trim(), userName.trim(), relayUrl)
    }
  }

  return (
    <div className="flex items-center justify-center min-h-screen bg-gray-50 p-8">
      <div className="w-full max-w-4xl space-y-12">
        <div className="text-center space-y-4">
          <h1 className="text-6xl font-bold">Join Call Room</h1>
          <p className="text-2xl text-muted-foreground">Enter your details to join the call</p>
        </div>
        <form onSubmit={handleSubmit} className="space-y-10">
          <div className="space-y-4">
            <Label htmlFor="roomName" className="text-2xl">
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
              className="h-16 text-xl placeholder:text-xl"
            />
          </div>
          <div className="space-y-4">
            <Label htmlFor="userName" className="text-2xl">
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
              className="h-16 text-xl placeholder:text-xl"
            />
          </div>
          <div className="space-y-4">
            <Label className="text-2xl">Relay</Label>
            <div className="space-y-5">
              {relayOptionGroups.map((group) => (
                <fieldset key={group.label} className="space-y-3">
                  <legend className="text-sm font-semibold uppercase tracking-wide text-muted-foreground">
                    {group.label}
                  </legend>
                  <div className="grid gap-3 sm:grid-cols-2">
                    {group.options.map((option) => (
                      <label
                        key={option.value}
                        className="flex min-h-24 items-center gap-4 rounded-xl border border-input bg-background px-5 py-4 text-lg"
                      >
                        <input
                          type="radio"
                          name="relayUrl"
                          value={option.value}
                          checked={relayUrl === option.value}
                          onChange={() => setRelayUrl(option.value)}
                          className="h-5 w-5 shrink-0 accent-blue-600"
                          data-testid={option.testId}
                        />
                        <div className="min-w-0">
                          <div className="font-semibold">{option.label}</div>
                          <div className="break-all text-sm text-muted-foreground">{option.helper}</div>
                        </div>
                      </label>
                    ))}
                  </div>
                </fieldset>
              ))}
            </div>
          </div>
          <Button
            type="submit"
            size="lg"
            data-testid="join-submit-button"
            className="w-full h-16 text-2xl font-semibold bg-blue-600 hover:bg-blue-700 text-white"
            disabled={!roomName.trim() || !userName.trim()}
          >
            Join Room
          </Button>
        </form>
      </div>
    </div>
  )
}

function buildRelayOptionGroups(): RelayOptionGroup[] {
  return [
    {
      label: 'Local Relay',
      options: buildLocalRelayPresets()
    },
    {
      label: 'Cloud (LB)',
      options: [LOAD_BALANCED_RELAY_PRESET]
    },
    {
      label: 'Cloud (specific relay)',
      options: CLOUD_RELAY_PRESETS
    }
  ]
}
