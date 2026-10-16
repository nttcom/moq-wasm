import { useMemo, useState, FormEvent, ChangeEvent } from 'react'
import {
  CLOUD_RELAY_PRESETS,
  LOAD_BALANCED_RELAY_PRESET,
  buildLocalRelayPresets,
  type RelayPreset
} from '../../../../utils/relayPresets'
import { Button } from '../../../../lib/ui/button'
import { Input } from '../../../../lib/ui/input'
import { Label } from '../../../../lib/ui/label'

interface JoinRoomFormProps {
  onJoin: (roomName: string, userName: string, relayUrl: string) => void
  joinError: Error | null
  connecting: boolean
}

type RelayTab = 'cloud' | 'local'

const RELAY_TAB_LABELS: Record<RelayTab, string> = { cloud: 'Cloud', local: 'Local' }

const INPUT_CLASS_NAME =
  'h-12 border-white/10 bg-slate-800 text-base text-white placeholder:text-slate-500 ring-offset-slate-900 focus-visible:ring-blue-500'

export function JoinRoomForm({ onJoin, joinError, connecting }: JoinRoomFormProps) {
  const relayOptions = useMemo<Record<RelayTab, RelayPreset[]>>(
    () => ({
      cloud: [LOAD_BALANCED_RELAY_PRESET, ...CLOUD_RELAY_PRESETS],
      local: buildLocalRelayPresets()
    }),
    []
  )
  const [roomName, setRoomName] = useState('')
  const [userName, setUserName] = useState('')
  const [relayTab, setRelayTab] = useState<RelayTab>('cloud')
  const [relayUrl, setRelayUrl] = useState<string>(relayOptions.cloud[0].value)

  const selectRelayTab = (tab: RelayTab) => {
    setRelayTab(tab)
    setRelayUrl(relayOptions[tab][0].value)
  }

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
          <div className="space-y-3">
            <div className="flex items-center justify-between">
              <span className="text-sm font-medium text-slate-300">Relay</span>
              <div role="tablist" className="flex rounded-full bg-slate-800 p-1">
                {(Object.keys(RELAY_TAB_LABELS) as RelayTab[]).map((tab) => (
                  <button
                    key={tab}
                    type="button"
                    role="tab"
                    aria-selected={relayTab === tab}
                    data-testid={`join-relay-tab-${tab}`}
                    onClick={() => selectRelayTab(tab)}
                    className={`rounded-full px-4 py-1 text-sm font-medium transition ${
                      relayTab === tab ? 'bg-slate-600 text-white' : 'text-slate-400 hover:text-white'
                    }`}
                  >
                    {RELAY_TAB_LABELS[tab]}
                  </button>
                ))}
              </div>
            </div>
            <div role="tabpanel" className="space-y-2">
              {relayOptions[relayTab].map((option) => (
                <label
                  key={option.value}
                  className={`flex cursor-pointer items-center gap-3 rounded-xl border px-4 py-3 transition ${
                    relayUrl === option.value
                      ? 'border-blue-500 bg-blue-500/10'
                      : 'border-white/10 bg-slate-800/60 hover:bg-slate-800'
                  }`}
                >
                  <input
                    type="radio"
                    name="relayUrl"
                    value={option.value}
                    checked={relayUrl === option.value}
                    onChange={() => setRelayUrl(option.value)}
                    className="h-4 w-4 shrink-0 accent-blue-500"
                    data-testid={option.testId}
                  />
                  <div className="min-w-0">
                    <div className="font-medium">{option.label}</div>
                    <div className="break-all text-sm text-slate-400">{option.helper}</div>
                  </div>
                </label>
              ))}
            </div>
          </div>
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
