import type { MseSink } from '../../utils/media/mseSink'
import './livePlayer.css'

const MSE_POOL_SIZE = 3

/// One picture is on screen at a time. A picture that has yet to present a
/// frame stays hidden and whatever is on screen stays until it does, so a
/// change of packaging, quality or position never shows an empty element.
export class PictureStage {
  readonly liveVideo = pictureVideo('live-player-video')
  readonly liveCanvas = pictureCanvas('live-player-live-canvas')
  readonly reviewCanvas = pictureCanvas('live-player-review-canvas')
  private readonly msePool = Array.from({ length: MSE_POOL_SIZE }, () => pictureVideo())
  private visible: HTMLElement = this.liveVideo

  constructor(container: HTMLElement) {
    this.liveVideo.hidden = false
    this.liveVideo.muted = true
    this.liveVideo.setAttribute('muted', '')
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

function pictureVideo(testId?: string): HTMLVideoElement {
  const video = document.createElement('video')
  video.className = 'live-player-picture'
  video.playsInline = true
  video.autoplay = true
  video.hidden = true
  if (testId) {
    video.dataset.testid = testId
  }
  return video
}

function pictureCanvas(testId: string): HTMLCanvasElement {
  const canvas = document.createElement('canvas')
  canvas.className = 'live-player-picture'
  canvas.hidden = true
  canvas.dataset.testid = testId
  return canvas
}
