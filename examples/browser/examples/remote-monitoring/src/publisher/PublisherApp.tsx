import { type ReactNode, useEffect, useRef, useState } from 'react'
import { ArrowLeft, Camera, FileVideo, Loader2, Radio, Sparkles, Webcam } from 'lucide-react'
import type { CameraId, VideoSource, VideoSourceType } from '../types/monitoring'
import { NAME_PATTERN } from '../types/monitoring'
import { Tip } from '../../../../lib/ui/tip'
import { PublisherSession } from './publisherSession'
import { CameraPublisher } from './cameraPublisher'
import { RelayUrlField } from '../components/RelayUrlField'
import { Button } from '../../../../lib/ui/button'
import { Input } from '../../../../lib/ui/input'
import { Label } from '../../../../lib/ui/label'
import { pageHref, useUrlSync } from '../hooks/useUrlSync'

type Status = 'idle' | 'connecting' | 'starting' | 'live' | 'error'

const SOURCE_LABELS: Record<VideoSourceType, string> = {
  synthetic: '合成',
  camera: 'カメラ',
  file: 'ファイル'
}

interface Props {
  location: string
  camId: CameraId
  relayUrl: string
}

const SOURCE_ICONS: Record<VideoSourceType, ReactNode> = {
  synthetic: <Sparkles className="h-3.5 w-3.5" />,
  camera: <Camera className="h-3.5 w-3.5" />,
  file: <FileVideo className="h-3.5 w-3.5" />
}

