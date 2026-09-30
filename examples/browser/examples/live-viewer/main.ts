import { MoqtClientWrapper } from '@moqt/moqtClient'
import { CLOUD_RELAY_PRESETS, LOAD_BALANCED_RELAY_PRESET } from '../../utils/relayPresets'
import { MEDIA_CATALOG_TRACK_NAME, type MediaCatalogTrack } from '../media/catalog'
import {
  getErrorMessage,
  initializeMediaExamplePage,
  parseTrackNamespace,
  setStatus,
  setStatusText
} from '../media/common'
import type { BufferPolicy } from './jitterBuffer'
import { type CatchUp, DEFAULT_BUFFER_POLICY } from './livePlayout'
import type { LivePictureKind } from './livePictureSink'
import { formatElapsed } from './mediaTimeline'
import { Mp4Publisher } from './mp4Publisher'
import { PlaybackCharts, SAMPLE_INTERVAL_MS } from './playbackCharts'
import { LivePlayer, type LivePlayerState, type Packaging, formatSyncOffset } from './player/livePlayer'
import { PublishPreview } from './publishPreview'
import {
  DEFAULT_WINDOW_SECONDS,
  StreamMonitor,
  type StreamRecord,
  renderDeliveryGrid,
  renderIdleStreamMonitor,
  renderStreamMonitor,
  streamKbps,
  summarizeStreams
} from './streamMonitor'

const AUTH_INFO = 'secret'
const MICROS_PER_SECOND = 1_000_000
const SKIP_SECONDS_BY_KEY: Record<string, number> = { ArrowLeft: -1, ArrowRight: 1, ArrowDown: -5, ArrowUp: 5 }
const MSE_ELEMENT_IDS = ['mse-a', 'mse-b', 'mse-c']
const POINTER_IDLE_MS = 2_500

const moqtClient = new MoqtClientWrapper()
const streamMonitor = new StreamMonitor()
const seekbar = element<HTMLInputElement>('seekbar')
const stage = element<HTMLDivElement>('stage')
let seeking = false
let pointerIdleTimer: ReturnType<typeof setTimeout> | undefined
let streamWindowSeconds = DEFAULT_WINDOW_SECONDS
const player = new LivePlayer({
  client: moqtClient,
  authInfo: AUTH_INFO,
  surface: {
    video: element<HTMLVideoElement>('video'),
    liveCanvas: element<HTMLCanvasElement>('live-canvas'),
    reviewCanvas: element<HTMLCanvasElement>('review'),
    msePool: MSE_ELEMENT_IDS.map((id) => element<HTMLVideoElement>(id))
  },
  callbacks: { onStateChange: renderPlayer, onLiveFrame: renderVideoStats, onLog: appendLog },
  deliveryObserver: streamMonitor,
  /// `?livePicture=canvas` forces the canvas sink, to see the Safari path in Chrome.
  livePicture: (new URLSearchParams(location.search).get('livePicture') as LivePictureKind | null) ?? undefined
})
const playbackCharts = new PlaybackCharts(element<HTMLElement>('playback-charts'))
setInterval(() => {
  if (player.state.started) {
    const stats = player.stats()
    const buffering = stats.bufferMs !== undefined
    playbackCharts.push({
      delayMs: stats.viewerDelayMs,
      bufferMs: stats.bufferMs,
      targetMs: buffering ? stats.targetBufferMs : undefined,
      outputLatencyMs: buffering ? stats.outputLatencyMs : undefined,
      spreadMs: buffering ? stats.arrivalSpreadMs : undefined,
      kbps: stats.receivedKbps,
      syncMs: stats.syncOffsetMs
    })
  }
}, SAMPLE_INTERVAL_MS)
const mp4Publisher = new Mp4Publisher(
  { onStatus: (text, state) => setStatus('publish-status', text, state), onLog: appendLog },
  new PublishPreview(element<HTMLCanvasElement>('publish-preview'))
)

