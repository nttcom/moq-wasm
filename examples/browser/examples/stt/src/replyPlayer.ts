const OPUS_SAMPLE_RATE = 48_000
const OPUS_FRAME_MICROS = 20_000

/** Plays the reply track: each object is one Opus packet, decoded with
 * WebCodecs and scheduled back to back. Packets are grouped by turn (the MoQT
 * group id); once `expect()` has been told how many packets a turn has and all
 * of them are scheduled, `onPlaybackEnd` fires when the last sample is out. */
export class ReplyPlayer {
  private readonly audioContext = new AudioContext({ sampleRate: OPUS_SAMPLE_RATE })
  private readonly decoder: AudioDecoder
  private readonly queuedTurns: number[] = []
  private readonly expected = new Map<number, number>()
  private readonly scheduled = new Map<number, number>()
  private readonly endsAt = new Map<number, number>()
  private timestamp = 0
  private nextStartTime = 0
  private decodingTurn = 0

  constructor(private readonly onPlaybackEnd: (turn: number, at: number) => void) {
    this.decoder = new AudioDecoder({
      output: (audioData) => this.play(audioData),
      error: (error) => console.error('[voice] reply decode failed', error)
    })
    this.decoder.configure({ codec: 'opus', sampleRate: OPUS_SAMPLE_RATE, numberOfChannels: 1 })
  }

  feed(turn: number, packet: Uint8Array): void {
    this.queuedTurns.push(turn)
    this.decoder.decode(new EncodedAudioChunk({ type: 'key', timestamp: this.timestamp, data: packet }))
    this.timestamp += OPUS_FRAME_MICROS
  }

  expect(turn: number, packets: number): void {
    this.expected.set(turn, packets)
    this.reportWhenComplete(turn)
  }

  private play(audioData: AudioData): void {
    const turn = this.queuedTurns.shift() ?? this.decodingTurn
    this.decodingTurn = turn
    const buffer = this.audioContext.createBuffer(1, audioData.numberOfFrames, audioData.sampleRate)
    const channel = new Float32Array(audioData.numberOfFrames)
    audioData.copyTo(channel, { planeIndex: 0, format: 'f32-planar' })
    buffer.copyToChannel(channel, 0)
    audioData.close()
    const source = this.audioContext.createBufferSource()
    source.buffer = buffer
    source.connect(this.audioContext.destination)
    const startTime = Math.max(this.audioContext.currentTime, this.nextStartTime)
    source.start(startTime)
    this.nextStartTime = startTime + buffer.duration
    this.scheduled.set(turn, (this.scheduled.get(turn) ?? 0) + 1)
    this.endsAt.set(turn, performance.now() + (this.nextStartTime - this.audioContext.currentTime) * 1000)
    this.reportWhenComplete(turn)
  }

  private reportWhenComplete(turn: number): void {
    const endsAt = this.endsAt.get(turn)
    if (endsAt === undefined || this.expected.get(turn) !== this.scheduled.get(turn)) {
      return
    }
    this.expected.delete(turn)
    setTimeout(() => this.onPlaybackEnd(turn, performance.now()), Math.max(0, endsAt - performance.now()))
  }

  async close(): Promise<void> {
    if (this.decoder.state !== 'closed') {
      this.decoder.close()
    }
    await this.audioContext.close()
  }
}
