import type { MseSink } from '../internal/mseSink'
import { createPictureCanvas, createPictureVideo } from './pictureElements'

const MSE_POOL_SIZE = 3

/// One picture is on screen at a time. A picture that has yet to present a
/// frame stays hidden and whatever is on screen stays until it does, so a
/// change of packaging, quality or position never shows an empty element.
export class PictureStage {
  readonly liveVideo = createPictureVideo({ testId: 'live-player-video', muted: true })
  readonly liveCanvas = createPictureCanvas('live-player-live-canvas')
  readonly reviewCanvas = createPictureCanvas('live-player-review-canvas')
  private readonly msePool = Array.from({ length: MSE_POOL_SIZE }, () => createPictureVideo({ muted: false }))
  private visible: HTMLElement = this.liveVideo

  constructor(
    container: HTMLElement,
    private readonly wantsPlaying: () => boolean
  ) {
    this.liveVideo.hidden = false
    this.liveVideo.addEventListener('pause', this.resumeLiveVideo)
    container.prepend(this.liveVideo, this.liveCanvas, this.reviewCanvas, ...this.msePool)
  }

  /// Takes the pictures out of the container.
  remove(): void {
    this.liveVideo.removeEventListener('pause', this.resumeLiveVideo)
    for (const element of [this.liveVideo, this.liveCanvas, this.reviewCanvas, ...this.msePool]) {
      element.remove()
    }
  }

  /// The browser pauses a media element the page detaches and reattaches in a
  /// later task, and nothing restarts it: autoplay applies only on load, and a
  /// rendition splice keeps the element and its stream. The video is muted, so
  /// autoplay policy allows the play.
  private readonly resumeLiveVideo = () => {
    if (this.visible === this.liveVideo && this.wantsPlaying()) {
      void this.liveVideo.play().catch(() => undefined)
    }
  }

  videos(): HTMLVideoElement[] {
    return [this.liveVideo, ...this.msePool]
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
