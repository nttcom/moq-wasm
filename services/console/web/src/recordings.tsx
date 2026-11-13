import { useCallback, useEffect, useState } from 'react'

import {
  api,
  describeError,
  formatBytes,
  formatUnixTime,
  recordingFileUrl,
  type ConsoleApp,
  type Recording
} from './api'
import { PageHeading } from './pages'
import { displayName } from './Sidebar'
import { Badge, Button, Card, ErrorText } from './ui'

const STATE_LABELS: Record<Recording['state'], string> = {
  starting: '開始中',
  recording: '録画中',
  stopping: '停止中',
  finished: '完了',
  failed: '失敗'
}

/** The app's MP4 room recordings that the recorder keeps in its bucket, newest first. */
export function RecordingsPage({ app }: { app: ConsoleApp }) {
  const [recordings, setRecordings] = useState<Recording[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(false)

  const load = useCallback(() => {
    let stopped = false
    setLoading(true)
    setError(null)
    api
      .recordings(app.appId)
      .then((answer) => {
        if (!stopped) setRecordings(answer.recordings)
      })
      .catch((reason: unknown) => {
        if (!stopped) setError(describeError(reason))
      })
      .finally(() => {
        if (!stopped) setLoading(false)
      })
    return () => {
      stopped = true
    }
  }, [app.appId])

  useEffect(() => {
    setRecordings(null)
    return load()
  }, [load])

  return (
    <>
      <PageHeading
        title="録画"
        subtitle="この app の room を recorder で録画した MP4 ファイル。ファイルはバケットから直接読み込みます"
        aside={
          <Button variant="secondary" size="sm" onClick={load} disabled={loading}>
            再読み込み
          </Button>
        }
      />
      <div className="mb-4 flex items-center gap-2">
        <Badge tone="info">app: {displayName(app)}</Badge>
      </div>
      <ErrorText message={error} />
      {recordings && recordings.length === 0 && (
        <p className="py-8 text-center text-sm text-muted">
          録画はまだありません。この app の token で開いた meeting から録画すると、ここに表示されます。
        </p>
      )}
      <div className="flex flex-col gap-4.5">
        {recordings?.map((recording) => (
          <RecordingCard key={recording.id} appId={app.appId} recording={recording} />
        ))}
      </div>
    </>
  )
}

function RecordingCard({ appId, recording }: { appId: string; recording: Recording }) {
  const [playing, setPlaying] = useState<string | null>(null)
  const room = recording.namespace_prefix.split('/').slice(1).join('/')
  const total = recording.files.reduce((sum, file) => sum + file.size_bytes, 0)
  return (
    <Card
      title={room}
      aside={
        <div className="flex items-center gap-2 text-xs text-muted">
          <span>{formatUnixTime(recording.requested_at_ms === null ? null : recording.requested_at_ms / 1000)}</span>
          <Badge tone={recording.state === 'finished' ? 'success' : 'neutral'}>{STATE_LABELS[recording.state]}</Badge>
        </div>
      }
    >
      {recording.error && <ErrorText message={recording.error} />}
      <p className="mb-2 text-xs text-muted">
        {recording.members.length} 人 · {recording.files.length} ファイル · {formatBytes(total)} ·{' '}
        <span className="font-mono">{recording.id}</span>
      </p>
      <table className="w-full text-left text-[13px]">
        <tbody>
          {recording.files.map((file) => {
            const url = recordingFileUrl(appId, recording.id, file.name)
            return (
              <tr key={file.name} className="border-b border-divider align-top last:border-0">
                <td className="py-2 pr-4">
                  <p className="font-mono text-xs break-all text-strong">{fileLabel(recording, file.name)}</p>
                  {playing === file.name && (
                    <video className="mt-2 w-full max-w-[640px] rounded-lg bg-black" src={url} controls autoPlay />
                  )}
                </td>
                <td className="w-20 py-2 pr-4 text-right text-xs text-body tabular-nums">
                  {formatBytes(file.size_bytes)}
                </td>
                <td className="w-40 py-2 text-right whitespace-nowrap">
                  <Button
                    variant="ghost"
                    size="sm"
                    onClick={() => setPlaying(playing === file.name ? null : file.name)}
                  >
                    {playing === file.name ? '閉じる' : '再生'}
                  </Button>
                  <a className="px-2 text-[13px] text-primary hover:underline" href={url}>
                    開く
                  </a>
                </td>
              </tr>
            )
          })}
        </tbody>
      </table>
    </Card>
  )
}

/** The file's name without the room, which the card's title already shows. */
function fileLabel(recording: Recording, name: string): string {
  if (name === 'composite.mp4') {
    return '全員を 1 つにまとめた動画（composite.mp4）'
  }
  return name.startsWith(`${recording.namespace_prefix}/`) ? name.slice(recording.namespace_prefix.length + 1) : name
}
