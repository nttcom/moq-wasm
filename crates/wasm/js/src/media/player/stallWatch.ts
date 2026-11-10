const STALL_MS = 500
/// So one late frame of a slow track does not count as a stall.
const STALL_FRAME_INTERVALS = 2

/// How long a picture may stand still before it counts as stalled: 500 ms, or
/// two frame intervals of a track the catalog lists below 4 fps.
export function stallTimeoutMs(framerate: number | undefined): number {
  return framerate && framerate > 0 ? Math.max(STALL_MS, (STALL_FRAME_INTERVALS * 1000) / framerate) : STALL_MS
}

export class StallWatch {
  private timer: ReturnType<typeof setTimeout> | undefined
  stalled = false

  constructor(private readonly onChange: () => void) {}

  framePresented(framerate: number | undefined): void {
    this.clear()
    this.timer = setTimeout(() => {
      this.timer = undefined
      this.setStalled(true)
    }, stallTimeoutMs(framerate))
  }

  clear(): void {
    if (this.timer !== undefined) {
      clearTimeout(this.timer)
      this.timer = undefined
    }
    this.setStalled(false)
  }

  private setStalled(stalled: boolean): void {
    if (this.stalled !== stalled) {
      this.stalled = stalled
      this.onChange()
    }
  }
}
