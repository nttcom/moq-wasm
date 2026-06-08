import { MediaTransportState } from '../../../../utils/media/transportState'
import { sendVideoChunkViaMoqt } from '../../../../utils/media/videoTransport'
import type { PublisherSession } from './publisherSession'
import { type CameraId } from '../types/monitoring'

const log = (...args: unknown[]) => console.log('[pub][camera]', ...args)

const KEYFRAME_INTERVAL_FRAMES = 30  // 30fps × 1s = 1 GoP per second
const VIDEO_CONFIG = {
  codec: 'avc1.640028',  // H.264 High Profile Level 4.0
  width: 1280,
  height: 720,
  bitrate: 1_000_000,
  framerate: 30,
  hardwareAcceleration: 'prefer-hardware' as const,
}

const CAM_COLORS: Record<CameraId, string> = {
  cam01: '#7a1a1a',
  cam02: '#1a3a7a',
  cam03: '#1a6a35',
  cam04: '#7a4a10',
}

export class CameraPublisher {
  private worker: Worker | null = null
  private stream: MediaStream | null = null
  private readonly transportState = new MediaTransportState()
  private chunkCount = 0
  private rafId: number | null = null

  constructor(
    private readonly session: PublisherSession,
    private readonly camId: CameraId,
  ) {}

  async start(): Promise<MediaStream> {
    // --- Video source (switch by commenting one line) ---
    const synthetic = true
    // const synthetic = false
    this.stream = synthetic ? await this.startSyntheticCamera() : await this.startRealCamera()
    const encoderConfig = synthetic ? this.SYNTHETIC_CONFIG : VIDEO_CONFIG
    // ---------------------------------------------------

    const track = this.stream.getVideoTracks()[0]

    log('starting encoder worker', { keyframeInterval: KEYFRAME_INTERVAL_FRAMES, config: encoderConfig })
    this.worker = new Worker(
      new URL('../../../../utils/media/encoders/videoEncoder.ts', import.meta.url),
      { type: 'module' }
    )
    this.worker.postMessage({ type: 'keyframeInterval', keyframeInterval: KEYFRAME_INTERVAL_FRAMES })
    this.worker.postMessage({ type: 'encoderConfig', config: encoderConfig })
    this.worker.onmessage = (e) => {
      if (e.data.type === 'chunk') {
        const { chunk, metadata, captureTimestampMicros } = e.data
        if (this.chunkCount === 0) {
          log('first encoded chunk', { type: chunk.type, timestamp: chunk.timestamp })
        }
        this.chunkCount++
        this.sendChunk(chunk, metadata, captureTimestampMicros).catch((err) =>
          console.error('[pub][camera] sendChunk error', err)
        )
      } else if (e.data.type === 'bitrate') {
        if (this.chunkCount % 150 === 0) log('bitrate', { kbps: e.data.kbps })
      } else if (e.data.type === 'configError') {
        console.error('[pub][camera] encoder config error', e.data)
      }
    }

    const processor = new (window as any).MediaStreamTrackProcessor({ track })
    const videoStream: ReadableStream<VideoFrame> = processor.readable
    log('sending video stream to encoder worker')
    this.worker.postMessage({ type: 'videoStream', videoStream }, [videoStream])

    return this.stream
  }

  // --- Synthetic canvas source (for debug / routing verification) ---
  // Canvas outputs BGRA frames. Switch to prefer-software to avoid HW encoder issues with BGRA.
  private readonly SYNTHETIC_CONFIG = { ...VIDEO_CONFIG, hardwareAcceleration: 'prefer-software' as const }

  private startSyntheticCamera(): MediaStream {
    log('synthetic source starting', { camId: this.camId })
    const canvas = document.createElement('canvas')
    canvas.width = VIDEO_CONFIG.width
    canvas.height = VIDEO_CONFIG.height
    const ctx = canvas.getContext('2d')!
    const bg = CAM_COLORS[this.camId] ?? '#333'

    const draw = () => {
      const now = new Date().toISOString().replace('T', ' ').substring(0, 23)
      ctx.fillStyle = bg
      ctx.fillRect(0, 0, canvas.width, canvas.height)
      // grid pattern
      ctx.strokeStyle = 'rgba(255,255,255,0.08)'
      ctx.lineWidth = 1
      for (let x = 0; x < canvas.width; x += 80) { ctx.beginPath(); ctx.moveTo(x, 0); ctx.lineTo(x, canvas.height); ctx.stroke() }
      for (let y = 0; y < canvas.height; y += 80) { ctx.beginPath(); ctx.moveTo(0, y); ctx.lineTo(canvas.width, y); ctx.stroke() }
      // camera ID
      ctx.fillStyle = 'white'
      ctx.font = 'bold 120px monospace'
      ctx.textAlign = 'center'
      ctx.textBaseline = 'middle'
      ctx.fillText(this.camId.toUpperCase(), canvas.width / 2, canvas.height / 2 - 60)
      // camera name
      ctx.font = '60px sans-serif'
      ctx.fillText(this.camId.toUpperCase(), canvas.width / 2, canvas.height / 2 + 40)
      // timestamp
      ctx.font = '36px monospace'
      ctx.fillStyle = 'rgba(255,255,255,0.7)'
      ctx.fillText(now, canvas.width / 2, canvas.height / 2 + 130)
      // live dot
      ctx.fillStyle = '#ff4444'
      ctx.beginPath()
      ctx.arc(60, 60, 18, 0, Math.PI * 2)
      ctx.fill()
    }
    // setInterval instead of requestAnimationFrame — keeps running in background tabs
    draw()
    this.rafId = setInterval(draw, 1000 / VIDEO_CONFIG.framerate) as unknown as number

    return canvas.captureStream(VIDEO_CONFIG.framerate)
  }

  // --- Real camera source (getUserMedia) ---
  private async startRealCamera(): Promise<MediaStream> {
    log('getUserMedia starting...')
    const stream = await navigator.mediaDevices.getUserMedia({
      video: { width: VIDEO_CONFIG.width, height: VIDEO_CONFIG.height, frameRate: VIDEO_CONFIG.framerate },
      audio: false,
    })
    const settings = stream.getVideoTracks()[0].getSettings()
    log('getUserMedia success', { width: settings.width, height: settings.height })
    return stream
  }

  private async sendChunk(
    chunk: EncodedVideoChunk,
    metadata: EncodedVideoChunkMetadata | undefined,
    captureTimestampMicros?: number
  ): Promise<void> {
    const aliases = this.session.getVideoTrackAliases()

    if (chunk.type === 'key') {
      log('keyframe', {
        aliases: aliases.map(a => a.toString()),
        groupId: (this.transportState.getVideoGroupId() + 1n).toString(),
      })
    }

    if (aliases.length === 0) return

    const rawClient = this.session.getRawClient()
    await sendVideoChunkViaMoqt({
      chunk,
      metadata,
      captureTimestampMicros,
      trackAliases: aliases,
      publisherPriority: 0,
      client: rawClient,
      transportState: this.transportState,
      sender: async (alias, groupId, subgroupId, objectId, payload, client, locHeader) => {
        await client.sendSubgroupObject(alias, groupId, subgroupId, objectId, undefined, payload, locHeader)
      },
    })
  }

  stop(): void {
    log('stopping')
    if (this.rafId !== null) clearInterval(this.rafId)
    this.worker?.terminate()
    this.worker = null
    this.stream?.getTracks().forEach((t) => t.stop())
    this.stream = null
  }
}
