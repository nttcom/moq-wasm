import { useEffect, useReducer, useRef, useState } from 'react'
import type { LivePlayerState } from 'moq-wasm/media'
import { CameraId, MonitorMode } from '../types/monitoring'
import { StageCamera } from './StageCamera'
import { PreviewStrip } from './PreviewStrip'
import { DebugBar } from './DebugBar'
import { SeekBar } from './SeekBar'
import { TransportControls } from './TransportControls'
import { RelayUrlField } from './RelayUrlField'
import { Button } from '../../../../lib/ui/button'
import { CameraPlayers } from '../monitor/cameraPlayers'
import { pageHref, useUrlSync } from '../hooks/useUrlSync'

interface Props {
  location: string
  relayUrl: string
}

type ConnStatus = 'idle' | 'connecting' | 'connected' | 'error'

const STEP_SECONDS = 1

/// A step that reaches the live edge would take the player live, where → switches the stage camera.
function stepsForwardWithinReview({ mode, seek }: LivePlayerState): boolean {
  return (
    mode === 'review' &&
    seek.playheadSeconds !== undefined &&
    seek.playheadSeconds + STEP_SECONDS < seek.liveEdgeSeconds
  )
}
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

  useEffect(() => players.setStage(stageId), [players, stageId])

  const isConnected = connStatus === 'connected'
  const stagePlayer = stageId ? players.get(stageId) : undefined
  const stageState = stagePlayer?.state
  // A paused live picture is the first still frame of a review, before any skip.
  const mode: MonitorMode = stageState && (stageState.mode === 'review' || stageState.paused) ? 'review' : 'live'
  // The readout and the seek bar follow the frame on screen, not the position a
  // step asked for: they stay on the last frame until the step's frame arrives.
  const shownSeconds = stageState?.mode === 'review' ? stageState.seek.shownSeconds : stageState?.seek.liveEdgeSeconds
  const lastShownRef = useRef<number | undefined>(undefined)
  if (shownSeconds !== undefined) {
    lastShownRef.current = shownSeconds
  }
  const thumbSeconds = shownSeconds ?? lastShownRef.current
  const behindSeconds = stageState && thumbSeconds !== undefined ? stageState.seek.liveEdgeSeconds - thumbSeconds : null

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
  const handleStepForward = () => {
    // Read when the key arrives: a held key repeats faster than the page renders.
    if (stagePlayer && stepsForwardWithinReview(stagePlayer.state)) {
      stagePlayer.skip(STEP_SECONDS)
    }
  }
  const handleJump = (seconds: number) => stagePlayer?.seek(seconds * MICROS_PER_SECOND)
  const handleReturnToLive = () => stagePlayer?.goLive()

  const handlePreviewSelect = (id: CameraId) => {
    if (mode === 'review') {
      stagePlayer?.goLive()
    }
    setStageId(id)
    setStageLost(false)
  }

  const switchStage = (offset: 1 | -1) => {
    if (cameras.length === 0 || stageId === undefined) return
    // A withdrawn stage camera is no longer listed: → takes the first camera and ← the last.
    const index = Math.max(cameras.indexOf(stageId), offset > 0 ? -1 : 0)
    setStageId(cameras[(index + offset + cameras.length) % cameras.length])
    setStageLost(false)
  }

  const canStepBack = mode === 'review' && stageState !== undefined && stageState.seek.seekable
  const canStepForward = stageState !== undefined && stepsForwardWithinReview(stageState)

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.target instanceof HTMLInputElement || event.target instanceof HTMLTextAreaElement) return
      // A held key repeats whether or not the last step's frame has arrived. Taking every repeat
      // moves the position ahead of the picture, which then catches up after the key is released.
      const review = stagePlayer?.state
      const stepPending = event.repeat && review?.mode === 'review' && review.seek.shownSeconds === undefined
      switch (event.key) {
        case 'ArrowLeft':
          if (mode === 'live') switchStage(-1)
          else if (canStepBack && !stepPending) handleStepBack()
          break
        case 'ArrowRight':
          if (mode === 'live') switchStage(1)
          else if (canStepForward && !stepPending) handleStepForward()
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

  const publisherHref = pageHref({ mode: 'publisher', location, relay: relayUrl })

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
          ← → でメインのカメラを切替。メインをクリックか ↓ で REVIEW（← → でコマ送り、↑ で LIVE
          に戻る）。右のカメラをクリックでもメインに出せる。
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
              {stageState.seek.seekable && thumbSeconds !== undefined && (
                <SeekBar
                  replayableStartSeconds={stageState.seek.replayableStartSeconds}
                  liveEdgeSeconds={stageState.seek.liveEdgeSeconds}
                  playheadSeconds={thumbSeconds}
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

        <PreviewStrip
          cameraIds={cameras}
          stageId={stageId}
          hostOf={(id) => players.host(id)}
          onSelect={handlePreviewSelect}
        />
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
