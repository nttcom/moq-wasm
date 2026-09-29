import { monotonicUnixMicros } from '../../utils/media/clock'

const MICROS_PER_MILLI = 1_000

/// Decodes the video samples the publisher sends and draws each frame when
/// the wall clock reaches its capture timestamp, which is the moment the
/// publisher sent it, so the picture shows what the viewer should be showing
/// with no delay at all.
export class PublishPreview {
  private decoder: VideoDecoder | undefined
  private readonly scheduled = new Map<ReturnType<typeof setTimeout>, VideoFrame>()

  constructor(private readonly canvas: HTMLCanvasElement) {}

  start(codec: string): void {
    this.stop()
    const decoder = new VideoDecoder({
      output: (frame) => this.schedule(frame),
      error: (error) => console.warn('[publishPreview] decoder error', error)
    })
    decoder.configure({ codec, optimizeForLatency: true })
    this.decoder = decoder
  }

  decode(annexB: Uint8Array, keyframe: boolean, captureMicros: number): void {
    if (this.decoder?.state !== 'configured') {
      return
    }
    this.decoder.decode(
      new EncodedVideoChunk({ type: keyframe ? 'key' : 'delta', timestamp: captureMicros, data: annexB })
    )
  }

  stop(): void {
    if (this.decoder && this.decoder.state !== 'closed') {
      this.decoder.close()
    }
    this.decoder = undefined
    for (const [timer, frame] of this.scheduled) {
      clearTimeout(timer)
      frame.close()
    }
    this.scheduled.clear()
  }

  private schedule(frame: VideoFrame): void {
    const delayMs = (frame.timestamp - monotonicUnixMicros()) / MICROS_PER_MILLI
    if (delayMs <= 0) {
      this.draw(frame)
      return
    }
    const timer = setTimeout(() => {
      this.scheduled.delete(timer)
      this.draw(frame)
    }, delayMs)
    this.scheduled.set(timer, frame)
  }

  private draw(frame: VideoFrame): void {
    this.canvas.width = frame.displayWidth
    this.canvas.height = frame.displayHeight
    this.canvas.getContext('2d')?.drawImage(frame, 0, 0)
    frame.close()
  }
}
