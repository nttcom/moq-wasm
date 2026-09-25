/// Where decoded live frames go. Chrome exposes `MediaStreamTrackGenerator`,
/// which feeds a video element through a MediaStream; Safari and Firefox do
/// not (Safari's `VideoTrackGenerator` only exists in workers), so there the
/// frames are drawn onto a canvas instead.
export interface LivePictureSink {
  readonly element: HTMLElement
  present(frame: VideoFrame): void
  detach(): void
}

type TrackGeneratorConstructor = new (init: { kind: 'video' }) => {
  readonly writable: WritableStream<VideoFrame>
} & MediaStreamTrack

function trackGeneratorConstructor(): TrackGeneratorConstructor | undefined {
  return (globalThis as { MediaStreamTrackGenerator?: TrackGeneratorConstructor }).MediaStreamTrackGenerator
}

class TrackGeneratorSink implements LivePictureSink {
  private readonly track: InstanceType<TrackGeneratorConstructor>
  private readonly writer: WritableStreamDefaultWriter<VideoFrame>

  constructor(
    readonly element: HTMLVideoElement,
    generator: TrackGeneratorConstructor
  ) {
    this.track = new generator({ kind: 'video' })
    this.writer = this.track.writable.getWriter()
  }

  /// A video element whose stream has not produced a frame yet never reaches
  /// loadedmetadata and keeps the tab in its loading state, so the stream is
  /// attached with the first frame.
  present(frame: VideoFrame): void {
    if (this.element.srcObject === null) {
      this.element.srcObject = new MediaStream([this.track])
    }
    if (this.writer.desiredSize === null || this.writer.desiredSize <= 0) {
      frame.close()
      return
    }
    void this.writer
      .write(frame)
      .catch(() => undefined)
      .finally(() => frame.close())
  }

  detach(): void {
    this.element.srcObject = null
  }
}

class CanvasSink implements LivePictureSink {
  private readonly context: CanvasRenderingContext2D | null

  constructor(
    readonly element: HTMLCanvasElement,
    private readonly onPresented: (picture: HTMLElement) => void
  ) {
    this.context = element.getContext('2d')
  }

  present(frame: VideoFrame): void {
    if (this.context) {
      if (this.element.width !== frame.displayWidth || this.element.height !== frame.displayHeight) {
        this.element.width = frame.displayWidth
        this.element.height = frame.displayHeight
      }
      this.context.drawImage(frame, 0, 0)
      this.onPresented(this.element)
    }
    frame.close()
  }

  detach(): void {
    this.context?.clearRect(0, 0, this.element.width, this.element.height)
  }
}

export type LivePictureKind = 'track-generator' | 'canvas'

export function createLivePictureSink(
  video: HTMLVideoElement,
  canvas: HTMLCanvasElement,
  onPresented: (picture: HTMLElement) => void,
  preferred?: LivePictureKind
): LivePictureSink {
  const generator = trackGeneratorConstructor()
  if (generator && preferred !== 'canvas') {
    return new TrackGeneratorSink(video, generator)
  }
  return new CanvasSink(canvas, onPresented)
}
