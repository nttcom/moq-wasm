const MILLIS_PER_SECOND = 1_000
const MICROS_PER_MILLI = 1_000
const POSITION_STALE_MS = 1_000
/// Within this distance a chunk continues the previous one sample for sample.
/// Capture timestamps are millisecond-precise and the context clock is read a
/// render quantum at a time, so each target wobbles by a few milliseconds; a
/// chunk placed on its own target would leave a gap or an overlap each time.
const CONTIGUOUS_TOLERANCE_MS = 20
const RENDERING_POLL_MS = 10

type Output = {
  context: AudioContext
  gain: GainNode
}

type Chunk = {
  buffer: AudioBuffer
  captureMicros: number | undefined
  atMs: number
}

type Scheduled = {
  captureMicros: number | undefined
  atMs: number
}

/// Plays decoded audio through an AudioContext running at the stream's sample
/// rate. Chunks are appended at a write head so the waveform stays continuous,
/// and the distance between the write head and the time the clock asked for
/// is reported as drift, which lets the clock follow the audio device. Chunks
/// that arrive before the context renders wait for it: a context reports
/// `running` before its clock has started and its output timestamp reads zero
/// until the device has called back, and a chunk placed against either lands
/// far from where the later ones do.
export class AudioPlayout {
  private output: Output | undefined
  private volume = 1
  private readonly sources = new Set<AudioBufferSourceNode>()
  private readonly waiting: Chunk[] = []
  private writeHead: number | undefined
  private renderingPoll: ReturnType<typeof setTimeout> | undefined
  private last: Scheduled | undefined

  constructor(private readonly onDrift: (driftMs: number) => void) {}

  play(audioData: AudioData, captureMicros: number | undefined, atMs: number): void {
    const output = this.ensureOutput(audioData.sampleRate)
    const buffer = output.context.createBuffer(
      audioData.numberOfChannels,
      audioData.numberOfFrames,
      audioData.sampleRate
    )
    for (let channel = 0; channel < audioData.numberOfChannels; channel += 1) {
      audioData.copyTo(buffer.getChannelData(channel), { planeIndex: channel, format: 'f32-planar' })
    }
    const chunk = { buffer, captureMicros, atMs }
    if (isRendering(output.context)) {
      this.schedule(output, chunk)
    } else {
      this.waiting.push(chunk)
      this.awaitRendering(output)
    }
  }

  flush(): void {
    for (const source of this.sources) {
      source.stop()
    }
    this.sources.clear()
    this.waiting.length = 0
    this.writeHead = undefined
    this.last = undefined
  }

  setVolume(volume: number): void {
    this.volume = volume
    if (this.output) {
      this.output.gain.gain.value = volume
    }
  }

  async setSuspended(suspended: boolean): Promise<void> {
    if (!this.output) {
      return
    }
    if (suspended) {
      this.flush()
      await this.output.context.suspend()
    } else {
      await this.output.context.resume()
    }
  }

  positionMicrosAt(nowMs: number): number | undefined {
    if (!this.last || this.last.captureMicros === undefined || nowMs - this.last.atMs > POSITION_STALE_MS) {
      return undefined
    }
    return this.last.captureMicros + (nowMs - this.last.atMs) * MICROS_PER_MILLI
  }

  private schedule({ context, gain }: Output, chunk: Chunk): void {
    const target = this.contextTimeFor(context, chunk.atMs)
    const continues =
      this.writeHead !== undefined &&
      this.writeHead >= context.currentTime &&
      Math.abs(this.writeHead - target) * MILLIS_PER_SECOND < CONTIGUOUS_TOLERANCE_MS
    const startAt = continues ? this.writeHead! : target
    if (continues) {
      this.onDrift((startAt - target) * MILLIS_PER_SECOND)
    }
    const skipped = Math.max(0, context.currentTime - startAt)
    if (skipped >= chunk.buffer.duration) {
      this.writeHead = undefined
      return
    }
    const source = new AudioBufferSourceNode(context, { buffer: chunk.buffer })
    source.connect(gain)
    source.addEventListener('ended', () => this.sources.delete(source))
    source.start(Math.max(startAt, context.currentTime), skipped)
    this.sources.add(source)
    this.writeHead = startAt + chunk.buffer.duration
    this.last = { captureMicros: chunk.captureMicros, atMs: chunk.atMs }
  }

  /// `getOutputTimestamp` pairs the context time being heard with the
  /// performance time it is heard at, so a performance time maps onto the
  /// context timeline with the output latency already accounted for.
  private contextTimeFor(context: AudioContext, atMs: number): number {
    const { contextTime, performanceTime } = context.getOutputTimestamp()
    if (contextTime === undefined || performanceTime === undefined) {
      return context.currentTime + (atMs - performance.now()) / MILLIS_PER_SECOND - (context.outputLatency || 0)
    }
    return contextTime + (atMs - performanceTime) / MILLIS_PER_SECOND
  }

  /// The context is created on first use, after the Watch click, so that the
  /// autoplay policy lets it run. It is replaced when the stream's sample rate
  /// changes so no chunk is resampled on its own.
  private ensureOutput(sampleRate: number): Output {
    if (this.output && this.output.context.sampleRate !== sampleRate) {
      this.flush()
      void this.output.context.close()
      this.output = undefined
    }
    if (!this.output) {
      const context = new AudioContext({ sampleRate })
      const gain = new GainNode(context, { gain: this.volume })
      gain.connect(context.destination)
      if (context.state === 'suspended') {
        void context.resume()
      }
      this.output = { context, gain }
    }
    return this.output
  }

  private awaitRendering(output: Output): void {
    if (this.renderingPoll !== undefined) {
      return
    }
    this.renderingPoll = setTimeout(() => {
      this.renderingPoll = undefined
      if (this.output !== output) {
        return
      }
      if (!isRendering(output.context)) {
        this.awaitRendering(output)
        return
      }
      for (const chunk of this.waiting.splice(0)) {
        this.schedule(output, chunk)
      }
    }, RENDERING_POLL_MS)
  }
}

function isRendering(context: AudioContext): boolean {
  const { performanceTime } = context.getOutputTimestamp()
  return (
    context.state === 'running' && context.currentTime > 0 && (performanceTime === undefined || performanceTime > 0)
  )
}
