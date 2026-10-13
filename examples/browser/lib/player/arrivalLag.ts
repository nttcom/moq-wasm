import { monotonicUnixMicros } from '../../utils/media/clock'
import { type LocHeader, readLocHeader } from '../../utils/media/loc'

/// How far the objects live delivery brought in last arrived behind their
/// capture, so that a group known only by its id, as reported by
/// TRACK_STATUS, can be placed on the capture timestamp axis.
export class ArrivalLag {
  private lagMicros: number | undefined

  observe(locHeader: LocHeader | undefined): number | undefined {
    const captureMicros = readLocHeader(locHeader).captureTimestampMicros
    if (captureMicros !== undefined) {
      this.lagMicros = monotonicUnixMicros() - captureMicros
    }
    return captureMicros
  }

  liveCaptureMicros(): number | undefined {
    return this.lagMicros === undefined ? undefined : monotonicUnixMicros() - this.lagMicros
  }

  reset(): void {
    this.lagMicros = undefined
  }
}
