import { getErrorMessage } from '../../../examples/media/common'
import type { MediaCatalogTrack } from '../../../examples/media/catalog'
import type { BufferPolicy } from '../jitterBuffer'
import type { LivePlayer, LivePlayerState, LivePlayerStats, Packaging } from '../livePlayer'
import { type CatchUp, DEFAULT_BUFFER_POLICY } from '../livePlayout'
import { formatElapsed } from '../mediaTimeline'
import type { LogLevel } from '../trackContext'
import { CONTROLS_MARKUP } from './controlsMarkup'
import './playerControls.css'

const MICROS_PER_SECOND = 1_000_000
const SKIP_SECONDS_BY_KEY: Record<string, number> = { ArrowLeft: -1, ArrowRight: 1, ArrowDown: -5, ArrowUp: 5 }
const POINTER_IDLE_MS = 2_500

/// The controls laid over the picture: seek bar, skip buttons, play/pause,
/// LIVE, speed, volume, the quality menu and fullscreen. They render
/// `LivePlayer.state` and hold only the state of the gesture in progress. The
/// arrow keys skip wherever the focus is, so a page holds one of them.
export class PlayerControls {
  readonly seekbar: HTMLInputElement
  private readonly root: HTMLElement
  private seeking = false
  private pointerIdleTimer: ReturnType<typeof setTimeout> | undefined

  constructor(
    private readonly container: HTMLElement,
    private readonly player: LivePlayer,
    private readonly onLog: (level: LogLevel, message: string) => void
  ) {
    container.classList.add('viewer-stage')
    const template = document.createElement('template')
    template.innerHTML = CONTROLS_MARKUP
    this.root = container
    container.append(template.content)
    this.seekbar = this.part<HTMLInputElement>('seekbar')
    this.part<HTMLButtonElement>('quality-button').popoverTargetElement = this.part('quality-menu')
    this.bind()
  }

  render(): void {
    const state = this.player.state
    fillSelect(this.part('video-track'), state.videoTracks, state.selectedVideoTrack, describeVideoTrack)
    fillSelect(this.part('audio-track'), state.audioTracks, state.selectedAudioTrack, (track) => track.label)
    this.renderPackaging(state)
    this.renderPlayPause(state.paused)
    this.part('buffering').hidden = !state.stalled
    this.renderSeekbar(state)
  }

  renderBuffer(stats: LivePlayerStats): void {
    this.part('buffer-current').textContent =
      stats.bufferMs === undefined
        ? 'Current: -'
        : `Current: ${Math.round(stats.bufferMs)} ms (${stats.fixedBuffer ? 'fixed' : `target ${Math.round(stats.targetBufferMs)}`})`
  }

  private part<T extends HTMLElement>(name: string): T {
    const found = this.root.querySelector<T>(`[data-part="${name}"]`)
    if (!found) {
      throw new Error(`missing control: ${name}`)
    }
    return found
  }

  private bind(): void {
    const { player, seekbar } = this
    this.part<HTMLSelectElement>('video-track').addEventListener(
      'change',
      (event) => void player.selectVideoTrack((event.target as HTMLSelectElement).value)
    )
    this.part<HTMLSelectElement>('audio-track').addEventListener(
      'change',
      (event) => void player.selectAudioTrack((event.target as HTMLSelectElement).value)
    )
    this.part<HTMLSelectElement>('packaging').addEventListener(
      'change',
      (event) => void player.setPackaging((event.target as HTMLSelectElement).value as Packaging)
    )
    this.part<HTMLSelectElement>('speed').addEventListener('change', (event) =>
      player.setPlaybackRate(Number((event.target as HTMLSelectElement).value))
    )
    this.part('live').addEventListener('click', () => this.goLive())
    this.part('play-pause').addEventListener('click', () => player.setPaused(!player.state.paused))
    this.part<HTMLInputElement>('volume').addEventListener('input', (event) =>
      player.setVolume((event.target as HTMLInputElement).valueAsNumber)
    )
    this.part('fullscreen').addEventListener('click', () => void this.toggleFullscreen())
    for (const part of ['min-buffer', 'max-buffer']) {
      this.part(part).addEventListener('change', () => this.applyBufferPolicy())
    }
    this.part<HTMLSelectElement>('catch-up').addEventListener('change', (event) =>
      player.setCatchUp((event.target as HTMLSelectElement).value as CatchUp)
    )
    this.container.addEventListener('fullscreenchange', () => this.renderFullscreen())
    for (const type of ['pointermove', 'pointerdown', 'keydown']) {
      this.container.addEventListener(type, () => this.markPointerActive())
    }
    for (const button of Array.from(this.root.querySelectorAll<HTMLButtonElement>('[data-skip-seconds]'))) {
      button.addEventListener('click', () => player.skip(Number(button.dataset.skipSeconds)))
    }
    document.addEventListener('keydown', (event) => {
      const seconds = SKIP_SECONDS_BY_KEY[event.key]
      if (seconds === undefined || this.usesArrowKeys(event.target)) {
        return
      }
      event.preventDefault()
      player.skip(seconds)
    })
    seekbar.addEventListener('input', () => {
      this.seeking = true
      this.renderSeekPosition(seekbar.valueAsNumber, seekbar.valueAsNumber, Number(seekbar.max))
    })
    seekbar.addEventListener('change', () => {
      this.seeking = false
      this.seekTo(seekbar.valueAsNumber)
    })
    /// The axis spans the whole broadcast but only the replayable window can be
    /// fetched, so Home lands on that window instead of on a position the relay no
    /// longer holds. End goes live here rather than through the browser, whose End
    /// only fires `change` when it actually moves the thumb.
    seekbar.addEventListener('keydown', (event) => {
      if (event.key === 'End') {
        event.preventDefault()
        this.goLive()
        return
      }
      if (event.key !== 'Home') {
        return
      }
      event.preventDefault()
      this.seeking = false
      const start = player.state.seek.replayableStartSeconds
      seekbar.value = String(start)
      this.seekTo(start)
    })
    for (const type of ['pointercancel', 'blur']) {
      seekbar.addEventListener(type, () => {
        this.seeking = false
        this.renderSeekbar(player.state)
      })
    }
  }

