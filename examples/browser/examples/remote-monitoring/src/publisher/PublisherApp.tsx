import { useEffect, useRef, useState } from 'react'
import type { CameraId } from '../types/monitoring'
import { ALL_CAMERA_IDS } from '../types/monitoring'
import { PublisherSession } from './publisherSession'
import { CameraPublisher } from './cameraPublisher'
import { RelayUrlField } from '../components/RelayUrlField'
import { Button } from '../components/ui/button'
import { Input } from '../components/ui/input'
import { Label } from '../components/ui/label'
import { useUrlSync } from '../hooks/useUrlSync'

type Status = 'idle' | 'connecting' | 'connected' | 'starting' | 'live' | 'error'

interface Props {
  location: string
  camId: CameraId | ''
  relayUrl: string
}

export function PublisherApp({ location: defaultLocation, camId: defaultCamId, relayUrl: defaultRelayUrl }: Props) {
  const [location, setLocation] = useState(defaultLocation)
  const [camId, setCamId] = useState<CameraId | ''>(defaultCamId)
  const [relayUrl, setRelayUrl] = useState(defaultRelayUrl)
  const [status, setStatus] = useState<Status>('idle')
  const [error, setError] = useState<string | null>(null)
  const [subscriberCount, setSubscriberCount] = useState(0)
  const videoRef = useRef<HTMLVideoElement>(null)
  const sessionRef = useRef<PublisherSession | null>(null)
  const publisherRef = useRef<CameraPublisher | null>(null)
  const timerRef = useRef<ReturnType<typeof setInterval> | null>(null)

  const isIdle = status === 'idle' || status === 'error'
  const canConnect = isIdle && !!camId && !!location

  useUrlSync({ mode: 'publisher', location, relay: relayUrl, cam: camId || null })

  useEffect(() => {
    return () => {
      if (timerRef.current) clearInterval(timerRef.current)
      publisherRef.current?.stop()
      sessionRef.current?.disconnect().catch(console.error)
    }
  }, [])

  const handleConnect = async () => {
    setStatus('connecting')
    setError(null)
    try {
      const session = new PublisherSession(location, camId as CameraId)
      await session.connect(relayUrl)
      sessionRef.current = session
      setStatus('connected')
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Connect failed')
      setStatus('error')
    }
  }

  const handlePublish = async () => {
    const session = sessionRef.current
    if (!session) return
    setStatus('starting')
    setError(null)
    try {
      const publisher = new CameraPublisher(session, camId as CameraId)
      const stream = await publisher.start()
      publisherRef.current = publisher
      if (videoRef.current) videoRef.current.srcObject = stream
      setStatus('live')
      timerRef.current = setInterval(() => {
        setSubscriberCount(session.getVideoTrackAliases().length)
      }, 1000)
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Publish failed')
      setStatus('connected')
    }
  }

  return (
    <div className="flex flex-col min-h-screen bg-zinc-950 text-white p-4 gap-4">
      {/* header */}
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-3">
          <h1 className="text-xl font-bold font-mono">Publisher</h1>
          {isIdle && (
            <a
              href={`?location=${encodeURIComponent(location)}&relay=${encodeURIComponent(relayUrl)}`}
              className="text-xs text-zinc-500 hover:text-zinc-300 font-mono underline"
            >
              Monitor へ
            </a>
          )}
        </div>
        <div className="flex items-center gap-2">
          {isIdle && (
            <Button
              onClick={handleConnect}
              disabled={!canConnect}
              className="bg-blue-600 hover:bg-blue-700 disabled:opacity-40"
            >
              接続
            </Button>
          )}
          {status === 'connecting' && (
            <Button disabled variant="outline">
              接続中…
            </Button>
          )}
          {status === 'connected' && (
            <>
              <span className="font-mono text-xs text-green-400">● 接続済み</span>
              <Button onClick={handlePublish} className="bg-blue-600 hover:bg-blue-700">
                配信開始
              </Button>
            </>
          )}
          {status === 'starting' && (
            <Button disabled variant="outline">
              カメラ起動中…
            </Button>
          )}
          {status === 'live' && (
            <span className="flex items-center gap-1.5 font-mono text-sm font-bold text-red-400">
              <span className="h-2.5 w-2.5 rounded-full bg-red-500 animate-pulse" />
              REC
            </span>
          )}
        </div>
      </div>

      {/* config form (idle/error only) */}
      {isIdle && (
        <div className="rounded-xl bg-zinc-900 px-4 py-4 space-y-3">
          <div className="grid grid-cols-[1fr_1fr_2fr] gap-3 items-end">
            <div className="space-y-1">
              <Label className="text-xs text-zinc-400 font-mono">カメラ ID</Label>
              <select
                value={camId}
                onChange={(e) => setCamId(e.target.value as CameraId)}
                className="w-full rounded-md bg-zinc-800 border border-zinc-700 px-3 py-2 text-sm font-mono text-white focus:outline-none focus:ring-1 focus:ring-blue-500"
              >
                <option value="" disabled>
                  選択してください
                </option>
                {ALL_CAMERA_IDS.map((id) => (
                  <option key={id} value={id}>
                    {id.toUpperCase()}
                  </option>
                ))}
              </select>
            </div>
            <div className="space-y-1">
              <Label className="text-xs text-zinc-400 font-mono">Location</Label>
              <Input
                value={location}
                onChange={(e) => setLocation(e.target.value)}
                className="bg-zinc-800 border-zinc-700 font-mono text-sm"
                placeholder="my-building"
              />
            </div>
            <RelayUrlField value={relayUrl} onChange={setRelayUrl} />
          </div>
          {error && <p className="text-red-400 text-xs font-mono">⚠ {error}</p>}
        </div>
      )}

      {/* preview */}
      <div className="relative flex-1 rounded-xl overflow-hidden bg-zinc-900 aspect-video flex items-center justify-center">
        <video ref={videoRef} autoPlay muted playsInline className="absolute inset-0 w-full h-full object-cover" />
        {status === 'idle' && <p className="font-mono text-zinc-500 text-sm">接続 → 配信開始 で配信を開始します</p>}
        {camId && (
          <div className="absolute bottom-3 left-3 font-mono text-xs text-white bg-black/50 rounded px-2 py-1">
            {camId.toUpperCase()}
          </div>
        )}
      </div>

      {/* status bar (connected or live) */}
      {!isIdle && (
        <div className="rounded-xl bg-zinc-900 px-4 py-3 font-mono text-sm space-y-1">
          <div className="flex gap-3 text-xs text-zinc-400">
            <span>Relay:</span>
            <span className="text-zinc-300">{relayUrl}</span>
          </div>
          <div className="flex gap-3 text-xs text-zinc-400">
            <span>Namespace:</span>
            <span className="text-zinc-300">
              [{location}, {camId}] / video
            </span>
          </div>
          <div className="flex gap-3 text-xs text-zinc-400">
            <span>Subscribers:</span>
            <span className={subscriberCount > 0 ? 'text-green-400' : 'text-zinc-500'}>{subscriberCount}</span>
          </div>
        </div>
      )}
    </div>
  )
}
