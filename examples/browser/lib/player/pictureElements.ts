/// The elements a live picture is shown in. They start hidden so a picture
/// that has yet to present a frame never shows as an empty element.
export function createPictureVideo({ testId, muted }: { testId?: string; muted: boolean }): HTMLVideoElement {
  const video = document.createElement('video')
  video.className = 'live-player-picture'
  video.playsInline = true
  video.autoplay = true
  video.hidden = true
  if (muted) {
    video.muted = true
    video.setAttribute('muted', '')
  }
  if (testId) {
    video.dataset.testid = testId
  }
  return video
}

export function createPictureCanvas(testId: string): HTMLCanvasElement {
  const canvas = document.createElement('canvas')
  canvas.className = 'live-player-picture'
  canvas.hidden = true
  canvas.dataset.testid = testId
  return canvas
}