export function PublisherApp({ location: defaultLocation, camId: defaultCamId, relayUrl: defaultRelayUrl }: Props) {
  const [location, setLocation] = useState(defaultLocation)
  const [camId, setCamId] = useState<CameraId>(defaultCamId)
  const [relayUrl, setRelayUrl] = useState(defaultRelayUrl)
  const [status, setStatus] = useState<Status>('idle')
  const [error, setError] = useState<string | null>(null)
  const [sourceType, setSourceType] = useState<VideoSourceType>('synthetic')
  const defaultFileUrl = (id: CameraId) => (id ? `/moq-wasm/videos/${id}.mp4` : '')
  const [fileUrl, setFileUrl] = useState(() => defaultFileUrl(defaultCamId))
  const videoRef = useRef<HTMLVideoElement>(null)
  const sessionRef = useRef<PublisherSession | null>(null)
  const publisherRef = useRef<CameraPublisher | null>(null)

  const isIdle = status === 'idle' || status === 'error'
  const canStart = isIdle && NAME_PATTERN.test(camId) && NAME_PATTERN.test(location)

  useUrlSync({ mode: 'publisher', location, relay: relayUrl, cam: camId || null })

  useEffect(() => {
    setFileUrl(defaultFileUrl(camId))
  }, [camId])

  useEffect(() => {
    return () => {
      publisherRef.current?.stop()
      sessionRef.current?.disconnect()
    }
  }, [])

  /// Connects and publishes in one step; a failure in either leaves nothing open.
  const handleStart = async () => {
    setStatus('connecting')
    setError(null)
    const session = new PublisherSession(location, camId)
    try {
      const moqSession = await session.connect(relayUrl)
      sessionRef.current = session
      setStatus('starting')
      const source: VideoSource = sourceType === 'file' ? { type: 'file', fileUrl } : { type: sourceType }
      const publisher = new CameraPublisher(camId, source)
      const stream = await publisher.start(moqSession, session.namespace)
      publisherRef.current = publisher
      if (videoRef.current) videoRef.current.srcObject = stream
      setStatus('live')
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Start failed')
      session.disconnect()
      sessionRef.current = null
      setStatus('error')
    }
  }

  return (
    <main className="mx-auto flex min-h-[calc(100dvh-var(--shell-height))] w-full max-w-[1200px] flex-col gap-4 px-4 py-5 text-fg sm:px-6">
      {/* header */}
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex items-center gap-3">
          <span className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg border border-accent-line bg-accent-soft text-accent">
            <Webcam className="h-[18px] w-[18px]" />
          </span>
          <div>
            <h1 className="text-lg font-bold leading-tight tracking-tight">カメラを配信する</h1>
          </div>
          {isIdle && (
            <a
              href={pageHref({ mode: null, cam: null, location, relay: relayUrl })}
              className="ml-2 inline-flex items-center gap-1 text-sm font-medium text-accent hover:text-accent-hover"
            >
              <ArrowLeft className="h-4 w-4" />
              Monitor に戻る
            </a>
          )}
        </div>
        <div className="flex items-center gap-2">
          {isIdle && (
            <Button onClick={handleStart} disabled={!canStart}>
              <Radio className="h-4 w-4" />
              配信開始
            </Button>
          )}
          {status === 'connecting' && (
            <Button disabled variant="outline">
              <Loader2 className="h-4 w-4 animate-spin" />
              接続中…
            </Button>
          )}
          {status === 'starting' && (
            <Button disabled variant="outline">
              <Loader2 className="h-4 w-4 animate-spin" />
              起動中…
            </Button>
          )}
          {status === 'live' && (
            <span className="inline-flex h-9 items-center gap-2 rounded-md bg-red-600 px-3 font-mono text-sm font-bold text-white">
              <span className="h-2 w-2 rounded-full bg-white animate-pulse" />
              REC
            </span>
          )}
        </div>
      </div>

      {/* config form (idle/error only) */}
      {isIdle && (
        <div className="space-y-4 rounded-xl border border-line bg-surface p-4 shadow-sm">
          <div className="grid items-start gap-3 md:grid-cols-[1fr_1fr_2fr]">
            <div className="space-y-1">
              <Label className="text-xs text-muted">カメラ名</Label>
              <Input
                value={camId}
                onChange={(e) => setCamId(e.target.value.trim())}
                className="font-mono text-sm"
                placeholder="entrance"
              />
            </div>
            <div className="space-y-1">
              <Label className="text-xs text-muted">Location</Label>
              <Input
                value={location}
                onChange={(e) => setLocation(e.target.value)}
                className="font-mono text-sm"
                placeholder="brave-otter"
              />
            </div>
            <RelayUrlField value={relayUrl} onChange={setRelayUrl} />
          </div>

          {/* video source selector */}
          <div className="space-y-2">
            <Label className="text-xs text-muted">映像ソース</Label>
            <div className="flex w-fit gap-0.5 rounded-lg border border-line bg-sunken p-0.5">
              {(['synthetic', 'camera', 'file'] as VideoSourceType[]).map((t) => (
                <button
                  key={t}
                  onClick={() => setSourceType(t)}
                  aria-pressed={sourceType === t}
                  className={`inline-flex cursor-pointer items-center gap-1.5 rounded-md px-3 py-1.5 text-xs font-semibold transition-colors ${
                    sourceType === t
                      ? 'bg-raised text-fg shadow-sm ring-1 ring-line-strong'
                      : 'text-muted hover:text-fg'
                  }`}
                >
                  {SOURCE_ICONS[t]}
                  {SOURCE_LABELS[t]}
                </button>
              ))}
            </div>
            {sourceType === 'file' && (
              <div className="flex items-center gap-2">
                <Input
                  value={fileUrl}
                  onChange={(e) => setFileUrl(e.target.value)}
                  className="font-mono text-xs"
                  placeholder="/moq-wasm/videos/cam01.mp4"
                />
                <Tip note="public/videos/ 以下のパスを指定" end />
              </div>
            )}
          </div>

          {error && (
            <p className="rounded-md border border-danger/40 bg-danger-soft px-3 py-2 font-mono text-xs text-danger">
              {error}
            </p>
          )}
        </div>
      )}

      {/* preview */}
      <div className="relative flex aspect-video items-center justify-center overflow-hidden rounded-xl bg-neutral-950 shadow-md ring-1 ring-line">
        <video ref={videoRef} autoPlay muted playsInline className="absolute inset-0 w-full h-full object-cover" />
        {status === 'idle' && <Webcam className="h-10 w-10 text-neutral-600" aria-hidden="true" />}
        {camId && (
          <div className="absolute bottom-3 left-3 font-mono text-xs text-white bg-black/50 rounded px-2 py-1">
            {camId}
          </div>
        )}
      </div>

      {/* status bar */}
      {!isIdle && (
        <div className="space-y-1.5 rounded-xl border border-line bg-surface px-4 py-3 font-mono text-sm shadow-sm">
          <div className="flex gap-3 text-xs text-muted">
            <span>Relay:</span>
            <span className="text-muted">{relayUrl}</span>
          </div>
          <div className="flex gap-3 text-xs text-muted">
            <span>Namespace:</span>
            <span className="text-muted">
              [{location}, {camId}] / video
            </span>
          </div>
          <div className="flex gap-3 text-xs text-muted">
            <span>Source:</span>
            <span className="text-muted">
              {SOURCE_LABELS[sourceType]}
              {sourceType === 'file' ? ` — ${fileUrl}` : ''}
            </span>
          </div>
        </div>
      )}
    </main>
  )
}