for (const preset of [LOAD_BALANCED_RELAY_PRESET, ...CLOUD_RELAY_PRESETS]) {
  const button = document.createElement('button')
  button.type = 'button'
  button.dataset.url = preset.value
  button.textContent = `Cloud ${preset.label}`
  button.title = preset.helper
  element('urlPresets').appendChild(button)
}
initializeMediaExamplePage('namespace')
player.setVolume(element<HTMLInputElement>('volume').valueAsNumber)
element<HTMLButtonElement>('watchBtn').addEventListener('click', () => void watchStream())
element<HTMLButtonElement>('stopBtn').addEventListener('click', () => void stopStream())
element<HTMLButtonElement>('publishBtn').addEventListener('click', () => void publishMp4())
element<HTMLButtonElement>('stopPublishBtn').addEventListener('click', () => void mp4Publisher.stop())
element<HTMLSelectElement>('video-track').addEventListener(
  'change',
  (event) => void player.selectVideoTrack((event.target as HTMLSelectElement).value)
)
element<HTMLSelectElement>('audio-track').addEventListener(
  'change',
  (event) => void player.selectAudioTrack((event.target as HTMLSelectElement).value)
)
element<HTMLSelectElement>('packaging').addEventListener(
  'change',
  (event) => void player.setPackaging((event.target as HTMLSelectElement).value as Packaging)
)
element<HTMLSelectElement>('speed').addEventListener('change', (event) =>
  player.setPlaybackRate(Number((event.target as HTMLSelectElement).value))
)
element<HTMLButtonElement>('liveBtn').addEventListener('click', goLive)
element<HTMLButtonElement>('playPauseBtn').addEventListener('click', () => player.setPaused(!player.state.paused))
element<HTMLInputElement>('volume').addEventListener('input', (event) =>
  player.setVolume((event.target as HTMLInputElement).valueAsNumber)
)
element<HTMLButtonElement>('fullscreenBtn').addEventListener('click', () => void toggleFullscreen())
element<HTMLButtonElement>('deliveryToggleBtn').addEventListener('click', () => {
  const overlay = element<HTMLDivElement>('delivery-overlay')
  overlay.hidden = !overlay.hidden
  element<HTMLButtonElement>('deliveryToggleBtn').setAttribute('aria-pressed', String(!overlay.hidden))
})
stage.addEventListener('fullscreenchange', renderFullscreen)
for (const type of ['pointermove', 'pointerdown', 'keydown']) {
  stage.addEventListener(type, markPointerActive)
}
for (const button of Array.from(document.querySelectorAll<HTMLButtonElement>('[data-skip-seconds]'))) {
  button.addEventListener('click', () => player.skip(Number(button.dataset.skipSeconds)))
}
document.addEventListener('keydown', (event) => {
  const seconds = SKIP_SECONDS_BY_KEY[event.key]
  if (seconds === undefined || usesArrowKeys(event.target)) {
    return
  }
  event.preventDefault()
  player.skip(seconds)
})
seekbar.addEventListener('input', () => {
  seeking = true
  renderSeekPosition(seekbar.valueAsNumber, seekbar.valueAsNumber, Number(seekbar.max))
})
seekbar.addEventListener('change', () => {
  seeking = false
  seekTo(seekbar.valueAsNumber)
})
/// The axis spans the whole broadcast but only the replayable window can be
/// fetched, so Home lands on that window instead of on a position the relay no
/// longer holds. End goes live here rather than through the browser, whose End
/// only fires `change` when it actually moves the thumb.
seekbar.addEventListener('keydown', (event) => {
  if (event.key === 'End') {
    event.preventDefault()
    goLive()
    return
  }
  if (event.key !== 'Home') {
    return
  }
  event.preventDefault()
  seeking = false
  const start = player.state.seek.replayableStartSeconds
  seekbar.value = String(start)
  seekTo(start)
})
for (const event of ['pointercancel', 'blur']) {
  seekbar.addEventListener(event, () => {
    seeking = false
    renderSeekbar(player.state)
  })
}
moqtClient.setOnSubgroupHeaderHandler((header) => streamMonitor.opened(header.trackAlias, header.groupId))
element<HTMLInputElement>('stream-gops').addEventListener('input', (event) => {
  const keptGroups = Number((event.target as HTMLInputElement).value) || 1
  streamMonitor.setKeptGroups(keptGroups)
  mp4Publisher.sentStreams.setKeptGroups(keptGroups)
})
for (const id of ['playout-buffer', 'max-buffer']) {
  element(id).addEventListener('change', applyBufferPolicy)
}
element<HTMLSelectElement>('catch-up').addEventListener('change', (event) => {
  player.setCatchUp((event.target as HTMLSelectElement).value as CatchUp)
})
element<HTMLInputElement>('stream-window').addEventListener('input', (event) => {
  streamWindowSeconds = Number((event.target as HTMLInputElement).value) || DEFAULT_WINDOW_SECONDS
})
requestAnimationFrame(function renderStreamsEachFrame() {
  renderStreams()
  requestAnimationFrame(renderStreamsEachFrame)
})

