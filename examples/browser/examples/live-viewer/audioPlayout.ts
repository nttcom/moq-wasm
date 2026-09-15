const MILLIS_PER_SECOND = 1_000
const MICROS_PER_MILLI = 1_000
const POSITION_STALE_MS = 1_000

type Output = {
  context: AudioContext
  gain: GainNode
}

type Scheduled = {
  captureMicros: number | undefined
  atMs: number
}

/// Plays decoded audio through an AudioContext, which starts each chunk at a
/// sample-accurate time. The output latency is taken off that time so the
/// sound leaves the device when the clock says rather than that much later.
export class AudioPlayout {
  private output: Output | undefined
  private volume = 1
  private readonly sources = new Set<AudioBufferSourceNode>()
  private last: Scheduled | undefined

  play(audioData: AudioData, captureMicros: number | undefined, atMs: number, nowMs: number): void {
    const { context, gain } = this.ensureOutput()
    const buffer = context.createBuffer(audioData.numberOfChannels, audioData.numberOfFrames, audioData.sampleRate)
    for (let channel = 0; channel < audioData.numberOfChannels; channel += 1) {
      audioData.copyTo(buffer.getChannelData(channel), { planeIndex: channel, format: 'f32-planar' })
    }
    const startAt = context.currentTime + (atMs - nowMs) / MILLIS_PER_SECOND - (context.outputLatency || 0)
    const skipped = Math.max(0, context.currentTime - startAt)
    if (skipped >= buffer.duration) {
      return
    }
    const source = new AudioBufferSourceNode(context, { buffer })
    source.connect(gain)
    source.addEventListener('ended', () => this.sources.delete(source))
    source.start(Math.max(startAt, context.currentTime), skipped)
    this.sources.add(source)
    this.last = { captureMicros, atMs }
  }

  flush(): void {
    for (const source of this.sources) {
      source.stop()
    }
    this.sources.clear()
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

  /// The context is created on first use, after the Watch click, so that the
  /// autoplay policy lets it run.
  private ensureOutput(): Output {
    if (!this.output) {
      const context = new AudioContext()
      const gain = new GainNode(context, { gain: this.volume })
      gain.connect(context.destination)
      this.output = { context, gain }
    }
    if (this.output.context.state === 'suspended') {
      void this.output.context.resume()
    }
    return this.output
  }
}
