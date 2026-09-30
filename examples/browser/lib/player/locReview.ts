import { ReviewPlayout } from './reviewPlayout'
import type { ReviewFrame } from './rewind'
import type { Playhead } from './deliveryObserver'
import type { LogLevel } from './trackContext'

const REVIEW_VIDEO_AHEAD_FRAMES = 30
const REVIEW_HANDOVER_MICROS = 500_000

export type LocReviewCallbacks = {
  onFrameShown(captureMicros: number, playhead: Playhead | undefined): void
  onLog(level: LogLevel, message: string): void
}

export class LocReview {
  readonly playout: ReviewPlayout
  private readonly frameIds = new Map<number, Omit<Playhead, 'captureMicros'>>()

  constructor(
    readonly canvas: HTMLCanvasElement,
    private readonly callbacks: LocReviewCallbacks
  ) {
    this.playout = new ReviewPlayout(
      (frame) => this.show(frame),
      (message) => callbacks.onLog('error', message)
    )
  }

  clearFrameIds(): void {
    this.frameIds.clear()
  }

  /// The window's audio is decoded up front. Frames are decoded a little ahead
  /// of their presentation, not the whole window at once: decoded frames hold
  /// GPU memory until they are shown. The function returns shortly before the
  /// last frame is due so the next window is decoded in time to follow on.
  async play(
    frames: ReviewFrame[],
    audio: ReviewFrame[],
    config: VideoDecoderConfig,
    audioConfig: AudioDecoderConfig | undefined,
    originMicros: number,
    isCurrent: () => boolean
  ): Promise<boolean> {
    if (audioConfig) {
      this.playout.decodeAudio(audio, audioConfig)
    }
    const decoder = new VideoDecoder({
      output: (frame) => {
        if (!isCurrent()) {
          frame.close()
          return
        }
        this.playout.presentVideo(frame)
      },
      error: (error) => this.callbacks.onLog('error', `review decoder: ${error.message}`)
    })
    decoder.configure(config)

    const origin = frames[0].captureMicros ?? originMicros
    for (const frame of frames) {
      while (isCurrent() && this.playout.queuedVideo + decoder.decodeQueueSize > REVIEW_VIDEO_AHEAD_FRAMES) {
        await new Promise((resolve) => setTimeout(resolve, 20))
      }
      if (!isCurrent() || decoder.state === 'closed') {
        break
      }
      if (frame.captureMicros !== undefined && frame.requestId !== undefined) {
        this.frameIds.set(frame.captureMicros, {
          kind: 'fetch',
          trackAlias: frame.requestId,
          groupId: frame.groupId,
          objectId: frame.objectId
        })
      }
      decoder.decode(
        new EncodedVideoChunk({
          type: frame.objectId === 0n ? 'key' : 'delta',
          timestamp: frame.captureMicros ?? origin,
          data: frame.data
        })
      )
    }
    if (decoder.state !== 'closed') {
      await decoder.flush().catch(() => undefined)
      decoder.close()
    }
    const last = frames[frames.length - 1]?.captureMicros
    if (last !== undefined) {
      await this.playout.waitUntilDue(last - REVIEW_HANDOVER_MICROS, isCurrent)
    }
    return isCurrent()
  }

  private show(frame: VideoFrame): void {
    const ids = this.frameIds.get(frame.timestamp)
    this.frameIds.delete(frame.timestamp)
    const context = this.canvas.getContext('2d')
    if (context) {
      this.canvas.width = frame.displayWidth
      this.canvas.height = frame.displayHeight
      context.drawImage(frame, 0, 0)
      this.callbacks.onFrameShown(frame.timestamp, ids && { ...ids, captureMicros: frame.timestamp })
    }
    frame.close()
  }
}
