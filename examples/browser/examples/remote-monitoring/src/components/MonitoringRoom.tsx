import { useEffect, useReducer, useRef, useState } from 'react'
import { CameraId, MonitorMode } from '../types/monitoring'
import { StageCamera } from './StageCamera'
import { PreviewStrip } from './PreviewStrip'
import { DebugBar } from './DebugBar'
import { SeekBar } from './SeekBar'
import { TransportControls } from './TransportControls'
import { RelayUrlField } from './RelayUrlField'
import { Button } from '../../../../lib/ui/button'
import { CameraPlayers } from '../monitor/cameraPlayers'
import { useUrlSync } from '../hooks/useUrlSync'

interface Props {
  location: string
  relayUrl: string
}

type ConnStatus = 'idle' | 'connecting' | 'connected' | 'error'

const STEP_SECONDS = 1
const MICROS_PER_SECOND = 1_000_000

export function MonitoringRoom({ location, relayUrl: defaultRelayUrl }: Props) {
  const [relayUrl, setRelayUrl] = useState(defaultRelayUrl)
  const [connStatus, setConnStatus] = useState<ConnStatus>('idle')
  /// In the order the relay announced them; a withdrawn camera leaves the list.
  const [cameras, setCameras] = useState<CameraId[]>([])
  const [stageId, setStageId] = useState<CameraId | undefined>()
  /// The stage's camera was withdrawn; it stays on the stage until another is chosen.
  const [stageLost, setStageLost] = useState(false)
  // Read by the camera events, which are not re-created when the state changes.
  const stageLostRef = useRef(false)
  stageLostRef.current = stageLost
  const [error, setError] = useState<string | null>(null)
  const [, playerChanged] = useReducer((version: number) => version + 1, 0)

  const playersRef = useRef<CameraPlayers | null>(null)
  if (!playersRef.current) {
    playersRef.current = new CameraPlayers(() => playerChanged())
  }
  const players = playersRef.current

  const previewIds = cameras.filter((id) => id !== stageId)
  const isConnected = connStatus === 'connected'
  const stagePlayer = stageId ? players.get(stageId) : undefined
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
      await players.watchCameras(location, ({ kind, camId }) => {
        if (kind === 'added') {
          setCameras((prev) => (prev.includes(camId) ? prev : [...prev, camId]))
          // The first camera, or the one that follows a withdrawn stage, takes the stage.
          setStageId((current) => (current === undefined || stageLostRef.current ? camId : current))
          setStageLost(false)
        } else {
          setCameras((prev) => prev.filter((id) => id !== camId))
          setStageId((current) => {
            if (current === camId) {
              setStageLost(true)
            }
            return current
          })
        }
      })
      setConnStatus('connected')
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Connect failed')
      setConnStatus('error')
    }
  }

  const enterReview = () => {
    if (!stagePlayer || stageLost || mode === 'review') return
    stagePlayer.setPaused(true)
  }

  const handleStepBack = () => stagePlayer?.skip(-STEP_SECONDS)
  const handleStepForward = () => stagePlayer?.skip(STEP_SECONDS)
  const handleJump = (seconds: number) => stagePlayer?.seek(seconds * MICROS_PER_SECOND)
  const handleReturnToLive = () => stagePlayer?.goLive()

  const handlePreviewSelect = (id: CameraId) => {
    if (mode === 'review') {
      stagePlayer?.goLive()
    }
    setStageId(id)
    setStageLost(false)
  }

  const canStepBack = mode === 'review' && stageState !== undefined && stageState.seek.seekable
  const canStepForward = stageState?.mode === 'review'

  // Arrow keys drive the stage: ← → step through a review, ↓ enters it, ↑ returns to live.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.target instanceof HTMLInputElement || event.target instanceof HTMLTextAreaElement) return
      switch (event.key) {
        case 'ArrowLeft':
          if (canStepBack) handleStepBack()
          break
        case 'ArrowRight':
          if (canStepForward) handleStepForward()
          break
        case 'ArrowDown':
          enterReview()
          break
        case 'ArrowUp':
          if (mode === 'review') handleReturnToLive()
          break
        default:
          return
      }
      event.preventDefault()
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  })

  const publisherHref = `?mode=publisher&location=${encodeURIComponent(location)}&relay=${encodeURIComponent(relayUrl)}`

  return (
    <div className="flex flex-col min-h-screen bg-zinc-950 text-zinc-100 p-4 gap-4">
      {/* header */}
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-3">
          <h1 className="text-xl font-bold">MoQT 遠隔監視</h1>
          <span className="font-mono text-sm text-zinc-400">{location}</span>
          <a href={publisherHref} className="text-xs text-zinc-500 hover:text-zinc-300 font-mono underline">
            この location にカメラを追加 →
          </a>
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
          <RelayUrlField value={relayUrl} onChange={setRelayUrl} />
          {error && <p className="text-red-400 text-xs font-mono">⚠ {error}</p>}
        </div>
      )}

      {/* hint */}
      {cameras.length > 0 && (
        <p className="text-xs text-zinc-500">
          ステージをクリックか ↓ で REVIEW（← → でコマ送り、↑ で LIVE
          に戻る）。右のプレビューをクリックでステージと入替。
        </p>
      )}

      {/* main grid */}
      <div className="grid gap-4 items-start grid-cols-[1fr_200px]">
        {/* stage */}
        <div>
          <StageCamera
            cameraId={stageId}
            host={stageId ? players.host(stageId) : undefined}
            mode={mode}
            connState={isConnected ? 'connected' : 'closed'}
            lost={stageLost}
            behindSeconds={behindSeconds}
            onClick={enterReview}
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

        <PreviewStrip cameraIds={previewIds} hostOf={(id) => players.host(id)} onSelect={handlePreviewSelect} />
      </div>

      {/* debug bar */}
      <DebugBar
        connState={isConnected ? 'connected' : 'closed'}
        relayUrl={relayUrl}
        cameras={cameras}
        reviewingCamera={mode === 'review' && stageId ? stageId : null}
        reviewStatus={stageState?.rewindStatus.text ?? null}
      />
    </div>
  )
}