async function watchStream(): Promise<void> {
  try {
    await stopStream()
    const url = element<HTMLInputElement>('url').value.trim()
    await moqtClient.connect(url)
    setStatus('connection-status', `Connected: ${url}`, 'ok')
    appendLog('info', `connected to ${url}`)
    await player.start(trackNamespace())
  } catch (error) {
    setStatus('connection-status', `Failed: ${getErrorMessage(error)}`, 'error')
    appendLog('error', getErrorMessage(error))
  }
}

async function stopStream(): Promise<void> {
  playbackCharts.reset()
  streamMonitor.reset()
  await player.stop()
  if (moqtClient.getConnectionStatus()) {
    await moqtClient.disconnect()
  }
  setStatus('connection-status', 'Not connected', 'idle')
}

async function publishMp4(): Promise<void> {
  const file = element<HTMLInputElement>('mp4-file').files?.[0]
  if (!file) {
    setStatus('publish-status', 'Choose an MP4 file first', 'error')
    return
  }
  setStatus('publish-status', `Opening ${file.name}`, 'idle')
  try {
    await mp4Publisher.start({
      file,
      url: element<HTMLInputElement>('url').value.trim(),
      namespace: trackNamespace(),
      authInfo: AUTH_INFO,
      loop: element<HTMLInputElement>('mp4-loop').checked
    })
  } catch (error) {
    setStatus('publish-status', `Publish failed: ${getErrorMessage(error)}`, 'error')
    appendLog('error', `publish: ${getErrorMessage(error)}`)
  }
}

