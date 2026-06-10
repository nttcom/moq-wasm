import { MediaTransportState } from '../../../../utils/media/transportState'
import { sendVideoChunkViaMoqt } from '../../../../utils/media/videoTransport'
import type { PublisherSession } from './publisherSession'
import { type CameraId, type VideoSource } from '../types/monitoring'

const log = (...args: unknown[]) => console.log('[pub][camera]', ...args)

const KEYFRAME_INTERVAL_FRAMES = 30 // 30fps × 1s = 1 GoP per second
const BITRATE_LOG_INTERVAL_CHUNKS = 900 // ~30 seconds at 30fps
const VIDEO_CONFIG = {
  codec: 'avc1.640028', // H.264 High Profile Level 4.0
  width: 1280,
  height: 720,
  bitrate: 1_000_000,
  framerate: 30,
  hardwareAcceleration: 'prefer-hardware' as const
}

const CAM_COLORS: Record<CameraId, string> = {
  cam01: '#7a1a1a',
  cam02: '#1a3a7a',
  cam03: '#1a6a35',
  cam04: '#7a4a10'
}

export class CameraPublisher {
  private worker: Worker | null = null
  private stream: MediaStream | null = null
  private videoEl: HTMLVideoElement | null = null
  private readonly transportState = new MediaTransportState()
  private chunkCount = 0
  private rafId: number | null = null

  constructor(
    private readonly session: PublisherSession,
    private readonly camId: CameraId,
    private readonly source: VideoSource
  ) {}

  async start(): Promise<MediaStream> {
    let encoderConfig: typeof VIDEO_CONFIG | typeof this.SYNTHETIC_CONFIG = VIDEO_CONFIG

    if (this.source.type === 'synthetic') {
      this.stream = this.startSyntheticCamera()
      encoderConfig = this.SYNTHETIC_CONFIG
    } else if (this.source.type === 'camera') {
      this.stream = await this.startRealCamera()
    } else {
      const url = this.source.fileUrl ?? ''
      this.stream = await this.startVideoFileCamera(url)
    }

    const track = this.stream.getVideoTracks()[0]

    this.worker = new Worker(new URL('../../../../utils/media/encoders/videoEncoder.ts', import.meta.url), {
      type: 'module'
    })
    this.worker.postMessage({ type: 'keyframeInterval', keyframeInterval: KEYFRAME_INTERVAL_FRAMES })
    this.worker.postMessage({ type: 'encoderConfig', config: encoderConfig })
    this.worker.onmessage = (e) => {
      if (e.data.type === 'chunk') {
        const { chunk, metadata, captureTimestampMicros } = e.data
        if (this.chunkCount === 0) {
          log('encoder ready, first chunk produced', { camId: this.camId, source: this.source.type, type: chunk.type })
        }
        if (this.chunkCount % BITRATE_LOG_INTERVAL_CHUNKS === 0 && this.chunkCount > 0) {
          log('bitrate', { camId: this.camId, kbps: e.data.kbps })
        }
        this.chunkCount++
        this.sendChunk(chunk, metadata, captureTimestampMicros).catch((err) =>
          console.error('[pub][camera] sendChunk error', { camId: this.camId, err })
        )
      } else if (e.data.type === 'bitrate') {
        if (this.chunkCount % BITRATE_LOG_INTERVAL_CHUNKS === 0 && this.chunkCount > 0) {
          log('bitrate', { camId: this.camId, kbps: e.data.kbps })
        }
      } else if (e.data.type === 'configError') {
        console.error('[pub][camera] encoder config error', { camId: this.camId, ...e.data })
      }
    }

    const processor = new (window as any).MediaStreamTrackProcessor({ track })
    const videoStream: ReadableStream<VideoFrame> = processor.readable
    this.worker.postMessage({ type: 'videoStream', videoStream }, [videoStream])

    return this.stream
  }

  // Canvas outputs BGRA frames. Switch to prefer-software to avoid HW encoder issues with BGRA.
  private readonly SYNTHETIC_CONFIG = { ...VIDEO_CONFIG, hardwareAcceleration: 'prefer-software' as const }