  /// Selects and text inputs use the arrow keys themselves. The seek bar's own
  /// stepping is replaced so that the vertical arrows move by five seconds.
  private usesArrowKeys(target: EventTarget | null): boolean {
    return target instanceof HTMLSelectElement || (target instanceof HTMLInputElement && target !== this.seekbar)
  }

  private goLive(): void {
    this.seeking = false
    this.player.goLive()
  }

  private applyBufferPolicy(): void {
    const policy: BufferPolicy = {
      minimumMs: this.nonNegativeNumber('min-buffer', DEFAULT_BUFFER_POLICY.minimumMs),
      maximumMs: this.nonNegativeNumber('max-buffer', DEFAULT_BUFFER_POLICY.maximumMs)
    }
    this.player.setBufferPolicy(policy)
    this.onLog(
      'info',
      `playout buffer ${policy.minimumMs}–${Number.isFinite(policy.maximumMs) ? policy.maximumMs : '∞'} ms`
    )
  }

  private nonNegativeNumber(part: string, fallback: number): number {
    const text = this.part<HTMLInputElement>(part).value
    const value = Number(text)
    return text !== '' && Number.isFinite(value) && value >= 0 ? value : fallback
  }

  private renderPackaging(state: LivePlayerState): void {
    const select = this.part<HTMLSelectElement>('packaging')
    for (const option of Array.from(select.options)) {
      option.disabled = option.value === 'cmaf' && !state.cmafAvailable
    }
    if (select.value !== state.packaging) {
      select.value = state.packaging
    }
  }

  private renderPlayPause(paused: boolean): void {
    const button = this.part('play-pause')
    if (button.getAttribute('aria-pressed') === String(paused)) {
      return
    }
    button.textContent = paused ? '▶' : '❚❚'
    button.setAttribute('aria-label', paused ? 'Play' : 'Pause')
    button.setAttribute('aria-pressed', String(paused))
  }

  private renderSpeed(state: LivePlayerState): void {
    const select = this.part<HTMLSelectElement>('speed')
    select.disabled = !state.playbackRateAdjustable
    if (select.value !== String(state.playbackRate)) {
      select.value = String(state.playbackRate)
    }
  }

  private async toggleFullscreen(): Promise<void> {
    try {
      if (document.fullscreenElement === this.container) {
        await document.exitFullscreen()
      } else {
        await this.container.requestFullscreen()
      }
    } catch (error) {
      this.onLog('error', `fullscreen: ${getErrorMessage(error)}`)
    }
  }

  private renderFullscreen(): void {
    const fullscreen = document.fullscreenElement === this.container
    const button = this.part('fullscreen')
    button.setAttribute('aria-label', fullscreen ? 'Exit fullscreen' : 'Fullscreen')
    button.title = fullscreen ? '全画面を終了' : '全画面'
    this.markPointerActive()
  }

