import { MseSink, type MseSources } from '../../../utils/media/mseSink'
import type { ReviewFrame } from '../rewind'

const REVIEW_BUFFER_AHEAD_SECONDS = 8
const REVIEW_DRAINED_SECONDS = 0.5
const APPEND_POLL_MS = 200

/// Review of CMAF windows: fetched fragments are appended to a MediaSource on
/// its own element; the live one stays open hidden and continues from the
/// newest range it is given once live delivery resumes. The next window is
/// fetched once playback has caught up to within a few seconds of what is
/// buffered. A seek within a review opens a new MediaSource, and the one it
/// replaces stays on screen until the new one has presented a frame.
export class CmafReview {
  sink: MseSink | undefined
  private opened = false

  get needsOpen(): boolean {
    return !this.opened
  }

  restart(): void {
    this.opened = false
  }

  /// Returns the sink it replaces, which the caller closes once the new one is
  /// on screen.
  async open(element: HTMLVideoElement, sources: MseSources): Promise<MseSink | undefined> {
    const previous = this.sink
    this.sink = await MseSink.open(element, sources)
    this.opened = true
    return previous
  }

  async append(frames: ReviewFrame[], audio: ReviewFrame[], isCurrent: () => boolean): Promise<boolean> {
    const sink = this.sink
    if (!isCurrent() || !sink) {
      return false
    }
    for (const frame of frames) {
      sink.appendVideo(frame.data)
    }
    for (const chunk of audio) {
      sink.appendAudio(chunk.data)
    }
    while (isCurrent()) {
      const ahead = (sink.bufferedEnd() ?? 0) - sink.element.currentTime
      if (ahead < REVIEW_BUFFER_AHEAD_SECONDS) {
        break
      }
      await new Promise((resolve) => setTimeout(resolve, APPEND_POLL_MS))
    }
    return isCurrent()
  }

  drained(): boolean {
    return (
      this.sink !== undefined && (this.sink.bufferedEnd() ?? 0) - this.sink.element.currentTime < REVIEW_DRAINED_SECONDS
    )
  }

  close(): void {
    this.sink?.close()
    this.sink = undefined
    this.opened = false
  }
}
