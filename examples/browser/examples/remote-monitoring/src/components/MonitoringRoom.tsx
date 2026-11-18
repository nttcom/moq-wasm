import { useEffect, useReducer, useRef, useState } from 'react'
import { Cctv, Loader2, MapPin, Plug, Plus } from 'lucide-react'
import type { LivePlayerState } from 'moq-wasm/media'
import { CameraId, MonitorMode } from '../types/monitoring'
import { StageCamera } from './StageCamera'
import { PreviewStrip } from './PreviewStrip'
import { DebugBar } from './DebugBar'
import { SeekBar } from './SeekBar'
import { TransportControls } from './TransportControls'
import { RelayUrlField } from './RelayUrlField'
import { Tip } from '../../../../lib/ui/tip'
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

/// The keys of the stage, shown in the title's tip.
const KEYBOARD_NOTE =
  '← → メインのカメラを切替\n↓ またはメインをクリックで REVIEW（← → でコマ送り、↑ で LIVE）\n右のカメラをクリックでメインに表示'

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
    <main className="mx-auto flex min-h-[calc(100dvh-var(--shell-height))] w-full max-w-[1600px] flex-col gap-4 px-4 py-5 text-fg sm:px-6">
      {/* header */}
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-2">
          <span className="flex h-9 w-9 shrink-0 items-center justify-center rounded-lg border border-accent-line bg-accent-soft text-accent">
            <Cctv className="h-[18px] w-[18px]" />
          </span>
          <div className="min-w-0">
            <p className="text-[11px] font-semibold uppercase tracking-[0.08em] text-accent">
              ユースケース · Remote Monitoring
            </p>
            <h1 className="flex items-center gap-1.5 text-lg font-bold leading-tight tracking-tight">
              MoQT 遠隔監視
              <Tip note={KEYBOARD_NOTE} />
            </h1>
          </div>
          <span
            className="inline-flex items-center gap-1 rounded-full border border-line-strong bg-sunken px-2.5 py-0.5 font-mono text-xs text-muted"
            title="location"
          >
            <MapPin className="h-3 w-3" />
            {location}
          </span>
        </div>
        <div className="flex items-center gap-2">
          <a
            href={publisherHref}
            className="inline-flex h-9 items-center gap-1.5 rounded-md border border-line-strong bg-raised px-3 text-sm font-semibold text-fg transition hover:bg-hover"
          >
            <Plus className="h-4 w-4" />
            カメラを追加
          </a>
          {(connStatus === 'idle' || connStatus === 'error') && (
            <Button onClick={handleConnect}>
              <Plug className="h-4 w-4" />
              接続
            </Button>
          )}
          {connStatus === 'connecting' && (
            <Button disabled variant="outline">
              <Loader2 className="h-4 w-4 animate-spin" />
              接続中…
            </Button>
          )}
          {connStatus === 'connected' && (
            <span className="inline-flex h-9 items-center gap-2 rounded-md border border-success/40 bg-success-soft px-3 text-sm font-semibold text-success">
              <span className="h-2 w-2 rounded-full bg-success shadow-[0_0_0_3px] shadow-success/25" />
              接続済み
            </span>
          )}
        </div>
      </div>

      {/* config form (idle/error only) */}
      {(connStatus === 'idle' || connStatus === 'error') && (
        <div className="space-y-3 rounded-xl border border-line bg-surface p-4 shadow-sm">
          <RelayUrlField value={relayUrl} onChange={setRelayUrl} />
          {error && (
            <p className="rounded-md border border-danger/40 bg-danger-soft px-3 py-2 font-mono text-xs text-danger">
              {error}
            </p>
          )}
        </div>
      )}

      {/* main grid */}
      <div className="grid items-start gap-4 lg:grid-cols-[minmax(0,1fr)_220px]">
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
            <div className="mt-3 space-y-3 rounded-xl border border-line bg-surface p-4 shadow-sm">
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
    </main>
  )
}