  private startSyntheticCamera(): MediaStream {
    const canvas = document.createElement('canvas')
    canvas.width = VIDEO_CONFIG.width
    canvas.height = VIDEO_CONFIG.height
    const ctx = canvas.getContext('2d')!
    const bg = CAM_COLORS[this.camId] ?? '#333'

    const draw = () => {
      const now = new Date().toISOString().replace('T', ' ').substring(0, 23)
      ctx.fillStyle = bg
      ctx.fillRect(0, 0, canvas.width, canvas.height)
      ctx.strokeStyle = 'rgba(255,255,255,0.08)'
      ctx.lineWidth = 1
      for (let x = 0; x < canvas.width; x += 80) {
        ctx.beginPath()
        ctx.moveTo(x, 0)
        ctx.lineTo(x, canvas.height)
        ctx.stroke()
      }
      for (let y = 0; y < canvas.height; y += 80) {
        ctx.beginPath()
        ctx.moveTo(0, y)
        ctx.lineTo(canvas.width, y)
        ctx.stroke()
      }
      ctx.fillStyle = 'white'
      ctx.font = 'bold 120px monospace'
      ctx.textAlign = 'center'
      ctx.textBaseline = 'middle'
      ctx.fillText(this.camId.toUpperCase(), canvas.width / 2, canvas.height / 2 - 60)
      ctx.font = '60px sans-serif'
      ctx.fillText(this.camId.toUpperCase(), canvas.width / 2, canvas.height / 2 + 40)
      ctx.font = '36px monospace'
      ctx.fillStyle = 'rgba(255,255,255,0.7)'
      ctx.fillText(now, canvas.width / 2, canvas.height / 2 + 130)
      ctx.fillStyle = '#ff4444'
      ctx.beginPath()
      ctx.arc(60, 60, 18, 0, Math.PI * 2)
      ctx.fill()
    }
    draw()
    this.rafId = setInterval(draw, 1000 / VIDEO_CONFIG.framerate) as unknown as number

    return canvas.captureStream(VIDEO_CONFIG.framerate)
  }

  private async startRealCamera(): Promise<MediaStream> {
    const stream = await navigator.mediaDevices.getUserMedia({
      video: { width: VIDEO_CONFIG.width, height: VIDEO_CONFIG.height, frameRate: VIDEO_CONFIG.framerate },
      audio: false
    })
    const settings = stream.getVideoTracks()[0].getSettings()
    log('camera ready', { camId: this.camId, width: settings.width, height: settings.height })
    return stream
  }

  private startVideoFileCamera(url: string): Promise<MediaStream> {
    return new Promise((resolve, reject) => {
      const video = document.createElement('video')
      // リスナーを src 代入より先に登録（キャッシュ済みファイルで canplay が即時発火する場合に備える）
      video.addEventListener(
        'canplay',
        () => {
          video
            .play()
            .then(() => {
              const stream = (video as any).captureStream()
              log('file source ready', { camId: this.camId, url, duration: video.duration.toFixed(1) + 's' })
              resolve(stream)
            })
            .catch(reject)
        },
        { once: true }
      )
      video.addEventListener(
        'error',
        () => {
          reject(new Error(`動画ファイルを読み込めません: ${url}`))
        },
        { once: true }
      )

      video.loop = true
      video.muted = true
      video.playsInline = true
      video.src = url
      this.videoEl = video
      video.load()
    })
  }

  private async sendChunk(
    chunk: EncodedVideoChunk,
    metadata: EncodedVideoChunkMetadata | undefined,
    captureTimestampMicros?: number
  ): Promise<void> {
    const aliases = this.session.getVideoTrackAliases()
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
      }
    })
  }

  stop(): void {
    log('stopping', { camId: this.camId })
    if (this.rafId !== null) clearInterval(this.rafId)
    this.worker?.terminate()
    this.worker = null
    this.stream?.getTracks().forEach((t) => t.stop())
    this.stream = null
    if (this.videoEl) {
      this.videoEl.pause()
      this.videoEl.src = ''
      this.videoEl = null
    }
  }
}
