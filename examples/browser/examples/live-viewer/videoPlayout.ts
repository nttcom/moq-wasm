const MICROS_PER_MILLI = 1_000
const POSITION_STALE_MS = 1_000
const LATE_MS = 33

type PendingFrame = {
  frame: VideoFrame
  atMs: number
}

type Presented = {
  captureMicros: number
  atMs: number
}

/// Holds decoded frames until the clock says they are due and then hands them
/// to the sink that shows them, which also closes them. When several frames
/// fall due together only the newest is shown. While paused nothing is shown
/// and the frames keep waiting; `shift` moves their due times by the pause.
export class VideoPlayout {
  private pending: PendingFrame[] = []
  private timer: ReturnType<typeof setTimeout> | undefined
  private last: Presented | undefined
  private paused = false
  /// Frames that fell due together with a newer one and were never shown.
  dropped = 0
  /// Frames shown more than a frame period after they were due.
  late = 0

  constructor(private readonly show: (frame: VideoFrame) => void) {}

  present(frame: VideoFrame, atMs: number): void {
    const index = this.pending.findIndex((pending) => pending.atMs > atMs)
    this.pending.splice(index === -1 ? this.pending.length : index, 0, { frame, atMs })
    this.schedule()
  }

  get queued(): number {
    return this.pending.length
  }

  flush(): void {
    for (const { frame } of this.pending) {
      frame.close()
    }
    this.pending = []
    this.clearTimer()
    this.last = undefined
  }

  setPaused(paused: boolean): void {
    this.paused = paused
    this.schedule()
  }

  shift(deltaMs: number): void {
    for (const pending of this.pending) {
      pending.atMs += deltaMs
    }
    if (this.last) {
      this.last = { ...this.last, atMs: this.last.atMs + deltaMs }
    }
    this.schedule()
  }

  positionMicrosAt(nowMs: number): number | undefined {
    if (!this.last || nowMs - this.last.atMs > POSITION_STALE_MS) {
      return undefined
    }
    return this.last.captureMicros + (nowMs - this.last.atMs) * MICROS_PER_MILLI
  }

  private schedule(): void {
    this.clearTimer()
    const head = this.pending[0]
    if (!head || this.paused) {
      return
    }
    this.timer = setTimeout(
      () => {
        this.timer = undefined
        this.presentDue()
      },
      Math.max(0, head.atMs - performance.now())
    )
  }

  private clearTimer(): void {
    if (this.timer !== undefined) {
      clearTimeout(this.timer)
      this.timer = undefined
    }
  }

  private presentDue(): void {
    const nowMs = performance.now()
    const firstNotDue = this.pending.findIndex((pending) => pending.atMs > nowMs)
    const due = this.pending.splice(0, firstNotDue === -1 ? this.pending.length : firstNotDue)
    const shown = due.pop()
    for (const { frame } of due) {
      frame.close()
    }
    this.dropped += due.length
    if (shown) {
      if (nowMs - shown.atMs > LATE_MS) {
        this.late += 1
      }
      this.last = shown.frame.timestamp ? { captureMicros: shown.frame.timestamp, atMs: nowMs } : undefined
      this.show(shown.frame)
    }
    this.schedule()
  }
}