  private markPointerActive(): void {
    this.container.classList.remove('pointer-idle')
    if (this.pointerIdleTimer !== undefined) {
      clearTimeout(this.pointerIdleTimer)
    }
    this.pointerIdleTimer = setTimeout(() => {
      this.pointerIdleTimer = undefined
      this.container.classList.add('pointer-idle')
    }, POINTER_IDLE_MS)
  }

  private renderSeekbar(state: LivePlayerState): void {
    const { seek } = state
    const { seekbar } = this
    const reviewing = state.mode === 'review'
    this.part('seek-available-window').dataset.seconds = seek.replayableSeconds.toFixed(1)
    this.part('live').classList.toggle('reviewing', reviewing)
    this.renderSpeed(state)
    if (this.seeking) {
      return
    }
    const latest = seek.liveEdgeSeconds
    seekbar.min = String(seek.broadcastStartSeconds ?? seek.replayableStartSeconds)
    seekbar.max = String(latest)
    seekbar.disabled = !seek.seekable
    const anchor = seek.anchorSeconds ?? latest
    const playhead = seek.playheadSeconds ?? latest
    seekbar.valueAsNumber = anchor
    this.renderReplayableWindow(seek.replayableStartSeconds, latest)
    this.renderReviewProgress(reviewing, anchor, playhead, latest)
    this.renderSeekPosition(anchor, playhead, latest)
  }

  private renderReviewProgress(reviewing: boolean, anchor: number, playhead: number, latest: number): void {
    const played = this.part('seek-review-progress')
    played.hidden = !reviewing || latest <= Number(this.seekbar.min)
    if (played.hidden) {
      return
    }
    played.style.left = this.percentOfAxis(anchor - Number(this.seekbar.min), latest)
    played.style.width = this.percentOfAxis(Math.max(0, playhead - anchor), latest)
  }

  private renderReplayableWindow(replayableStart: number, latest: number): void {
    const min = Number(this.seekbar.min)
    const window = this.part('seek-available-window')
    window.style.left = this.percentOfAxis(replayableStart - min, latest)
    window.style.width = this.percentOfAxis(latest - replayableStart, latest)
    const elapsed = this.player.elapsedMsAt(min * MICROS_PER_SECOND)
    this.part('seek-start').textContent = elapsed === undefined ? '--:--' : formatElapsed(elapsed)
  }

  private percentOfAxis(seconds: number, latest: number): string {
    const axis = latest - Number(this.seekbar.min)
    return axis > 0 ? `${((seconds / axis) * 100).toFixed(3)}%` : '0%'
  }

  /// `seekbar.max` is frozen while a drag is in progress, so comparing against it
  /// rather than against the live edge keeps the right end meaning "go live" even
  /// when a group arrives mid-gesture.
  private seekTo(captureSeconds: number): void {
    if (captureSeconds >= Number(this.seekbar.max)) {
      this.goLive()
      return
    }
    this.player.seek(captureSeconds * MICROS_PER_SECOND)
  }

  private renderSeekPosition(anchor: number, playhead: number, latest: number): void {
    const behind = Math.max(0, latest - playhead)
    this.part('seek-position').textContent = behind < 0.1 ? 'LIVE' : `-${behind.toFixed(1)}s`
    const thumbBehind = Math.max(0, latest - anchor)
    this.seekbar.setAttribute(
      'aria-valuetext',
      thumbBehind < 0.1 ? 'Live' : `${thumbBehind.toFixed(1)} seconds behind live`
    )
    this.renderSeekElapsed(playhead, latest)
  }

  private renderSeekElapsed(position: number, latest: number): void {
    const elapsed = this.player.elapsedMsAt(position * MICROS_PER_SECOND)
    const broadcast = this.player.elapsedMsAt(latest * MICROS_PER_SECOND)
    this.part('seek-elapsed').textContent =
      elapsed === undefined || broadcast === undefined
        ? '--:-- / --:--'
        : `${formatElapsed(elapsed)} / ${formatElapsed(broadcast)}`
  }
}

function fillSelect(
  select: HTMLSelectElement,
  tracks: MediaCatalogTrack[],
  selected: string,
  describe: (track: MediaCatalogTrack) => string
): void {
  const names = tracks.map((track) => track.name)
  const current = Array.from(select.options).map((option) => option.value)
  if (names.length !== current.length || names.some((name, index) => name !== current[index])) {
    select.replaceChildren(
      ...tracks.map((track) => {
        const option = document.createElement('option')
        option.value = track.name
        option.textContent = describe(track)
        return option
      })
    )
  }
  if (select.value !== selected) {
    select.value = selected
  }
}

function describeVideoTrack(track: MediaCatalogTrack): string {
  const resolution = track.width && track.height ? ` (${track.width}x${track.height})` : ''
  return `${track.label}${resolution}`
}
