import { useEffect, useRef, useState, useCallback } from 'react'
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
import { MonitorSession } from '../monitor/monitorSession'
import { CameraSubscriber } from '../monitor/cameraSubscriber'
import { useUrlSync } from '../hooks/useUrlSync'

interface Props {
  location: string
  relayUrl: string
}

type ConnStatus = 'idle' | 'connecting' | 'connected' | 'error'

export function MonitoringRoom({ location: defaultLocation, relayUrl: defaultRelayUrl }: Props) {
  const [location, setLocation] = useState(defaultLocation)
  const [relayUrl, setRelayUrl] = useState(defaultRelayUrl)
  const [stageId, setStageId] = useState<CameraId>('cam01')
  const [mode, setMode] = useState<MonitorMode>('live')
  const [connStatus, setConnStatus] = useState<ConnStatus>('idle')
  const [subscribedCameras, setSubscribedCameras] = useState<Set<CameraId>>(new Set())
  const [subscribingCameras, setSubscribingCameras] = useState<Set<CameraId>>(new Set())
  const [error, setError] = useState<string | null>(null)

  // Seek state
  const [currentGroupId, setCurrentGroupId] = useState<bigint | null>(null)
  const [latestGroupId, setLatestGroupId] = useState<bigint | null>(null)
  const [firstGroupId, setFirstGroupId] = useState<bigint | null>(null)
  const [entryGroupId, setEntryGroupId] = useState<bigint | null>(null)
  const [fetchWindow, setFetchWindow] = useState<{ startGroup: bigint; endGroup: bigint } | null>(null)

  const sessionRef = useRef<MonitorSession | null>(null)
  const subscribersRef = useRef<Map<CameraId, CameraSubscriber>>(new Map())
  const canvasRefs = useRef<Map<CameraId, HTMLCanvasElement>>(new Map())
  const stageIdRef = useRef<CameraId>(stageId)

  // Review
  const reviewBufferRef = useRef<Map<bigint, Uint8Array>>(new Map())
  const activeFetchIdRef = useRef<bigint | null>(null)
  const fetchGenRef = useRef(0)

  const thumbIds = ALL_CAMERA_IDS.filter((id) => id !== stageId)
  const isConnected = connStatus === 'connected'

  useUrlSync({ location, relay: relayUrl })

  useEffect(() => {
    stageIdRef.current = stageId
  }, [stageId])

  useEffect(() => {
    return () => {
      subscribersRef.current.forEach((s) => s.dispose())
      sessionRef.current?.disconnect().catch(console.error)
    }
  }, [])

  const handleConnect = async () => {
    setConnStatus('connecting')
    setError(null)
    try {
      const session = new MonitorSession()
      await session.connect(relayUrl)
      sessionRef.current = session
      setConnStatus('connected')
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Connect failed')
      setConnStatus('error')
    }
  }

  const handleSubscribe = async (camId: CameraId) => {
    const session = sessionRef.current
    if (!session) return

    setSubscribingCameras((prev) => new Set([...prev, camId]))
    try {
      const subscriber = new CameraSubscriber(camId)
      subscriber.onGroupIdUpdate = (first, latest) => {
        if (camId !== stageIdRef.current) return
        setFirstGroupId(first)
        setLatestGroupId(latest)
      }
      const existingCanvas = canvasRefs.current.get(camId)
      if (existingCanvas) subscriber.setCanvas(existingCanvas)
      subscribersRef.current.set(camId, subscriber)

      const ok = await session.subscribeVideo(location, camId, (groupId, msg) => {
        subscriber.handleObject(groupId, msg)
      })

      if (ok) {
        setSubscribedCameras((prev) => new Set([...prev, camId]))
      } else {
        subscriber.dispose()
        subscribersRef.current.delete(camId)
      }
    } finally {
      setSubscribingCameras((prev) => {
        const s = new Set(prev)
        s.delete(camId)
        return s
      })
    }
  }

  const handleCanvasReady = useCallback(
    (camId: CameraId) => (canvas: HTMLCanvasElement | null) => {
      subscribersRef.current.get(camId)?.setCanvas(canvas)
      if (canvas) {
        canvasRefs.current.set(camId, canvas)
      }
    },
    []
  )

  const cleanupReview = useCallback((targetStageId: CameraId) => {
    fetchGenRef.current++
    if (activeFetchIdRef.current !== null && sessionRef.current) {
      sessionRef.current.clearFetch(activeFetchIdRef.current)
      activeFetchIdRef.current = null
    }
    reviewBufferRef.current = new Map()
    subscribersRef.current.get(targetStageId)!.resumeLive()
    setFetchWindow(null)
    setCurrentGroupId(null)
    setEntryGroupId(null)
  }, [])

  const startFetch = useCallback(
    async (seekGroup: bigint, camId: CameraId, targetObjectId?: bigint) => {
      const session = sessionRef.current
      const subscriber = subscribersRef.current.get(camId)
      if (!session || !subscriber) return

      const myGen = ++fetchGenRef.current

      const first = subscriber.firstReceivedGroupId ?? seekGroup
      const windowStart = seekGroup - first > 6n ? seekGroup - 6n : first
      const rawWindowEnd = seekGroup + 5n
      const windowEnd = latestGroupId !== null && rawWindowEnd > latestGroupId ? latestGroupId : rawWindowEnd

      if (activeFetchIdRef.current !== null) {
        session.clearFetch(activeFetchIdRef.current)
        activeFetchIdRef.current = null
      }
      reviewBufferRef.current = new Map()

      setCurrentGroupId(seekGroup)
      setFetchWindow({ startGroup: windowStart, endGroup: windowEnd })

      const targetGroup = seekGroup
      const needSequential = targetObjectId !== undefined && targetObjectId > 0n
      const seqBuffer = new Map<bigint, Uint8Array>()
      let seqDecoded = false

      try {
        const fetchId = await session.fetchVideo(location, camId, windowStart, windowEnd, (msg) => {
          if (fetchGenRef.current !== myGen) return
          const payload = new Uint8Array(msg.objectPayload)

          // Always store I-frames for step navigation
          if (msg.objectId === 0n) {
            reviewBufferRef.current.set(msg.groupId, payload)
          }

          if (msg.groupId === targetGroup) {
            if (needSequential) {
              // Collect all frames up to targetObjectId for sequential decode
              if (msg.objectId <= targetObjectId!) {
                seqBuffer.set(msg.objectId, payload)
              }
              if (msg.objectId === targetObjectId && !seqDecoded) {
                seqDecoded = true
                subscriber.decodeFrameSequential(seqBuffer, targetObjectId!)
              }
            } else if (msg.objectId === 0n) {
              subscriber.decodeFrame(msg.groupId, payload)
            }
          }
        })
        if (fetchGenRef.current !== myGen) {
          session.clearFetch(fetchId)
          return
        }
        activeFetchIdRef.current = fetchId
      } catch (e) {
        if (fetchGenRef.current === myGen) {
          console.error('[review] FETCH failed', e)
        }
      }
    },
    [location]
  )

  const handleStageClick = async () => {
    if (!subscribedCameras.has(stageId)) return
    if (mode === 'review') return

    const subscriber = subscribersRef.current.get(stageId)
    if (!subscriber) return
    const latest = subscriber.latestReceivedGroupId
    const first = subscriber.firstReceivedGroupId
    if (latest === null || first === null) return

    // Pause live decode before fetching
    subscriber.paused = true
    setMode('review')
    setEntryGroupId(latest)

    // Seek to the exact frame being displayed at click time
    const seekGroup = subscriber.lastSentGroupId ?? latest
    const seekObjectId = subscriber.lastSentObjectId

    await startFetch(seekGroup, stageId, seekObjectId).catch((e) => {
      console.error('[review] FETCH failed', e)
      cleanupReview(stageId)
      setMode('live')
    })
  }

  const handleStepBack = () => {
    if (currentGroupId === null || firstGroupId === null) return
    if (currentGroupId <= firstGroupId) return
    const next = currentGroupId - 1n
    if (fetchWindow && next < fetchWindow.startGroup) {
      startFetch(next, stageId).catch(console.error)
      return
    }
    setCurrentGroupId(next)
    const payload = reviewBufferRef.current.get(next)
    const subscriber = subscribersRef.current.get(stageId)
    if (payload && subscriber) subscriber.decodeFrame(next, payload)
  }

  const handleStepForward = () => {
    if (currentGroupId === null || latestGroupId === null) return
    if (currentGroupId >= latestGroupId) return
    const next = currentGroupId + 1n
    if (fetchWindow && next > fetchWindow.endGroup) {
      startFetch(next, stageId).catch(console.error)
      return
    }
    setCurrentGroupId(next)
    const payload = reviewBufferRef.current.get(next)
    const subscriber = subscribersRef.current.get(stageId)
    if (payload && subscriber) subscriber.decodeFrame(next, payload)
  }

  const handleJump = (seekGroup: bigint) => {
    startFetch(seekGroup, stageId).catch(console.error)
  }

  const handleReturnToLive = () => {
    cleanupReview(stageId)
    setMode('live')
  }

  const handleThumbSelect = (id: CameraId) => {
    if (mode === 'review') {
      cleanupReview(stageId)
    }
    const stageCanvas = canvasRefs.current.get(stageId)
    if (stageCanvas) {
      stageCanvas.getContext('2d')?.clearRect(0, 0, stageCanvas.width, stageCanvas.height)
    }
    const newSubscriber = subscribersRef.current.get(id)
    setFirstGroupId(newSubscriber?.firstReceivedGroupId ?? null)
    setLatestGroupId(newSubscriber?.latestReceivedGroupId ?? null)
    setStageId(id)
    setMode('live')
  }

  const subButtonForCamera = isConnected ? handleSubscribe : undefined

  const canStepBack = currentGroupId !== null && firstGroupId !== null && currentGroupId > firstGroupId
  const canStepForward = currentGroupId !== null && latestGroupId !== null && currentGroupId < latestGroupId

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
            mode={mode}
            connState={isConnected ? 'connected' : 'closed'}
            currentGroupId={currentGroupId}
            latestGroupId={latestGroupId}
            onCanvasReady={handleCanvasReady(stageId)}
            onSubscribe={subButtonForCamera ? () => subButtonForCamera(stageId) : undefined}
            isSubscribed={subscribedCameras.has(stageId)}
            isSubscribing={subscribingCameras.has(stageId)}
            onClick={handleStageClick}
          />
          {mode === 'review' && (
            <div className="mt-3 space-y-2">
              {firstGroupId !== null && entryGroupId !== null && currentGroupId !== null && (
                <SeekBar
                  firstGroupId={firstGroupId}
                  entryGroupId={entryGroupId}
                  latestGroupId={latestGroupId ?? entryGroupId}
                  currentGroupId={currentGroupId}
                  fetchWindow={fetchWindow}
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
            onSelect={handleThumbSelect}
            onCanvasReady={handleCanvasReady}
            onSubscribe={subButtonForCamera}
            subscribedCameras={subscribedCameras}
            subscribingCameras={subscribingCameras}
          />
        )}
      </div>

      {/* debug bar */}
      <DebugBar
        connState={isConnected ? 'connected' : connStatus === 'idle' ? 'closed' : 'closed'}
        relayUrl={relayUrl}
        subscribedCameras={[...subscribedCameras]}
        fetchingCamera={mode === 'review' ? stageId : null}
        fetchWindow={fetchWindow}
      />
    </div>
  )
}
