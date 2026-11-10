import type { MoqSession } from 'moq-wasm'
import { type LocalBroadcast, createBroadcast } from 'moq-wasm/media'
import { type CameraId, type VideoSource } from '../types/monitoring'

const log = (...args: unknown[]) => console.log('[pub][camera]', ...args)

const VIDEO_ENCODING = {
  codec: 'avc3.640028', // H.264 High Profile Level 4.0 (in-band SPS/PPS; encoder emits annex-b)
  hardwareAcceleration: 'prefer-hardware' as const
}
const MAIN_VIDEO = { width: 1280, height: 720, bitrate: 1_000_000, framerate: 30 }

const CAM_COLORS: Record<CameraId, string> = {
  cam01: '#7a1a1a',
  cam02: '#1a3a7a',
  cam03: '#1a6a35',
  cam04: '#7a4a10'
}

export class CameraPublisher {
  private stream: MediaStream | null = null
  private videoEl: HTMLVideoElement | null = null
  private broadcast: LocalBroadcast | null = null
  private rafId: number | null = null

  constructor(
    private readonly camId: CameraId,
    private readonly source: VideoSource
  ) {}

  async start(session: MoqSession, namespace: string[]): Promise<MediaStream> {
    let encoding: typeof VIDEO_ENCODING | typeof this.SYNTHETIC_ENCODING = VIDEO_ENCODING

    if (this.source.type === 'synthetic') {
      this.stream = this.startSyntheticCamera()
      encoding = this.SYNTHETIC_ENCODING
    } else if (this.source.type === 'camera') {
      this.stream = await this.startRealCamera()
    } else {
      const url = this.source.fileUrl ?? ''
      this.stream = await this.startVideoFileCamera(url)
    }

    // A video file's captured stream carries its sound too; the monitor shows video only.
    this.broadcast = await createBroadcast(session, namespace, {
      onError: (err) => console.error('[pub][camera] publish error', { camId: this.camId, err })
    })
    await this.broadcast.addSource('camera', {
      label: this.camId,
      video: this.stream.getVideoTracks()[0],
      encoding,
      main: MAIN_VIDEO
    })
    log('publishing', { camId: this.camId, source: this.source.type, namespace })
    return this.stream
  }

  // Canvas outputs BGRA frames. Switch to prefer-software to avoid HW encoder issues with BGRA.
  private readonly SYNTHETIC_ENCODING = { ...VIDEO_ENCODING, hardwareAcceleration: 'prefer-software' as const }

  private startSyntheticCamera(): MediaStream {
    const canvas = document.createElement('canvas')
    canvas.width = MAIN_VIDEO.width
    canvas.height = MAIN_VIDEO.height
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
    this.rafId = setInterval(draw, 1000 / MAIN_VIDEO.framerate) as unknown as number

    return canvas.captureStream(MAIN_VIDEO.framerate)
  }

  private async startRealCamera(): Promise<MediaStream> {
    const stream = await navigator.mediaDevices.getUserMedia({
      video: { width: MAIN_VIDEO.width, height: MAIN_VIDEO.height, frameRate: MAIN_VIDEO.framerate },
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

  stop(): void {
    log('stopping', { camId: this.camId })
    if (this.rafId !== null) clearInterval(this.rafId)
    this.broadcast?.close().catch((err) => console.warn('[pub][camera] close failed', { camId: this.camId, err }))
    this.broadcast = null
    this.stream?.getTracks().forEach((t) => t.stop())
    this.stream = null
    if (this.videoEl) {
      this.videoEl.pause()
      this.videoEl.src = ''
      this.videoEl = null
    }
  }
}
