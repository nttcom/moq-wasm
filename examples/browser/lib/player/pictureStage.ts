import type { MseSink } from '../../utils/media/mseSink'
import { createPictureCanvas, createPictureVideo } from './pictureElements'
import './livePlayer.css'

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

  constructor(container: HTMLElement) {
    this.liveVideo.hidden = false
    container.prepend(this.liveVideo, this.liveCanvas, this.reviewCanvas, ...this.msePool)
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