function renderPlayer(): void {
  const state = player.state
  setStatus('catalog-status', state.catalogStatus.text, state.catalogStatus.state)
  setStatus('playback-status', state.playbackStatus.text, state.playbackStatus.state)
  setStatus('rewind-status', state.rewindStatus.text, state.rewindStatus.state)
  fillSelect(element<HTMLSelectElement>('video-track'), state.videoTracks, state.selectedVideoTrack, describeVideoTrack)
  fillSelect(
    element<HTMLSelectElement>('audio-track'),
    state.audioTracks,
    state.selectedAudioTrack,
    (track) => track.label
  )
  renderPackagingOptions(state)
  renderPlayPause(state.paused)
  element<HTMLElement>('buffering').hidden = !state.stalled
  renderSeekbar(state)
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

function renderPackagingOptions(state: LivePlayerState): void {
  const select = element<HTMLSelectElement>('packaging')
  for (const option of Array.from(select.options)) {
    option.disabled = option.value === 'cmaf' && !state.cmafAvailable
  }
  if (select.value !== state.packaging) {
    select.value = state.packaging
  }
}

function renderPlayPause(paused: boolean): void {
  const button = element<HTMLButtonElement>('playPauseBtn')
  if (button.getAttribute('aria-pressed') === String(paused)) {
    return
  }
  button.textContent = paused ? '▶' : '❚❚'
  button.setAttribute('aria-label', paused ? 'Play' : 'Pause')
  button.setAttribute('aria-pressed', String(paused))
}

function applyBufferPolicy(): void {
  const policy: BufferPolicy = {
    minimumMs: nonNegativeNumber('playout-buffer', DEFAULT_BUFFER_POLICY.minimumMs),
    maximumMs: nonNegativeNumber('max-buffer', DEFAULT_BUFFER_POLICY.maximumMs)
  }
  player.setBufferPolicy(policy)
  appendLog(
    'info',
    `playout buffer ${policy.minimumMs}–${Number.isFinite(policy.maximumMs) ? policy.maximumMs : '∞'} ms`
  )
}

function nonNegativeNumber(id: string, fallback: number): number {
  const text = element<HTMLInputElement>(id).value
  const value = Number(text)
  return text !== '' && Number.isFinite(value) && value >= 0 ? value : fallback
}

function renderVideoStats(): void {
  const stats = player.stats()
  const delay = stats.viewerDelayMs === undefined ? '' : ` · delay ${Math.round(stats.viewerDelayMs)} ms`
  const buffer =
    stats.bufferMs === undefined
      ? ''
      : ` · buffer ${Math.round(stats.bufferMs)} ms (target ${Math.round(stats.targetBufferMs)})`
  element('buffer-current').textContent =
    stats.bufferMs === undefined
      ? 'Current: -'
      : `Current: ${Math.round(stats.bufferMs)} ms (${stats.fixedBuffer ? 'fixed' : `target ${Math.round(stats.targetBufferMs)}`})`
  const size = stats.frameSize ? `${stats.frameSize.width}x${stats.frameSize.height}` : ''
  element<HTMLSpanElement>('video-stats').textContent =
    `${size}${delay}${buffer} · ${Math.round(stats.receivedKbps)} kbps · ${stats.videoObjects} objects · A/V ${formatSyncOffset(stats.syncOffsetMs)} · audio breaks ${stats.audioBreaks} · video ${stats.videoDrops} · shed ${Math.round(stats.shedMs)} ms`
}

function renderPublishStreams(state: LivePlayerState): void {
  element<HTMLElement>('publish-live').style.display = mp4Publisher.publishing ? '' : 'none'
  const viewerDelayMs = player.stats().viewerDelayMs
  element<HTMLSpanElement>('publish-latency').textContent =
    state.started && state.packaging === 'loc' && state.mode === 'live' && viewerDelayMs !== undefined
      ? `viewer delay ${Math.round(viewerDelayMs)} ms`
      : 'viewer delay -'
  if (!mp4Publisher.publishing) {
    return
  }
  const now = Date.now()
  const records = mp4Publisher.sentStreams.snapshot()
  renderStreamMonitor(
    element<SVGSVGElement>('publish-stream-monitor'),
    records,
    mp4Publisher.sentStreams.slotsPerTrack(),
    streamWindowSeconds,
    [],
    now
  )
  element<HTMLSpanElement>('publish-stream-stats').textContent = records.length
    ? `${summarizeStreams(records, [], now)} · ${Math.round(streamKbps(records, streamWindowSeconds, now))} kbps`
    : 'no subscriber yet'
}

function renderStreams(): void {
  const state = player.state
  renderPublishStreams(state)
  const reviewGrid = element<SVGSVGElement>('delivery-grid-review')
  const reviewTimeline = element<SVGSVGElement>('stream-monitor-review')
  if (!state.started) {
    renderIdleStreamMonitor(element<SVGSVGElement>('stream-monitor'))
    renderIdleStreamMonitor(element<SVGSVGElement>('delivery-grid'))
    reviewGrid.style.display = 'none'
    reviewTimeline.style.display = 'none'
    element<HTMLSpanElement>('stream-stats').textContent = '-'
    return
  }
  const reviewing = state.mode === 'review'
  const { liveVideoAlias, liveAudioAlias, mediaTimelineTrackName, reviewFetchWindows } = state.delivery
  const now = Date.now()
  const records = streamMonitor.snapshot()
  const liveRecords = records.filter(
    (record) =>
      record.kind === 'subscribe' &&
      (!reviewing || record.track === MEDIA_CATALOG_TRACK_NAME || record.track === mediaTimelineTrackName)
  )
  const fetchRecords = records.filter((record) => record.kind === 'fetch')
  const playheads = streamMonitor.currentPlayheads()
  const livePlayhead = playheads.find((playhead) => playhead.kind === 'subscribe')
  const reviewPlayhead = playheads.find((playhead) => playhead.kind === 'fetch')
  renderStreamMonitor(
    element<SVGSVGElement>('stream-monitor'),
    liveRecords,
    streamMonitor.slotsPerTrack(),
    streamWindowSeconds,
    livePlayhead ? [livePlayhead] : [],
    now
  )
  element<HTMLSpanElement>('stream-stats').textContent = summarizeStreams(records, playheads, now)
  reviewTimeline.style.display = reviewing && fetchRecords.length > 0 ? '' : 'none'
  if (reviewing && fetchRecords.length > 0) {
    renderStreamMonitor(
      reviewTimeline,
      fetchRecords,
      streamMonitor.slotsPerTrack(),
      streamWindowSeconds,
      reviewPlayhead ? [reviewPlayhead] : [],
      latestFetchActivity(fetchRecords, now)
    )
  }
  const liveGrid = element<SVGSVGElement>('delivery-grid')
  liveGrid.style.display = reviewing ? 'none' : ''
  if (!reviewing) {
    renderDeliveryGrid(
      liveGrid,
      records,
      [
        { label: 'audio', kind: 'subscribe', trackAlias: liveAudioAlias },
        { label: 'video', kind: 'subscribe', trackAlias: liveVideoAlias }
      ],
      streamMonitor.slotsPerTrack(),
      livePlayhead
    )
  }
  reviewGrid.style.display = reviewing ? '' : 'none'
  if (reviewing) {
    const fetchIds =
      reviewFetchWindows.find((window) => window.video === reviewPlayhead?.trackAlias) ?? reviewFetchWindows[0] ?? {}
    renderDeliveryGrid(
      reviewGrid,
      records,
      [
        { label: 'fetch audio', kind: 'fetch', trackAlias: fetchIds.audio, cadenceAlias: liveAudioAlias },
        { label: 'fetch video', kind: 'fetch', trackAlias: fetchIds.video, cadenceAlias: liveVideoAlias }
      ],
      streamMonitor.slotsPerTrack(),
      reviewPlayhead
    )
  }
}

/// A FETCH arrives in a burst well before it is played, so the review
/// timeline ends at the newest FETCH activity rather than at the wall clock.
function latestFetchActivity(records: StreamRecord[], now: number): number {
  return Math.max(...records.map((record) => record.finishedAt ?? now))
}

function trackNamespace(): string[] {
  return parseTrackNamespace(element<HTMLInputElement>('namespace').value)
}

function appendLog(level: 'info' | 'warn' | 'error', message: string): void {
  const panel = document.getElementById('logPanel')
  if (!panel) {
    return
  }

  const entry = document.createElement('div')
  entry.className = `log-entry log-${level}`
  entry.textContent = `${new Date().toLocaleTimeString()} ${message}`
  panel.prepend(entry)
}

function element<T extends Element>(id: string): T {
  const found = document.getElementById(id)
  if (!found) {
    throw new Error(`missing element: ${id}`)
  }
  return found as Element as T
}

/// Selects and text inputs use the arrow keys themselves. The seek bar's own
/// stepping is replaced so that the vertical arrows move by five seconds.
function usesArrowKeys(target: EventTarget | null): boolean {
  return target instanceof HTMLSelectElement || (target instanceof HTMLInputElement && target !== seekbar)
}

function goLive(): void {
  seeking = false
  player.goLive()
}

async function toggleFullscreen(): Promise<void> {
  try {
    if (document.fullscreenElement === stage) {
      await document.exitFullscreen()
    } else {
      await stage.requestFullscreen()
    }
  } catch (error) {
    appendLog('error', `fullscreen: ${getErrorMessage(error)}`)
  }
}

function renderFullscreen(): void {
  const fullscreen = document.fullscreenElement === stage
  const button = element<HTMLButtonElement>('fullscreenBtn')
  button.setAttribute('aria-label', fullscreen ? 'Exit fullscreen' : 'Fullscreen')
  button.title = fullscreen ? '全画面を終了' : '全画面'
  markPointerActive()
}

function markPointerActive(): void {
  stage.classList.remove('pointer-idle')
  if (pointerIdleTimer !== undefined) {
    clearTimeout(pointerIdleTimer)
  }
  pointerIdleTimer = setTimeout(() => {
    pointerIdleTimer = undefined
    stage.classList.add('pointer-idle')
  }, POINTER_IDLE_MS)
}

function renderSpeed(state: LivePlayerState): void {
  const select = element<HTMLSelectElement>('speed')
  select.disabled = !state.playbackRateAdjustable
  if (select.value !== String(state.playbackRate)) {
    select.value = String(state.playbackRate)
  }
}

function renderSeekbar(state: LivePlayerState): void {
  const { seek } = state
  const reviewing = state.mode === 'review'
  element<HTMLDivElement>('seek-available-window').dataset.seconds = seek.replayableSeconds.toFixed(1)
  element<HTMLButtonElement>('liveBtn').classList.toggle('reviewing', reviewing)
  renderSpeed(state)
  if (seeking) {
    return
  }
  const latest = seek.liveEdgeSeconds
  seekbar.min = String(seek.broadcastStartSeconds ?? seek.replayableStartSeconds)
  seekbar.max = String(latest)
  seekbar.disabled = !seek.seekable
  const anchor = seek.anchorSeconds ?? latest
  const playhead = seek.playheadSeconds ?? latest
  seekbar.valueAsNumber = anchor
  renderReplayableWindow(seek.replayableStartSeconds, latest)
  renderReviewProgress(reviewing, anchor, playhead, latest)
  renderSeekPosition(anchor, playhead, latest)
}

function renderReviewProgress(reviewing: boolean, anchor: number, playhead: number, latest: number): void {
  const played = element<HTMLDivElement>('seek-review-progress')
  played.hidden = !reviewing || latest <= Number(seekbar.min)
  if (played.hidden) {
    return
  }
  played.style.left = percentOfAxis(anchor - Number(seekbar.min), latest)
  played.style.width = percentOfAxis(Math.max(0, playhead - anchor), latest)
}

function renderReplayableWindow(replayableStart: number, latest: number): void {
  const min = Number(seekbar.min)
  const window = element<HTMLDivElement>('seek-available-window')
  window.style.left = percentOfAxis(replayableStart - min, latest)
  window.style.width = percentOfAxis(latest - replayableStart, latest)
  const elapsed = player.elapsedMsAt(min * MICROS_PER_SECOND)
  setStatusText('seek-start', elapsed === undefined ? '--:--' : formatElapsed(elapsed))
}

function percentOfAxis(seconds: number, latest: number): string {
  const axis = latest - Number(seekbar.min)
  return axis > 0 ? `${((seconds / axis) * 100).toFixed(3)}%` : '0%'
}

/// `seekbar.max` is frozen while a drag is in progress, so comparing against it
/// rather than against the live edge keeps the right end meaning "go live" even
/// when a group arrives mid-gesture.
function seekTo(captureSeconds: number): void {
  if (captureSeconds >= Number(seekbar.max)) {
    goLive()
    return
  }
  player.seek(captureSeconds * MICROS_PER_SECOND)
}

function renderSeekPosition(anchor: number, playhead: number, latest: number): void {
  const behind = Math.max(0, latest - playhead)
  const label = behind < 0.1 ? 'LIVE' : `-${behind.toFixed(1)}s`
  setStatusText('seek-position', label)
  const thumbBehind = Math.max(0, latest - anchor)
  seekbar.setAttribute('aria-valuetext', thumbBehind < 0.1 ? 'Live' : `${thumbBehind.toFixed(1)} seconds behind live`)
  renderSeekElapsed(playhead, latest)
}

function renderSeekElapsed(position: number, latest: number): void {
  const elapsed = player.elapsedMsAt(position * MICROS_PER_SECOND)
  const broadcast = player.elapsedMsAt(latest * MICROS_PER_SECOND)
  setStatusText(
    'seek-elapsed',
    elapsed === undefined || broadcast === undefined
      ? '--:-- / --:--'
      : `${formatElapsed(elapsed)} / ${formatElapsed(broadcast)}`
  )
}
