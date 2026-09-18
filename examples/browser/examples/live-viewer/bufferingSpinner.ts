const STALL_MS = 500

/// Shows a spinner over the picture when the frames stop: each presented
/// frame hides it and restarts the wait, so it appears once the picture has
/// stood still for `STALL_MS`, whether the data is late or the next window is
/// still being fetched.
export class BufferingSpinner {
  private timer: ReturnType<typeof setTimeout> | undefined

  constructor(private readonly spinner: HTMLElement) {}

  framePresented(): void {
    this.hide()
    this.timer = setTimeout(() => {
      this.timer = undefined
      this.spinner.hidden = false
    }, STALL_MS)
  }

  hide(): void {
    if (this.timer !== undefined) {
      clearTimeout(this.timer)
      this.timer = undefined
    }
    if (!this.spinner.hidden) {
      this.spinner.hidden = true
    }
  }
}
