import { type Channels, crossfadeSamples, joinAfterSkip, trimRepetition } from './audioSplice'

const MILLIS_PER_SECOND = 1_000
const MICROS_PER_MILLI = 1_000
const POSITION_STALE_MS = 1_000
/// Within this distance a chunk continues the previous one sample for sample
/// and the difference is reported as drift for the clock to absorb. Capture
/// timestamps are millisecond-precise, `currentTime` advances a render
/// quantum at a time and `outputLatency` can be revised after start-up, so a
/// chunk placed on its own target would leave a gap or an overlap each time;
/// only a hole this wide in the source is played as a hole.
const CONTIGUOUS_TOLERANCE_MS = 100
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
  channels: Channels
  sampleRate: number
  captureMicros: number | undefined
  playoutTimeAt: (nowMs: number) => number
  joinFrom: Channels | undefined
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
/// context reports `running` before its clock has started, and chunks that
/// arrive while others are waiting queue behind them so the order holds.
/// Chunks that are skipped are not heard; the chunk played after them fades
/// in from the head of the first one, which is what the sound heard so far
/// continues into. A compression rate above 1 removes repetitions of the
/// waveform from the chunks until they are that much shorter. A chunk that
/// ends before the next one is due reports the difference as drift, which
/// pulls the clock in by what the compression removed.
export class AudioPlayout {
  private output: Output | undefined
  private volume = 1
  private compressionRate = 1
  private compressionDebtSamples = 0
  private skippedHead: Channels | undefined
  private readonly sources = new Map<AudioBufferSourceNode, number>()
  private readonly waiting: Chunk[] = []
  private writeHead: number | undefined
  private renderingPoll: ReturnType<typeof setTimeout> | undefined
  private last: Scheduled | undefined
  breaks = 0
  shedMs = 0

  constructor(private readonly onDrift: (driftMs: number, captureMicros: number | undefined) => void) {}

  /// Opens the context ahead of the first chunk so that its start-up has
  /// passed by the time the chunk is due.
  prepare(sampleRate: number): void {
    this.ensureOutput(sampleRate)
  }

  play(audioData: AudioData, captureMicros: number | undefined, playoutTimeAt: (nowMs: number) => number): void {
    const output = this.ensureOutput(audioData.sampleRate)
    const chunk = {
      channels: copyChannels(audioData, audioData.numberOfFrames),
      sampleRate: audioData.sampleRate,
      captureMicros,
      playoutTimeAt,
      joinFrom: this.skippedHead
    }
    this.skippedHead = undefined
    if (isRendering(output.context) && this.waiting.length === 0) {
      this.schedule(output, chunk)
    } else {
      this.waiting.push(chunk)
      this.awaitRendering(output)
    }
  }

  skip(audioData: AudioData): void {
    this.skippedHead ??= copyChannels(
      audioData,
      Math.min(audioData.numberOfFrames, crossfadeSamples(audioData.sampleRate))
    )
    this.shedMs += audioData.duration / MICROS_PER_MILLI
  }

  flush(): void {
    for (const source of this.sources.keys()) {
      source.stop()
    }
    this.sources.clear()
    this.waiting.length = 0
    this.writeHead = undefined
    this.last = undefined
    this.skippedHead = undefined
    this.compressionDebtSamples = 0
  }

  setCompressionRate(compressionRate: number): void {
    this.compressionRate = compressionRate
    if (compressionRate === 1) {
      this.compressionDebtSamples = 0
    }
  }

  setVolume(volume: number): void {
    this.volume = volume
    if (this.output) {
      const { context, gain } = this.output
      gain.gain.setTargetAtTime(volume, context.currentTime, VOLUME_RAMP_SECONDS)
    }
  }

  /// Suspending freezes the context clock, so what is scheduled resumes in
  /// place; the caller flushes first when it should not.
  async suspend(): Promise<void> {
    await this.output?.context.suspend()
  }

  async resume(): Promise<void> {
    await this.output?.context.resume()
  }

  close(): void {
    this.flush()
    if (this.renderingPoll !== undefined) {
      clearTimeout(this.renderingPoll)
      this.renderingPoll = undefined
    }
    if (this.output) {
      void this.output.context.close()
      this.output = undefined
    }
  }

  /// How long before it is heard a chunk has to be scheduled.
  outputLatencyMs(): number {
    return (this.output?.context.outputLatency || 0) * MILLIS_PER_SECOND
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
    if (!continues && this.writeHead !== undefined) {
      this.breaks += 1
    }
    this.onDrift((startAt - target) * MILLIS_PER_SECOND, chunk.captureMicros)
    const channels = this.splice(chunk, continues)
    const buffer = context.createBuffer(channels.length, channels[0].length, chunk.sampleRate)
    channels.forEach((channel, index) => buffer.copyToChannel(channel, index))
    const source = new AudioBufferSourceNode(context, { buffer })
    source.connect(gain)
    source.addEventListener('ended', () => this.sources.delete(source))
    source.start(startAt)
    this.sources.set(source, startAt)
    this.writeHead = startAt + buffer.duration
    this.last = { captureMicros: chunk.captureMicros, atMs }
  }

  private splice(chunk: Chunk, continues: boolean): Channels {
    const originalSamples = chunk.channels[0].length
    let channels =
      continues && chunk.joinFrom ? joinAfterSkip(chunk.joinFrom, chunk.channels, chunk.sampleRate) : chunk.channels
    if (this.compressionRate > 1) {
      this.compressionDebtSamples += channels[0].length * (1 - 1 / this.compressionRate)
      const trimmed = trimRepetition(channels, chunk.sampleRate, Math.floor(this.compressionDebtSamples))
      if (trimmed) {
        this.compressionDebtSamples -= channels[0].length - trimmed[0].length
        channels = trimmed
      }
    }
    this.shedMs += ((originalSamples - channels[0].length) * MILLIS_PER_SECOND) / chunk.sampleRate
    return channels
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

function copyChannels(audioData: AudioData, frameCount: number): Channels {
  return Array.from({ length: audioData.numberOfChannels }, (_, planeIndex) => {
    const channel = new Float32Array(frameCount)
    audioData.copyTo(channel, { planeIndex, format: 'f32-planar', frameCount })
    return channel
  })
}

function isRendering(context: AudioContext): boolean {
  return context.state === 'running' && context.currentTime > 0
}
