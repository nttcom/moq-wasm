import { useState, FormEvent, ChangeEvent } from 'react'
import { Gauge, MonitorUp, Users, Video } from 'lucide-react'
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

const INPUT_CLASS_NAME = 'h-11 text-[15px]'

const FEATURES = [
  { icon: Users, text: '同じルーム名の参加者と映像・音声でつながる' },
  { icon: MonitorUp, text: '画面共有・チャット・録画・字幕に対応' },
  { icon: Gauge, text: '受信統計と再生バッファをその場で確認' }
]

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
      <div className="grid w-full max-w-4xl overflow-hidden rounded-xl border border-line bg-surface shadow-lg md:grid-cols-[1fr_1.1fr]">
        <section className="flex flex-col gap-6 border-b border-line bg-subtle p-8 md:border-b-0 md:border-r">
          <span className="flex h-11 w-11 items-center justify-center rounded-lg border border-accent-line bg-accent-soft text-accent">
            <Video className="h-5 w-5" />
          </span>
          <div className="space-y-2">
            <p className="text-xs font-semibold uppercase tracking-[0.08em] text-accent">ユースケース · Meeting</p>
            <h1 className="text-2xl font-bold tracking-tight text-fg">ルームに参加する</h1>
            <p className="text-sm leading-relaxed text-muted">
              MoQT の Relay を経由して、多人数の映像・音声会議をブラウザだけで試せます。
            </p>
          </div>
          <ul className="space-y-3">
            {FEATURES.map(({ icon: Icon, text }) => (
              <li key={text} className="flex items-start gap-3 text-sm text-muted">
                <Icon className="mt-0.5 h-4 w-4 shrink-0 text-accent" />
                {text}
              </li>
            ))}
          </ul>
        </section>
        <form onSubmit={handleSubmit} className="flex flex-col gap-5 p-8">
          <div className="space-y-2">
            <Label htmlFor="roomName">ルーム名</Label>
            <Input
              type="text"
              id="roomName"
              data-testid="join-room-name-input"
              value={roomName}
              onChange={(e: ChangeEvent<HTMLInputElement>) => setRoomName(e.target.value)}
              placeholder="例: weekly-sync"
              autoComplete="off"
              required
              className={INPUT_CLASS_NAME}
            />
          </div>
          <div className="space-y-2">
            <Label htmlFor="userName">表示名</Label>
            <Input
              type="text"
              id="userName"
              data-testid="join-user-name-input"
              value={userName}
              onChange={(e: ChangeEvent<HTMLInputElement>) => setUserName(e.target.value)}
              placeholder="ほかの参加者に表示される名前"
              autoComplete="nickname"
              required
              className={INPUT_CLASS_NAME}
            />
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
            {connecting ? '接続しています…' : 'ルームに参加'}
          </Button>
        </form>
      </div>
    </main>
  )
}
