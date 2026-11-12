import type { MediaCatalogTrack } from '../catalog'
import { readLocHeader } from '../internal/loc'
import type { AudioPlayout } from './audioPlayout'
import type { LivePlayout } from './livePlayout'
import { type LocObject, audioCatalogMessage, objectMessage } from './locLive'

/// A second audio track, such as a translation of the main one, played on a
/// live playout's clock. It starts silent; `fadeTo` fades it in. A chunk that
/// is due before it can be heard is dropped and, unless it came from the
/// relay's cache of the group the subscription joined, counted in
/// `droppedLate`.
export class AudioLane {
  private readonly worker = new Worker(new URL('../internal/decoders/audioDecoder.ts', import.meta.url), {
    type: 'module'
  })
  private readonly audio: AudioPlayout
  private newestCaptureMicros: number | undefined
  private cachedUpToMicros: number | undefined
  private firstScheduledReported = false
  droppedLate = 0

  constructor(
    track: MediaCatalogTrack,
    private readonly playout: LivePlayout,
    private readonly onFirstScheduled: (dueMs: number) => void
  ) {
    this.audio = playout.openAudioInput()
    this.audio.setVolume(0)
    this.worker.postMessage(audioCatalogMessage(track))
    this.worker.onmessage = (event) => {
      if (event.data.type === 'audioData') {
        this.schedule(event.data.audioData as AudioData, event.data.captureTimestampMicros as number | undefined)
      }
    }
  }

  /// `cached` marks an object a joining FETCH delivered from before the
  /// subscription started.
  push(groupId: bigint, object: LocObject, cached = false): void {
    if (cached) {
      const captureMicros = readLocHeader(object.locHeader).timestampMicros
      if (captureMicros !== undefined) {
        this.cachedUpToMicros = Math.max(this.cachedUpToMicros ?? 0, captureMicros)
      }
    }
    const [message, transfer] = objectMessage(groupId, object)
    this.worker.postMessage(message, transfer)
  }

  fadeTo(volume: number, rampSeconds?: number): void {
    this.audio.setVolume(volume, rampSeconds)
  }

  positionMicros(): number | undefined {
    return this.audio.positionMicrosAt(performance.now())
  }

  close(): void {
    this.worker.terminate()
    this.audio.close()
  }

  private schedule(audioData: AudioData, captureMicros: number | undefined): void {
    const dueMs = captureMicros === undefined ? undefined : this.playout.dueAt(captureMicros)
    if (
      captureMicros === undefined ||
      dueMs === undefined ||
      (this.newestCaptureMicros !== undefined && captureMicros <= this.newestCaptureMicros)
    ) {
      audioData.close()
      return
    }
    this.newestCaptureMicros = captureMicros
    if (dueMs < performance.now() + this.audio.outputLatencyMs()) {
      audioData.close()
      if (this.cachedUpToMicros === undefined || captureMicros > this.cachedUpToMicros) {
        this.droppedLate += 1
      }
      return
    }
    this.audio.play(audioData, captureMicros, () => dueMs)
    audioData.close()
    if (!this.firstScheduledReported) {
      this.firstScheduledReported = true
      this.onFirstScheduled(dueMs)
    }
  }
}
