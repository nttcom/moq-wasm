import type { MseSink } from '../../../utils/media/mseSink'

/// One picture is on screen at a time. A picture that has yet to present a
/// frame stays hidden and whatever is on screen stays until it does, so a
/// change of packaging, quality or position never shows an empty element.
export class PictureStage {
  private visible: HTMLElement

  constructor(
    initial: HTMLElement,
    private readonly msePool: HTMLVideoElement[]
  ) {
    this.visible = initial
  }

  show(next: HTMLElement): void {
    if (next === this.visible) {
      return
    }
    this.visible.hidden = true
    next.hidden = false
    this.visible = next
  }

  /// The sink being replaced is closed as soon as it is off screen; while it is
  /// on screen it plays on until the replacement has presented a frame.
  replace(next: HTMLElement, stillWanted: () => boolean, previous: MseSink | undefined): void {
    if (previous && previous.element !== this.visible) {
      previous.close()
    }
    const swap = () => {
      previous?.close()
      if (stillWanted()) {
        this.show(next)
      }
    }
    if (next instanceof HTMLVideoElement) {
      next.requestVideoFrameCallback(swap)
    } else {
      requestAnimationFrame(swap)
    }
  }

  freeMseElement(): HTMLVideoElement {
    const free = this.msePool.find((video) => !video.getAttribute('src'))
    if (!free) {
      throw new Error('every MediaSource element is in use')
    }
    return free
  }
}
