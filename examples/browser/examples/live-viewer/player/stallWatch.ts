const STALL_MS = 500

/// Reports the picture as stalled once it has stood still for `STALL_MS`:
/// each presented frame clears it and restarts the wait, whether the data is
/// late or the next window is still being fetched.
export class StallWatch {
  private timer: ReturnType<typeof setTimeout> | undefined
  stalled = false

  constructor(private readonly onChange: () => void) {}

  framePresented(): void {
    this.clear()
    this.timer = setTimeout(() => {
      this.timer = undefined
      this.setStalled(true)
    }, STALL_MS)
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
