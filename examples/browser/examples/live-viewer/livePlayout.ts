import { AudioPlayout } from './audioPlayout'
import { PlayoutClock } from './playoutClock'
import { VideoPlayout } from './videoPlayout'

const PLAYOUT_DELAY_MS = 200
const MAX_EARLY_MS = 200
const MICROS_PER_MILLI = 1_000

/// Live LOC playback. Both decoders hand over samples as soon as they are
/// decoded and one clock decides when each is presented, which is what keeps
/// the picture and the sound together. The audio device is the master: the
/// clock follows the drift the audio playout reports, so the picture stays
/// with the sound as the two clocks part. A sample without a capture
/// timestamp is presented at once. Pausing drops what arrives, and the clock
/// is re-anchored on resume so playback comes back at the live edge.
export class LivePlayout {
  private readonly clock = new PlayoutClock(PLAYOUT_DELAY_MS, MAX_EARLY_MS)
  private readonly audio = new AudioPlayout((driftMs) => this.clock.shift(driftMs))
  private readonly video: VideoPlayout
  private paused = false

  constructor(videoWriter: WritableStreamDefaultWriter<VideoFrame>, onVideoPresented: (frame: VideoFrame) => void) {
    this.video = new VideoPlayout(videoWriter, onVideoPresented)
  }

  presentVideo(frame: VideoFrame): void {
    if (this.paused) {
      frame.close()
      return
    }
    const nowMs = performance.now()
    this.video.present(frame, this.playoutTime(frame.timestamp, nowMs) ?? nowMs)
  }

  playAudio(audioData: AudioData, captureMicros: number | undefined): void {
    if (this.paused) {
      audioData.close()
      return
    }
    const nowMs = performance.now()
    this.audio.play(audioData, captureMicros, this.playoutTime(captureMicros, nowMs) ?? nowMs)
    audioData.close()
  }

  setVolume(volume: number): void {
    this.audio.setVolume(volume)
  }

  setPaused(paused: boolean): void {
    if (this.paused === paused) {
      return
    }
    this.paused = paused
    if (paused) {
      this.video.flush()
    } else {
      this.clock.reset()
    }
    void this.audio.setSuspended(paused)
  }

  reset(): void {
    this.video.flush()
    this.audio.flush()
    this.clock.reset()
  }

  /// How far the picture on screen is ahead of the sound, from the capture
  /// timestamps each was presented at.
  syncOffsetMs(): number | undefined {
    const nowMs = performance.now()
    const video = this.video.positionMicrosAt(nowMs)
    const audio = this.audio.positionMicrosAt(nowMs)
    if (video === undefined || audio === undefined) {
      return undefined
    }
    return (video - audio) / MICROS_PER_MILLI
  }

  private playoutTime(captureMicros: number | undefined, nowMs: number): number | undefined {
    if (!captureMicros) {
      return undefined
    }
    const { atMs, reanchored } = this.clock.playoutTime(captureMicros, nowMs)
    if (reanchored) {
      this.video.flush()
      this.audio.flush()
    }
    return atMs
  }
}
