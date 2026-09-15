const MILLIS_PER_SECOND = 1_000
const MICROS_PER_MILLI = 1_000
const POSITION_STALE_MS = 1_000
/// Within this distance a chunk continues the previous one sample for sample.
/// Capture timestamps are millisecond-precise and `currentTime` advances a
/// render quantum at a time, so each target wobbles by a few milliseconds; a
/// chunk placed on its own target would leave a gap or an overlap each time.
const CONTIGUOUS_TOLERANCE_MS = 20
const RENDERING_POLL_MS = 10
const VOLUME_RAMP_SECONDS = 0.02

type Output = {
  context: AudioContext
  gain: GainNode
}

/// The playout time is asked for when the chunk is scheduled, not when it
/// arrives: a chunk that waited for the context must be placed against the
/// clock as it stands after the chunks before it have moved it.
type Chunk = {
  buffer: AudioBuffer
  captureMicros: number | undefined
  playoutTimeAt: (nowMs: number) => number
}

type Scheduled = {
  captureMicros: number | undefined
  atMs: number
}

/// Plays decoded audio through an AudioContext running at the stream's sample
/// rate. Chunks are appended whole at a write head so the waveform stays
/// continuous, and how far the write head sits from the time the clock asked
/// for is reported as drift, which lets the clock follow the audio device: a
/// chunk that is late is not trimmed, it starts now and moves the clock.
/// Chunks that arrive before the context renders wait for it, because a
/// context reports `running` before its clock has started.
export class AudioPlayout {
  private output: Output | undefined
  private volume = 1
  private readonly sources = new Map<AudioBufferSourceNode, number>()
  private readonly waiting: Chunk[] = []
  private writeHead: number | undefined
  private renderingPoll: ReturnType<typeof setTimeout> | undefined
  private last: Scheduled | undefined

  constructor(private readonly onDrift: (driftMs: number) => void) {}

  /// Opens the context ahead of the first chunk so that its start-up has
  /// passed by the time the chunk is due.
  prepare(sampleRate: number): void {
    this.ensureOutput(sampleRate)
  }

  play(audioData: AudioData, captureMicros: number | undefined, playoutTimeAt: (nowMs: number) => number): void {
    const output = this.ensureOutput(audioData.sampleRate)
    const buffer = output.context.createBuffer(
      audioData.numberOfChannels,
      audioData.numberOfFrames,
      audioData.sampleRate
    )
    for (let channel = 0; channel < audioData.numberOfChannels; channel += 1) {
      audioData.copyTo(buffer.getChannelData(channel), { planeIndex: channel, format: 'f32-planar' })
    }
    const chunk = { buffer, captureMicros, playoutTimeAt }
    if (isRendering(output.context)) {
      this.schedule(output, chunk)
    } else {
      this.waiting.push(chunk)
      this.awaitRendering(output)
    }
  }

  /// Drops what has not started yet; the chunk being heard plays out so the
  /// cut falls on a chunk boundary.
  dropScheduled(): void {
    const now = this.output?.context.currentTime ?? 0
    for (const [source, startAt] of this.sources) {
      if (startAt > now) {
        source.stop()
        this.sources.delete(source)
      }
    }
    this.waiting.length = 0
    this.writeHead = undefined
    this.last = undefined
  }

  flush(): void {
    for (const source of this.sources.keys()) {
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
      const { context, gain } = this.output
      gain.gain.setTargetAtTime(volume, context.currentTime, VOLUME_RAMP_SECONDS)
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
    const nowMs = performance.now()
    const atMs = chunk.playoutTimeAt(nowMs)
    const target = contextTimeFor(context, atMs, nowMs)
    const continues =
      this.writeHead !== undefined &&
      this.writeHead >= context.currentTime &&
      Math.abs(this.writeHead - target) * MILLIS_PER_SECOND < CONTIGUOUS_TOLERANCE_MS
    const startAt = continues ? this.writeHead! : Math.max(target, context.currentTime)
    this.onDrift((startAt - target) * MILLIS_PER_SECOND)
    const source = new AudioBufferSourceNode(context, { buffer: chunk.buffer })
    source.connect(gain)
    source.addEventListener('ended', () => this.sources.delete(source))
    source.start(startAt)
    this.sources.set(source, startAt)
    this.writeHead = startAt + chunk.buffer.duration
    this.last = { captureMicros: chunk.captureMicros, atMs }
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

/// A sound started at `currentTime + x` is heard `outputLatency` later, so
/// the latency comes off the start time for the sound to be heard at `atMs`.
function contextTimeFor(context: AudioContext, atMs: number, nowMs: number): number {
  return context.currentTime + (atMs - nowMs) / MILLIS_PER_SECOND - (context.outputLatency || 0)
}

function isRendering(context: AudioContext): boolean {
  return context.state === 'running' && context.currentTime > 0
}
