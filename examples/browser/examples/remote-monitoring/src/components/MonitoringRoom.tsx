import { useEffect, useReducer, useRef, useState } from 'react'
import { CameraId, MonitorMode, ALL_CAMERA_IDS } from '../types/monitoring'
import { StageCamera } from './StageCamera'
import { ThumbStrip } from './ThumbStrip'
import { DebugBar } from './DebugBar'
import { SeekBar } from './SeekBar'
import { TransportControls } from './TransportControls'
import { RelayUrlField } from './RelayUrlField'
import { Button } from './ui/button'
import { Input } from './ui/input'
import { Label } from './ui/label'
import { CameraPlayers } from '../monitor/cameraPlayers'
import { useUrlSync } from '../hooks/useUrlSync'

interface Props {
  location: string
  relayUrl: string
}

type ConnStatus = 'idle' | 'connecting' | 'connected' | 'error'

const STEP_SECONDS = 1
const MICROS_PER_SECOND = 1_000_000

export function MonitoringRoom({ location: defaultLocation, relayUrl: defaultRelayUrl }: Props) {
  const [location, setLocation] = useState(defaultLocation)
  const [relayUrl, setRelayUrl] = useState(defaultRelayUrl)
  const [stageId, setStageId] = useState<CameraId>('cam01')
  const [connStatus, setConnStatus] = useState<ConnStatus>('idle')
  const [subscribedCameras, setSubscribedCameras] = useState<Set<CameraId>>(new Set())
  const [subscribingCameras, setSubscribingCameras] = useState<Set<CameraId>>(new Set())
  const [error, setError] = useState<string | null>(null)
  const [, playerChanged] = useReducer((version: number) => version + 1, 0)

  const playersRef = useRef<CameraPlayers | null>(null)
  if (!playersRef.current) {
    playersRef.current = new CameraPlayers(() => playerChanged())
  }
  const players = playersRef.current

  const thumbIds = ALL_CAMERA_IDS.filter((id) => id !== stageId)
  const isConnected = connStatus === 'connected'
  const stagePlayer = players.get(stageId)
  const stageState = stagePlayer?.state
  // A paused live picture is the first still frame of a review, before any skip.
  const mode: MonitorMode = stageState && (stageState.mode === 'review' || stageState.paused) ? 'review' : 'live'
  const playheadSeconds = stageState?.seek.playheadSeconds ?? stageState?.seek.liveEdgeSeconds
  const behindSeconds =
    stageState && playheadSeconds !== undefined ? stageState.seek.liveEdgeSeconds - playheadSeconds : null

  useUrlSync({ location, relay: relayUrl })

  useEffect(() => {
    return () => {
      players.disconnect().catch(console.error)
    }
  }, [players])

  const handleConnect = async () => {
    setConnStatus('connecting')
    setError(null)
    try {
      await players.connect(relayUrl, () => setConnStatus('error'))
      setConnStatus('connected')
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Connect failed')
      setConnStatus('error')
    }
  }

  const handleSubscribe = async (camId: CameraId) => {
    setSubscribingCameras((prev) => new Set([...prev, camId]))
    try {
      await players.watch(location, camId)
      setSubscribedCameras((prev) => new Set([...prev, camId]))
    } catch (e) {
      console.error('[mon] watch failed', { camId, error: String(e) })
    } finally {
      setSubscribingCameras((prev) => {
        const s = new Set(prev)
        s.delete(camId)
        return s
      })
    }
  }

  const handleStageClick = () => {
    if (!stagePlayer || !subscribedCameras.has(stageId) || mode === 'review') return
    stagePlayer.setPaused(true)
  }

  const handleStepBack = () => stagePlayer?.skip(-STEP_SECONDS)
  const handleStepForward = () => stagePlayer?.skip(STEP_SECONDS)
  const handleJump = (seconds: number) => stagePlayer?.seek(seconds * MICROS_PER_SECOND)
  const handleReturnToLive = () => stagePlayer?.goLive()

  const handleThumbSelect = (id: CameraId) => {
    if (mode === 'review') {
      stagePlayer?.goLive()
    }
    setStageId(id)
  }

  const subButtonForCamera = isConnected ? handleSubscribe : undefined

  const canStepBack = mode === 'review' && stageState !== undefined && stageState.seek.seekable
  const canStepForward = stageState?.mode === 'review'

  return (
    <div className="flex flex-col min-h-screen bg-zinc-950 text-zinc-100 p-4 gap-4">
      {/* header */}
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-3">
          <h1 className="text-xl font-bold">MoQT 遠隔監視</h1>
          {(connStatus === 'idle' || connStatus === 'error') && (
            <a
              href={`?mode=publisher&location=${encodeURIComponent(location)}&relay=${encodeURIComponent(relayUrl)}`}
              className="text-xs text-zinc-500 hover:text-zinc-300 font-mono underline"
            >
              Publisher へ →
            </a>
          )}
        </div>
        <div className="flex items-center gap-2">
          {(connStatus === 'idle' || connStatus === 'error') && (
            <Button onClick={handleConnect} className="bg-blue-600 hover:bg-blue-700 text-white">
              接続
            </Button>
          )}
          {connStatus === 'connecting' && (
            <Button disabled variant="outline">
              接続中…
            </Button>
          )}
          {connStatus === 'connected' && (
            <span className="flex items-center gap-1.5 font-mono text-sm text-green-400">
              <span className="h-2 w-2 rounded-full bg-green-500" />
              接続済み
            </span>
          )}
        </div>
      </div>

      {/* config form (idle/error only) */}
      {(connStatus === 'idle' || connStatus === 'error') && (
        <div className="rounded-xl bg-zinc-900 px-4 py-4 space-y-3">
          <div className="grid grid-cols-[1fr_2fr] gap-3 items-end">
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

      {/* hint */}
      {subscribedCameras.size > 0 && (
        <p className="text-xs text-zinc-500">
          主役をクリックすると REVIEW（コマ送りモード）。右サムネをクリックで主役と入替。
        </p>
      )}

      {/* main grid */}
      <div className={`grid gap-4 items-start ${thumbIds.length > 0 ? 'grid-cols-[1fr_200px]' : 'grid-cols-1'}`}>
        {/* stage */}
        <div>
          <StageCamera
            cameraId={stageId}
            host={players.host(stageId)}
            mode={mode}
            connState={isConnected ? 'connected' : 'closed'}
            behindSeconds={behindSeconds}
            onSubscribe={subButtonForCamera ? () => subButtonForCamera(stageId) : undefined}
            isSubscribed={subscribedCameras.has(stageId)}
            isSubscribing={subscribingCameras.has(stageId)}
            onClick={handleStageClick}
          />
          {mode === 'review' && stageState && (
            <div className="mt-3 space-y-2">
              {stageState.seek.seekable && playheadSeconds !== undefined && (
                <SeekBar
                  replayableStartSeconds={stageState.seek.replayableStartSeconds}
                  liveEdgeSeconds={stageState.seek.liveEdgeSeconds}
                  playheadSeconds={playheadSeconds}
                  onJump={handleJump}
                />
              )}
              <TransportControls
                onStepBack={handleStepBack}
                onStepForward={handleStepForward}
                onReturnToLive={handleReturnToLive}
                canStepBack={canStepBack}
                canStepForward={canStepForward}
              />
            </div>
          )}
        </div>

        {/* strip */}
        {thumbIds.length > 0 && (
          <ThumbStrip
            cameraIds={thumbIds}
            hostOf={(id) => players.host(id)}
            onSelect={handleThumbSelect}
            onSubscribe={subButtonForCamera}
            subscribedCameras={subscribedCameras}
            subscribingCameras={subscribingCameras}
          />
        )}
      </div>

      {/* debug bar */}
      <DebugBar
        connState={isConnected ? 'connected' : 'closed'}
        relayUrl={relayUrl}
        subscribedCameras={[...subscribedCameras]}
        reviewingCamera={mode === 'review' ? stageId : null}
        reviewStatus={stageState?.rewindStatus.text ?? null}
      />
    </div>
  )
}
