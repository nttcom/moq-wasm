const PLAYBACK_BUFFER_THRESHOLD_SECONDS = 1
const RETAINED_BEHIND_PLAYHEAD_SECONDS = 30
const EVICT_WHEN_BEHIND_SECONDS = 60

export type MseTrackSource = {
  mimeType: string
  initSegment: Uint8Array
}

export type MseSources = {
  video: MseTrackSource
  audio?: MseTrackSource
  /// Offset from the first buffered video frame at which playback starts, so a
  /// review that decodes from a keyframe can begin at a later frame.
  startAtSeconds?: number
}

type Buffered = {
  sourceBuffer: SourceBuffer
  queue: Uint8Array[]
}

function covers(ranges: TimeRanges, seconds: number): boolean {
  for (let index = 0; index < ranges.length; index += 1) {
    if (ranges.start(index) <= seconds && seconds <= ranges.end(index)) {
      return true
    }
  }
  return false
}

/// One MediaSource on a video element with a SourceBuffer per track. Appends
/// are queued because a SourceBuffer rejects appendBuffer while it is updating,
/// and playback starts once a second of video is buffered.
export class MseSink {
  private readonly mediaSource = new MediaSource()
  private readonly objectUrl: string
  private video: Buffered | undefined
  private audio: Buffered | undefined
  private startedAtSeconds: number | undefined

  private constructor(
    readonly element: HTMLVideoElement,
    private readonly startAtSeconds: number
  ) {
    this.objectUrl = URL.createObjectURL(this.mediaSource)
  }

  static async open(element: HTMLVideoElement, { video, audio, startAtSeconds = 0 }: MseSources): Promise<MseSink> {
    const sink = new MseSink(element, startAtSeconds)
    await new Promise<void>((resolve, reject) => {
      sink.mediaSource.addEventListener('sourceopen', () => resolve(), { once: true })
      sink.mediaSource.addEventListener('error', () => reject(new Error('MediaSource failed to open')), {
        once: true
      })
      element.srcObject = null
      element.muted = audio === undefined
      element.src = sink.objectUrl
    })
    sink.video = sink.attach(video)
    sink.audio = audio ? sink.attach(audio) : undefined
    sink.appendVideo(video.initSegment)
    if (audio) {
      sink.appendAudio(audio.initSegment)
    }
    return sink
  }

  appendVideo(data: Uint8Array): void {
    this.enqueue(this.video, data)
  }

  appendAudio(data: Uint8Array): void {
    this.enqueue(this.audio, data)
  }

  bufferedEnd(): number | undefined {
    const buffered = this.video?.sourceBuffer.buffered
    return buffered && buffered.length > 0 ? buffered.end(buffered.length - 1) : undefined
  }

  /// Measured from where the first buffered video began, which eviction may
  /// later remove from the buffered ranges.
  secondsFromStart(): number {
    return this.startedAtSeconds === undefined ? 0 : this.element.currentTime - this.startedAtSeconds
  }

  close(): void {
    this.element.muted = true
    this.element.removeAttribute('src')
    this.element.load()
    URL.revokeObjectURL(this.objectUrl)
  }

  private attach(source: MseTrackSource): Buffered {
    const sourceBuffer = this.mediaSource.addSourceBuffer(source.mimeType)
    const buffered: Buffered = { sourceBuffer, queue: [] }
    sourceBuffer.addEventListener('updateend', () => {
      this.startWhenBuffered()
      this.evictBehindPlayhead(buffered)
      this.flush(buffered)
    })
    return buffered
  }

  private enqueue(buffered: Buffered | undefined, data: Uint8Array): void {
    if (!buffered || this.mediaSource.readyState !== 'open') {
      return
    }
    buffered.queue.push(data)
    this.flush(buffered)
  }

  private flush(buffered: Buffered): void {
    if (buffered.sourceBuffer.updating || buffered.queue.length === 0) {
      return
    }
    buffered.sourceBuffer.appendBuffer(buffered.queue.shift()!.slice())
  }

  /// With an audio track, playback starts where both tracks have data: the
  /// element would otherwise sit on the first frame until the audio arrives,
  /// and the audio may begin a little after the video when the first keyframe
  /// lands while the MediaSource is being opened.
  private startWhenBuffered(): void {
    const end = this.bufferedEnd()
    if (this.startedAtSeconds !== undefined || end === undefined) {
      return
    }
    const bufferStart = this.video!.sourceBuffer.buffered.start(0)
    let start = bufferStart + this.startAtSeconds
    const audioRanges = this.audio?.sourceBuffer.buffered
    if (audioRanges) {
      if (audioRanges.length === 0) {
        return
      }
      start = Math.max(start, audioRanges.start(0))
      if (!covers(audioRanges, start)) {
        return
      }
    }
    if (end - start < PLAYBACK_BUFFER_THRESHOLD_SECONDS) {
      return
    }
    this.startedAtSeconds = bufferStart
    this.element.currentTime = start
    void this.element.play().catch(() => undefined)
  }

  private evictBehindPlayhead(buffered: Buffered): void {
    const ranges = buffered.sourceBuffer.buffered
    if (ranges.length === 0 || buffered.sourceBuffer.updating) {
      return
    }
    const behind = this.element.currentTime - ranges.start(0)
    if (behind > EVICT_WHEN_BEHIND_SECONDS) {
      buffered.sourceBuffer.remove(0, this.element.currentTime - RETAINED_BEHIND_PLAYHEAD_SECONDS)
    }
  }
}
