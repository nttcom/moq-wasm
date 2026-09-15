const MICROS_PER_MILLI = 1_000
const POSITION_STALE_MS = 1_000

type PendingFrame = {
  frame: VideoFrame
  atMs: number
}

type Presented = {
  captureMicros: number
  atMs: number
}

/// Holds decoded frames until the clock says they are due and then writes them
/// to the MediaStream the video element shows. When several frames fall due
/// together only the newest is shown.
export class VideoPlayout {
  private pending: PendingFrame[] = []
  private timer: ReturnType<typeof setTimeout> | undefined
  private last: Presented | undefined

  constructor(
    private readonly writer: WritableStreamDefaultWriter<VideoFrame>,
    private readonly onPresented: (frame: VideoFrame) => void
  ) {}

  present(frame: VideoFrame, atMs: number): void {
    const index = this.pending.findIndex((pending) => pending.atMs > atMs)
    this.pending.splice(index === -1 ? this.pending.length : index, 0, { frame, atMs })
    this.schedule()
  }

  flush(): void {
    for (const { frame } of this.pending) {
      frame.close()
    }
    this.pending = []
    this.clearTimer()
    this.last = undefined
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
    if (!head) {
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
    if (shown) {
      this.last = shown.frame.timestamp ? { captureMicros: shown.frame.timestamp, atMs: nowMs } : undefined
      this.write(shown.frame)
    }
    this.schedule()
  }

  private write(frame: VideoFrame): void {
    this.onPresented(frame)
    if (this.writer.desiredSize === null || this.writer.desiredSize <= 0) {
      frame.close()
      return
    }
    void this.writer
      .write(frame)
      .catch(() => undefined)
      .finally(() => frame.close())
  }
}
