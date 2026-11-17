const SAMPLE_INTERVAL_MS = 250
const FPS_WINDOW_MS = 1_000

export type FrameSample = { atMs: number; frames: number }

/// The frames presented per second over `samples`, which end with the newest; `undefined` until
/// two samples lie within the window.
export function framesPerSecond(samples: FrameSample[]): number | undefined {
  const oldest = samples[0]
  const newest = samples[samples.length - 1]
  if (!oldest || !newest || newest.atMs <= oldest.atMs) {
    return undefined
  }
  return Math.round(((newest.frames - oldest.frames) * 1_000) / (newest.atMs - oldest.atMs))
}

type Count = { frames: number; samples: FrameSample[] }

/// Puts a badge on every camera: the frames its picture presented in the last second.
export class PlaybackBadges {
  private readonly badges = new Map<HTMLElement, HTMLDivElement>()
  private readonly counts = new WeakMap<HTMLVideoElement, Count>()
  private timer: ReturnType<typeof setInterval> | undefined

  add(host: HTMLElement): void {
    const badge = document.createElement('div')
    badge.className = 'cam-playback-badge'
    this.badges.set(host, badge)
    this.timer ??= setInterval(() => this.update(), SAMPLE_INTERVAL_MS)
  }

  private update(): void {
    const now = performance.now()
    for (const [host, badge] of this.badges) {
      // A host holds the live, review and CMAF pictures; the player hides all but the one on
      // screen. A picture drawn on a canvas has no frame count, so it gets no badge.
      const video = host.querySelector<HTMLVideoElement>('video:not([hidden])')
      const fps = video ? this.sample(video, now) : undefined
      if (!video || fps === undefined) {
        badge.remove()
        continue
      }
      badge.textContent = `${fps} fps`
      badge.classList.toggle('cam-playback-badge-stalled', fps === 0 && !video.paused)
      // `CameraPlayers` replaces the host's children when a camera's source changes.
      if (badge.parentElement !== host) {
        host.appendChild(badge)
      }
    }
  }

  private sample(video: HTMLVideoElement, now: number): number | undefined {
    let count = this.counts.get(video)
    if (!count) {
      const started: Count = { frames: 0, samples: [] }
      count = started
      this.counts.set(video, started)
      const onFrame = () => {
        started.frames += 1
        video.requestVideoFrameCallback(onFrame)
      }
      video.requestVideoFrameCallback(onFrame)
    }
    count.samples = count.samples.filter((sample) => now - sample.atMs <= FPS_WINDOW_MS)
    count.samples.push({ atMs: now, frames: count.frames })
    return framesPerSecond(count.samples)
  }
}
